use std::path::Path;
use std::time::Duration;

use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaType, IMFSinkWriter, IMFSourceReader, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS,
    MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
    MF_SINK_WRITER_DISABLE_THROTTLING, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, MF_SOURCE_READERF_ENDOFSTREAM,
    MF_TRANSCODE_CONTAINERTYPE, MFAudioFormat_AAC, MFAudioFormat_Float, MFAudioFormat_PCM,
    MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFCreateSourceReaderFromURL, MFMediaType_Audio, MFMediaType_Video,
    MFTranscodeContainerType_MPEG4, MFVideoFormat_H264, MFVideoFormat_RGB32,
    MFVideoInterlace_Progressive, eAVEncH264VProfile_High,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{GUID, HSTRING};

use rewind_trace::{Traced, trace};

use crate::edit::{Edit, PixelRect, Stroke};
use crate::effects::{Coverage, Image, Plane, blur, paint_mask, scale_into, stroke_mask};
use crate::error::{MediaError, Result};
use crate::mixdown::{CHANNELS, mixdown, resample, separate, to_stereo};
use crate::probe::tracks;
use crate::span::Span;
use crate::stretch::stretch;
use crate::trim::{replace, temp_path, trim};

const HNS: i64 = 10_000_000;
const RATE: u32 = 48_000;
const AAC_BYTES_PER_SECOND: u32 = 24_000;
const AUDIO_CHUNK_FRAMES: usize = 4_800;
const BITS_PER_PIXEL_FRAME: f64 = 0.12;
const MIN_BITRATE: f64 = 4_000_000.0;
const LOWEST_BITRATE: u32 = 100_000;
pub const AUDIO_BITS_PER_SECOND: u32 = AAC_BYTES_PER_SECOND * 8;
const MAX_BITRATE: f64 = 80_000_000.0;
const DEFAULT_FPS: (u32, u32) = (60, 1);
const AUDIO_SHARE: f32 = 0.1;

fn pack(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn unpack(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, (value & 0xffff_ffff) as u32)
}

fn reader(path: &Path) -> Result<IMFSourceReader> {
    let mut attributes: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attributes, 1) }?;
    let attributes = attributes.ok_or(MediaError::NoVideo)?;
    unsafe { attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1) }?;
    Ok(unsafe { MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attributes) }?)
}

fn streams_of(reader: &IMFSourceReader, major: GUID) -> Vec<u32> {
    tracks(reader)
        .into_iter()
        .filter(|(_, media_type)| {
            unsafe { media_type.GetGUID(&MF_MT_MAJOR_TYPE) }.is_ok_and(|value| value == major)
        })
        .map(|(index, _)| index)
        .collect()
}

fn select_only(reader: &IMFSourceReader, stream: u32) -> Result<()> {
    unsafe {
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        reader.SetStreamSelection(stream, true)?;
    }
    Ok(())
}

fn seek(reader: &IMFSourceReader, at: Duration) -> Result<()> {
    let position = PROPVARIANT::from(Span::hns(at));
    unsafe { reader.SetCurrentPosition(&GUID::zeroed(), &position) }?;
    Ok(())
}

pub fn audio_tracks(path: &Path) -> Result<usize> {
    Ok(streams_of(&reader(path)?, MFMediaType_Audio).len())
}

