use std::path::Path;
use std::time::Duration;

use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSourceReader, MF_LOW_LATENCY, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, MF_SOURCE_READERF_ENDOFSTREAM,
    MFCreateAttributes, MFCreateMediaType, MFCreateSourceReaderFromURL, MFMediaType_Video,
    MFVideoFormat_RGB32,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{GUID, HSTRING};

use rewind_trace::Traced;

use crate::error::{MediaError, Result};
use crate::player::Frame;
use crate::png::{Bgra, write_png};
use crate::probe::video_stream;
use crate::span::Span;

const MAX_ATTEMPTS: u32 = 120;

fn pack(width: u32, height: u32) -> u64 {
    (u64::from(width) << 32) | u64::from(height)
}

fn select_video(source: &Path) -> Result<(IMFSourceReader, u32)> {
    let mut attributes: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attributes, 2) }?;
    let attributes = attributes.ok_or(MediaError::NoFrame)?;
    unsafe {
        attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        attributes.SetUINT32(&MF_LOW_LATENCY, 1)?;
    }
    let reader =
        unsafe { MFCreateSourceReaderFromURL(&HSTRING::from(source.as_os_str()), &attributes) }?;
    let stream = video_stream(&reader).ok_or(MediaError::NoVideo)?;
    unsafe {
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        reader.SetStreamSelection(stream, true)?;
    }
    Ok((reader, stream))
}

fn set_output(
    reader: &IMFSourceReader,
    stream: u32,
    size: Option<(u32, u32)>,
) -> Result<(u32, u32, i32)> {
    let output = unsafe { MFCreateMediaType() }?;
    unsafe {
        output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        if let Some((width, height)) = size {
            output.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
        }
        reader.SetCurrentMediaType(stream, None, &output)?;
    }
    let current = unsafe { reader.GetCurrentMediaType(stream) }?;
    let frame_size = unsafe { current.GetUINT64(&MF_MT_FRAME_SIZE) }?;
    let (width, height) = ((frame_size >> 32) as u32, (frame_size & 0xffff_ffff) as u32);
    let stride = unsafe { current.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
        .map_or((width * 4) as i32, |stride| stride as i32);
    Ok((width, height, stride))
}

fn open(source: &Path, size: Option<(u32, u32)>) -> Result<(IMFSourceReader, u32, u32, u32, i32)> {
    let (reader, stream) = select_video(source)?;
    let (width, height, stride) = set_output(&reader, stream, size)?;
    Ok((reader, stream, width, height, stride))
}

fn seek(reader: &IMFSourceReader, at: Duration) -> Result<()> {
    unsafe { reader.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(Span::hns(at))) }?;
    Ok(())
}

fn read_frame(
    reader: &IMFSourceReader,
    stream: u32,
    width: u32,
    height: u32,
    stride: i32,
) -> Result<Frame> {
    for _ in 0..MAX_ATTEMPTS {
        let mut flags = 0u32;
        let mut sample = None;
        unsafe { reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample)) }?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            break;
        }
        let Some(sample) = sample else {
            continue;
        };
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }?;
        let mut data = std::ptr::null_mut();
        let mut length = 0u32;
        unsafe { buffer.Lock(&mut data, None, Some(&mut length)) }?;
        let bytes = unsafe { std::slice::from_raw_parts(data, length as usize) };
        let row = (width * 4) as usize;
        let pitch = stride.unsigned_abs() as usize;
        let mut pixels = Vec::with_capacity(row * height as usize);
        for line in 0..height as usize {
            let source = if stride < 0 {
                height as usize - 1 - line
            } else {
                line
            };
            let start = source * pitch;
            if let Some(chunk) = bytes.get(start..start + row) {
                pixels.extend_from_slice(chunk);
            }
        }
        unsafe { buffer.Unlock() }?;
        return Ok(Frame {
            width,
            height,
            pixels,
        });
    }
    Err(MediaError::NoFrame)
}

pub fn frames_at(
    source: &Path,
    times: &[Duration],
    size: Option<(u32, u32)>,
) -> Result<Vec<Frame>> {
    let (reader, stream, width, height, stride) = open(source, size)?;
    times
        .iter()
        .map(|time| {
            seek(&reader, *time)?;
            read_frame(&reader, stream, width, height, stride)
        })
        .collect()
}

fn scaled_size(width: u32, height: u32, max_width: u32) -> Option<(u32, u32)> {
    let scaled = u64::from(height) * u64::from(max_width);
    (max_width > 0 && max_width < width && scaled % u64::from(width) == 0)
        .then(|| (max_width, (scaled / u64::from(width)) as u32))
        .filter(|(_, height)| *height > 0)
}

fn scaled_frame(source: &Path, at: Duration, max_width: u32) -> Result<Frame> {
    let (reader, stream) = select_video(source)?;
    let (width, height, _) = set_output(&reader, stream, None)?;
    let size = scaled_size(width, height, max_width).ok_or(MediaError::NoFrame)?;
    let (width, height, stride) = set_output(&reader, stream, Some(size))?;
    seek(&reader, at)?;
    let frame = read_frame(&reader, stream, width, height, stride)?;
    let complete = frame.pixels.len() == width as usize * height as usize * 4;
    ((width, height) == size && complete)
        .then_some(frame)
        .ok_or(MediaError::NoFrame)
}

fn full_frame(source: &Path, at: Duration) -> Result<Frame> {
    frames_at(source, &[at], None)?
        .into_iter()
        .next()
        .ok_or(MediaError::NoFrame)
}

pub fn thumbnail(source: &Path, at: Duration, max_width: u32, destination: &Path) -> Result<()> {
    let frame = match scaled_frame(source, at, max_width) {
        Ok(frame) => frame,
        Err(_) => full_frame(source, at).traced("media", "read a frame for a thumbnail")?,
    };
    write_png(
        destination,
        &Bgra {
            width: frame.width,
            height: frame.height,
            stride: frame.width * 4,
            pixels: &frame.pixels,
        },
        Some(max_width),
    )
}
