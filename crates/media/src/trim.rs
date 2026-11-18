use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaType, IMFSample, MF_MT_MAJOR_TYPE, MF_SINK_WRITER_DISABLE_THROTTLING,
    MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_ANY_STREAM, MF_SOURCE_READERF_ENDOFSTREAM,
    MF_TRANSCODE_CONTAINERTYPE, MFCreateAttributes, MFCreateSinkWriterFromURL, MFMediaType_Video,
    MFSampleExtension_CleanPoint, MFSampleExtension_DecodeTimestamp,
    MFTranscodeContainerType_MPEG4,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{GUID, HSTRING};

use crate::error::{MediaError, Result};
use crate::probe::open_reader;
use crate::span::Span;

const TEMP_SUFFIX: &str = "trim.tmp";
const REPLACE_ATTEMPTS: u32 = 8;
const REPLACE_BACKOFF: Duration = Duration::from_millis(150);

struct Stream {
    output: u32,
    video: bool,
    done: bool,
}

pub(crate) fn writer_attributes() -> Result<IMFAttributes> {
    let mut attributes: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attributes, 2) }?;
    let attributes = attributes.ok_or(MediaError::NoVideo)?;
    unsafe {
        attributes.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
        attributes.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
    }
    Ok(attributes)
}

pub(crate) fn shift(sample: &IMFSample, time: i64, by: i64) -> Result<()> {
    unsafe { sample.SetSampleTime(time + by) }?;
    if let Ok(decode) = unsafe { sample.GetUINT64(&MFSampleExtension_DecodeTimestamp) } {
        let moved = (decode as i64).saturating_add(by);
        unsafe { sample.SetUINT64(&MFSampleExtension_DecodeTimestamp, moved as u64) }?;
    }
    Ok(())
}

fn is_video(media_type: &IMFMediaType) -> bool {
    unsafe { media_type.GetGUID(&MF_MT_MAJOR_TYPE) }.is_ok_and(|major| major == MFMediaType_Video)
}

pub fn trim(source: &Path, span: Span, destination: &Path) -> Result<()> {
    if span.length().is_zero() {
        return Err(MediaError::EmptyRange);
    }
    let reader = open_reader(source)?;
    unsafe { reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, true) }?;
    let writer = unsafe {
        MFCreateSinkWriterFromURL(
            &HSTRING::from(destination.as_os_str()),
            None,
            &writer_attributes()?,
        )
    }?;
    let mut streams = Vec::with_capacity(3);
    for index in 0.. {
        let Ok(native) = (unsafe { reader.GetNativeMediaType(index, 0) }) else {
            break;
        };
        let output = unsafe { writer.AddStream(&native) }?;
        unsafe { writer.SetInputMediaType(output, &native, None) }?;
        streams.push(Stream {
            output,
            video: is_video(&native),
            done: false,
        });
    }
    if !streams.iter().any(|stream| stream.video) {
        return Err(MediaError::NoVideo);
    }
    let start = PROPVARIANT::from(Span::hns(span.start));
    unsafe { reader.SetCurrentPosition(&GUID::zeroed(), &start) }?;
    unsafe { writer.BeginWriting() }?;

    let end = Span::hns(span.end);
    let mut origin: Option<i64> = None;
    while streams.iter().any(|stream| !stream.done) {
        let mut index = 0u32;
        let mut flags = 0u32;
        let mut time = 0i64;
        let mut sample = None;
        unsafe {
            reader.ReadSample(
                MF_SOURCE_READER_ANY_STREAM.0 as u32,
                0,
                Some(&mut index),
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )
        }?;
        let Some(stream) = streams.get_mut(index as usize) else {
            break;
        };
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            stream.done = true;
        }
        let Some(sample) = sample else {
            continue;
        };
        if time >= end {
            stream.done = true;
            continue;
        }
        let keyframe = unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }.unwrap_or(0) != 0;
        let base = match origin {
            Some(base) => base,
            None if stream.video && keyframe => *origin.insert(time),
            None => continue,
        };
        if time < base {
            continue;
        }
        shift(&sample, time, -base)?;
        unsafe { writer.WriteSample(stream.output, &sample) }?;
    }
    unsafe { writer.Finalize() }?;
    Ok(())
}

pub(crate) fn temp_path(source: &Path) -> PathBuf {
    source.with_extension(format!("{TEMP_SUFFIX}.mp4"))
}

pub(crate) fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) if attempt + 1 >= REPLACE_ATTEMPTS => return Err(error),
            Err(_) => {
                attempt += 1;
                thread::sleep(REPLACE_BACKOFF * attempt);
            }
        }
    }
}

pub fn trim_in_place(source: &Path, span: Span) -> Result<()> {
    let temp = temp_path(source);
    let result = trim(source, span, &temp).and_then(|()| Ok(replace(&temp, source)?));
    if result.is_err() {
        std::fs::remove_file(&temp).ok();
    }
    result
}