fn read_track(path: &Path, stream: u32, span: Span) -> Result<Vec<f32>> {
    let reader = reader(path)?;
    select_only(&reader, stream)?;
    let wanted = unsafe { MFCreateMediaType() }?;
    unsafe {
        wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        wanted.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
        reader.SetCurrentMediaType(stream, None, &wanted)?;
    }
    let current = unsafe { reader.GetCurrentMediaType(stream) }?;
    let rate = unsafe { current.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND) }?;
    let channels = unsafe { current.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) }?.max(1) as usize;
    seek(&reader, span.start)?;
    let (start, end) = (Span::hns(span.start), Span::hns(span.end));
    let mut samples = Vec::new();
    loop {
        let mut flags = 0u32;
        let mut time = 0i64;
        let mut sample = None;
        unsafe {
            reader.ReadSample(
                stream,
                0,
                None,
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )
        }?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 || time >= end {
            break;
        }
        let Some(sample) = sample else {
            continue;
        };
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }?;
        let mut data = std::ptr::null_mut();
        let mut length = 0u32;
        unsafe { buffer.Lock(&mut data, None, Some(&mut length)) }?;
        let values = unsafe { std::slice::from_raw_parts(data as *const f32, length as usize / 4) };
        let frames = values.len() / channels;
        let frame_at = |index: usize| time + index as i64 * HNS / i64::from(rate);
        let first = (0..frames)
            .find(|index| frame_at(*index) >= start)
            .unwrap_or(frames);
        let last = (first..frames)
            .find(|index| frame_at(*index) >= end)
            .unwrap_or(frames);
        samples.extend_from_slice(&values[first * channels..last * channels]);
        unsafe { buffer.Unlock() }?;
    }
    Ok(resample(to_stereo(samples, channels), rate, RATE))
}

fn prepare_audio(path: &Path, edit: &Edit) -> Result<Vec<Vec<i16>>> {
    let streams = streams_of(&reader(path)?, MFMediaType_Audio);
    if streams.is_empty() {
        return Ok(Vec::new());
    }
    let tracks = streams
        .iter()
        .map(|stream| read_track(path, *stream, edit.span))
        .collect::<Result<Vec<_>>>()?;
    let (origin, mix, mutes) = (edit.span.start, &edit.mix, &edit.mutes);
    let kept = if edit.merge_audio {
        vec![mixdown(&tracks, RATE, origin, mix, mutes)]
    } else {
        separate(&tracks, RATE, origin, mix, mutes)
    };
    drop(tracks);
    let frames = (edit.output_length().as_secs_f64() * f64::from(RATE)) as usize;
    Ok(kept
        .into_iter()
        .map(|track| {
            let mut pcm: Vec<i16> = stretch(track, CHANNELS, RATE, edit.speed)
                .iter()
                .map(|sample| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)
                .collect();
            pcm.resize(frames * CHANNELS, 0);
            pcm
        })
        .collect())
}

struct Video {
    reader: IMFSourceReader,
    stream: u32,
    width: u32,
    height: u32,
    stride: i32,
    fps: (u32, u32),
}

fn open_video(path: &Path) -> Result<Video> {
    let reader = reader(path)?;
    let stream = *streams_of(&reader, MFMediaType_Video)
        .first()
        .ok_or(MediaError::NoVideo)?;
    select_only(&reader, stream)?;
    let native = unsafe { reader.GetNativeMediaType(stream, 0) }?;
    let fps = unsafe { native.GetUINT64(&MF_MT_FRAME_RATE) }
        .map(unpack)
        .ok()
        .filter(|(num, den)| *num > 0 && *den > 0)
        .unwrap_or(DEFAULT_FPS);
    let wanted = unsafe { MFCreateMediaType() }?;
    unsafe {
        wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader.SetCurrentMediaType(stream, None, &wanted)?;
    }
    let current = unsafe { reader.GetCurrentMediaType(stream) }?;
    let (width, height) = unpack(unsafe { current.GetUINT64(&MF_MT_FRAME_SIZE) }?);
    let stride = unsafe { current.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
        .map_or((width * 4) as i32, |stride| stride as i32);
    Ok(Video {
        reader,
        stream,
        width,
        height,
        stride,
        fps,
    })
}

fn video_type(subtype: &GUID, size: (u32, u32), fps: (u32, u32)) -> Result<IMFMediaType> {
    let media_type = unsafe { MFCreateMediaType() }?;
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack(size.0, size.1))?;
        media_type.SetUINT64(&MF_MT_FRAME_RATE, pack(fps.0, fps.1))?;
        media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    }
    Ok(media_type)
}

fn audio_type(subtype: &GUID, bytes_per_second: u32) -> Result<IMFMediaType> {
    let media_type = unsafe { MFCreateMediaType() }?;
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        media_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
        media_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS as u32)?;
        media_type.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bytes_per_second)?;
        if *subtype == MFAudioFormat_PCM {
            media_type.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, (CHANNELS * 2) as u32)?;
        }
    }
    Ok(media_type)
}

fn bitrate(size: (u32, u32), fps: (u32, u32)) -> u32 {
    let rate = f64::from(fps.0) / f64::from(fps.1.max(1));
    (f64::from(size.0) * f64::from(size.1) * rate * BITS_PER_PIXEL_FRAME)
        .clamp(MIN_BITRATE, MAX_BITRATE) as u32
}

struct Output {
    writer: IMFSinkWriter,
    video: u32,
    audio: Vec<u32>,
}

fn open_output(
    destination: &Path,
    size: (u32, u32),
    fps: (u32, u32),
    audio_tracks: usize,
    wanted_bitrate: Option<u32>,
) -> Result<Output> {
    let mut attributes: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attributes, 3) }?;
    let attributes = attributes.ok_or(MediaError::NoVideo)?;
    unsafe {
        attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        attributes.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
        attributes.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
    }
    let writer = unsafe {
        MFCreateSinkWriterFromURL(&HSTRING::from(destination.as_os_str()), None, &attributes)
    }?;
    let encoded = video_type(&MFVideoFormat_H264, size, fps)?;
    unsafe {
        encoded.SetUINT32(
            &MF_MT_AVG_BITRATE,
            wanted_bitrate.map_or_else(|| bitrate(size, fps), |wanted| wanted.max(LOWEST_BITRATE)),
        )?;
        encoded.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
    }
    let video = unsafe { writer.AddStream(&encoded) }?;
    let input = video_type(&MFVideoFormat_RGB32, size, fps)?;
    unsafe {
        input.SetUINT32(&MF_MT_DEFAULT_STRIDE, size.0 * 4)?;
        writer.SetInputMediaType(video, &input, None)?;
    }
    let mut audio = Vec::with_capacity(audio_tracks);
    for _ in 0..audio_tracks {
        let stream =
            unsafe { writer.AddStream(&audio_type(&MFAudioFormat_AAC, AAC_BYTES_PER_SECOND)?) }?;
        let pcm = audio_type(&MFAudioFormat_PCM, RATE * (CHANNELS as u32) * 2)?;
        unsafe { writer.SetInputMediaType(stream, &pcm, None) }?;
        audio.push(stream);
    }
    unsafe { writer.BeginWriting() }?;
    Ok(Output {
        writer,
        video,
        audio,
    })
}

fn write_bytes(
    writer: &IMFSinkWriter,
    stream: u32,
    bytes: &[u8],
    time: i64,
    duration: i64,
) -> Result<()> {
    let buffer = unsafe { MFCreateMemoryBuffer(bytes.len() as u32) }?;
    let mut data = std::ptr::null_mut();
    unsafe { buffer.Lock(&mut data, None, None) }?;
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len()) };
    unsafe {
        buffer.Unlock()?;
        buffer.SetCurrentLength(bytes.len() as u32)?;
    }
    let sample = unsafe { MFCreateSample() }?;
    unsafe {
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(time)?;
        sample.SetSampleDuration(duration)?;
        writer.WriteSample(stream, &sample)?;
    }
    Ok(())
}

struct AudioFeed {
    tracks: Vec<Vec<i16>>,
    written: usize,
}

impl AudioFeed {
    fn frames(&self) -> usize {
        self.tracks.first().map_or(0, |pcm| pcm.len() / CHANNELS)
    }

    fn time_of(frame: usize) -> i64 {
        frame as i64 * HNS / i64::from(RATE)
    }

    fn write_until(&mut self, output: &Output, until: i64) -> Result<()> {
        while self.written < self.frames() && Self::time_of(self.written) <= until {
            let end = (self.written + AUDIO_CHUNK_FRAMES).min(self.frames());
            let time = Self::time_of(self.written);
            for (stream, pcm) in output.audio.iter().zip(&self.tracks) {
                let chunk = &pcm[self.written * CHANNELS..end * CHANNELS];
                let bytes = unsafe {
                    std::slice::from_raw_parts(chunk.as_ptr() as *const u8, chunk.len() * 2)
                };
                write_bytes(
                    &output.writer,
                    *stream,
                    bytes,
                    time,
                    Self::time_of(end) - time,
                )?;
            }
            self.written = end;
        }
        Ok(())
    }
}

fn top_down_into(bytes: &[u8], width: u32, height: u32, stride: i32, pixels: &mut Vec<u8>) {
    let row = (width * 4) as usize;
    let pitch = stride.unsigned_abs() as usize;
    pixels.clear();
    pixels.reserve(row * height as usize);
    for line in 0..height as usize {
        let source = if stride < 0 {
            height as usize - 1 - line
        } else {
            line
        };
        let start = source * pitch;
        match bytes.get(start..start + row) {
            Some(chunk) => pixels.extend_from_slice(chunk),
            None => pixels.resize(pixels.len() + row, 0),
        }
    }
}

fn has_effects(edit: &Edit, at: Duration) -> bool {
    edit.blurs.iter().any(|item| item.window.contains(at))
        || edit.strokes.iter().any(|item| item.window.contains(at))
}

struct StrokeMasks(Vec<Option<Coverage>>);

impl StrokeMasks {
    fn new(edit: &Edit) -> Self {
        Self(edit.strokes.iter().map(|_| None).collect())
    }

    fn release(&mut self) {
        self.0.fill_with(|| None);
    }

    fn paint(&mut self, image: &mut Image, strokes: &[Stroke], at: Duration) {
        for (slot, stroke) in self.0.iter_mut().zip(strokes) {
            if !stroke.window.contains(at) {
                *slot = None;
                continue;
            }
            if slot.is_none() {
                *slot = stroke_mask(stroke, image.width, image.height);
            }
            if let Some(mask) = slot {
                paint_mask(image, mask, stroke.color, 1.0);
            }
        }
    }
}

fn apply(
    pixels: &mut [u8],
    (width, height): (u32, u32),
    edit: &Edit,
    at: Duration,
    masks: &mut StrokeMasks,
) {
    let mut image = Image {
        pixels,
        width,
        height,
    };
    for item in edit.blurs.iter().filter(|item| item.window.contains(at)) {
        blur(&mut image, item.area);
    }
    masks.paint(&mut image, &edit.strokes, at);
}

struct Scratch {
    pixels: Vec<u8>,
    masks: StrokeMasks,
}

fn compose(
    bytes: &[u8],
    video: &Video,
    edit: &Edit,
    at: Duration,
    scratch: &mut Scratch,
    (rect, size): (PixelRect, (u32, u32)),
    out: &mut [u8],
) {
    let direct = Plane {
        bytes,
        pitch: video.stride.unsigned_abs() as usize,
        height: video.height,
        flipped: video.stride < 0,
    };
    if !has_effects(edit, at) && direct.covers(video.width) {
        scratch.masks.release();
        scale_into(&direct, rect, size.0, size.1, out);
        return;
    }
    let pixels = &mut scratch.pixels;
    top_down_into(bytes, video.width, video.height, video.stride, pixels);
    apply(
        pixels,
        (video.width, video.height),
        edit,
        at,
        &mut scratch.masks,
    );
    let plane = Plane {
        bytes: pixels,
        pitch: video.width as usize * 4,
        height: video.height,
        flipped: false,
    };
    scale_into(&plane, rect, size.0, size.1, out);
}

fn write_frame(
    writer: &IMFSinkWriter,
    stream: u32,
    length: usize,
    time: i64,
    duration: i64,
    fill: impl FnOnce(&mut [u8]),
) -> Result<()> {
    let buffer = unsafe { MFCreateMemoryBuffer(length as u32) }?;
    let mut data = std::ptr::null_mut();
    unsafe { buffer.Lock(&mut data, None, None) }?;
    fill(unsafe { std::slice::from_raw_parts_mut(data, length) });
    unsafe {
        buffer.Unlock()?;
        buffer.SetCurrentLength(length as u32)?;
    }
    let sample = unsafe { MFCreateSample() }?;
    unsafe {
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(time)?;
        sample.SetSampleDuration(duration)?;
        writer.WriteSample(stream, &sample)?;
    }
    Ok(())
}

pub fn render(
    source: &Path,
    edit: &Edit,
    destination: &Path,
    progress: &dyn Fn(f32),
) -> Result<()> {
    if edit.span.length().is_zero() {
        return Err(MediaError::EmptyRange);
    }
    progress(0.0);
    let audio = prepare_audio(source, edit)?;
    progress(AUDIO_SHARE);
    let video = open_video(source)?;
    let layout = edit.output_size(video.width, video.height);
    let frame_size = layout.1;
    let output = open_output(
        destination,
        frame_size,
        video.fps,
        audio.len(),
        edit.bitrate,
    )?;
    let mut feed = AudioFeed {
        tracks: audio,
        written: 0,
    };
    let speed = f64::from(edit.speed.max(0.01));
    let frame_hns = (HNS * i64::from(video.fps.1) / i64::from(video.fps.0)).max(1);
    let shown_hns = if speed < 1.0 {
        (frame_hns as f64 / speed) as i64
    } else {
        frame_hns
    };
    let (start, end) = (Span::hns(edit.span.start), Span::hns(edit.span.end));
    let length = (end - start).max(1) as f32;
    seek(&video.reader, edit.span.start)?;
    let mut next_slot = 0i64;
    let frame_bytes = frame_size.0 as usize * frame_size.1 as usize * 4;
    let mut scratch = Scratch {
        pixels: Vec::new(),
        masks: StrokeMasks::new(edit),
    };
    loop {
        let mut flags = 0u32;
        let mut time = 0i64;
        let mut sample = None;
        unsafe {
            video.reader.ReadSample(
                video.stream,
                0,
                None,
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )
        }?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 || time >= end {
            break;
        }
        let Some(sample) = sample else {
            continue;
        };
        if time < start {
            continue;
        }
        let out_time = ((time - start) as f64 / speed) as i64;
        if out_time + frame_hns / 2 < next_slot {
            continue;
        }
        next_slot = out_time + frame_hns;
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }?;
        let mut data = std::ptr::null_mut();
        let mut size = 0u32;
        unsafe { buffer.Lock(&mut data, None, Some(&mut size)) }?;
        let bytes = unsafe { std::slice::from_raw_parts(data, size as usize) };
        let at = Duration::from_nanos(time as u64 * 100);
        let fed = feed.write_until(&output, out_time);
        let written = fed.and_then(|()| {
            write_frame(
                &output.writer,
                output.video,
                frame_bytes,
                out_time,
                shown_hns,
                |out| compose(bytes, &video, edit, at, &mut scratch, layout, out),
            )
        });
        unsafe { buffer.Unlock() }?;
        written?;
        progress(AUDIO_SHARE + (1.0 - AUDIO_SHARE) * ((time - start) as f32 / length));
    }
    feed.write_until(&output, i64::MAX)?;
    unsafe { output.writer.Finalize() }?;
    progress(1.0);
    Ok(())
}

const AREA: &str = "media";

pub fn export(
    source: &Path,
    edit: &Edit,
    destination: &Path,
    progress: &dyn Fn(f32),
) -> Result<()> {
    trace!(
        AREA,
        "exporting {source:?} into {destination:?}, full render {}, edit {edit:?}",
        edit.needs_render()
    );
    if edit.needs_render() {
        render(source, edit, destination, progress).traced(AREA, "render the clip")
    } else {
        trim(source, edit.span, destination)
    }
}

pub fn export_in_place(source: &Path, edit: &Edit, progress: &dyn Fn(f32)) -> Result<()> {
    let temp = temp_path(source);
    let result = export(source, edit, &temp, progress).and_then(|()| {
        Ok(replace(&temp, source).traced(AREA, "replace the clip with its edited copy")?)
    });
    if result.is_err() {
        std::fs::remove_file(&temp).ok();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{Area, Blur, Window};
    use crate::effects::draw;

    const SIZE: (u32, u32) = (96, 64);

    fn noise() -> Vec<u8> {
        let mut seed = 0x9e37_79b9_u32;
        (0..SIZE.0 * SIZE.1 * 4)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect()
    }

    fn window(start: u64, end: u64) -> Window {
        Window {
            start: Duration::from_millis(start),
            end: Duration::from_millis(end),
        }
    }

    fn stroke(points: &[(f32, f32)], color: [u8; 3], window: Window) -> Stroke {
        Stroke {
            points: points.to_vec(),
            width: 0.08,
            color,
            window,
        }
    }

    fn edit() -> Edit {
        let mut edit = Edit::new(Span {
            start: Duration::ZERO,
            end: Duration::from_secs(1),
        });
        edit.blurs.push(Blur {
            area: Area::centered(0.5, 0.5),
            window: window(200, 600),
        });
        edit.strokes = vec![
            stroke(&[(0.1, 0.1), (0.9, 0.8)], [255, 0, 0], window(0, 500)),
            stroke(
                &[(0.2, 0.9), (0.5, 0.3), (0.8, 0.9)],
                [0, 255, 0],
                window(300, 800),
            ),
            stroke(&[(-3.0, -3.0)], [0, 0, 255], window(0, 1000)),
            stroke(&[(0.5, 0.5)], [255, 255, 0], window(700, 1000)),
        ];
        edit
    }

    fn uncached(edit: &Edit, at: Duration) -> Vec<u8> {
        let mut pixels = noise();
        let mut image = Image {
            pixels: &mut pixels,
            width: SIZE.0,
            height: SIZE.1,
        };
        for item in edit.blurs.iter().filter(|item| item.window.contains(at)) {
            blur(&mut image, item.area);
        }
        for stroke in edit.strokes.iter().filter(|item| item.window.contains(at)) {
            draw(&mut image, stroke);
        }
        pixels
    }

    #[test]
    fn cached_stroke_masks_paint_exactly_what_fresh_masks_paint() {
        let edit = edit();
        let mut masks = StrokeMasks::new(&edit);
        let times = [
            0, 50, 100, 250, 300, 350, 499, 500, 650, 700, 799, 800, 950, 400, 0,
        ];
        for at in times.map(Duration::from_millis) {
            let mut pixels = noise();
            apply(&mut pixels, SIZE, &edit, at, &mut masks);
            assert_eq!(pixels, uncached(&edit, at), "{at:?}");
            assert_ne!(pixels, noise(), "{at:?}");
        }
    }

    #[test]
    fn stroke_masks_are_kept_only_while_their_stroke_is_visible() {
        let edit = edit();
        let mut masks = StrokeMasks::new(&edit);
        let held = |masks: &StrokeMasks| masks.0.iter().map(Option::is_some).collect::<Vec<_>>();
        let mut pixels = noise();
        apply(
            &mut pixels,
            SIZE,
            &edit,
            Duration::from_millis(400),
            &mut masks,
        );
        assert_eq!(held(&masks), [true, true, false, false]);
        apply(
            &mut pixels,
            SIZE,
            &edit,
            Duration::from_millis(750),
            &mut masks,
        );
        assert_eq!(held(&masks), [false, true, false, true]);
        masks.release();
        assert_eq!(held(&masks), [false; 4]);
    }
}
