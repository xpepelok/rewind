use std::path::Path;
use std::sync::mpsc;
use std::thread;

use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaType, IMFSample, IMFSinkWriter,
    MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, MF_MT_AAC_PAYLOAD_TYPE,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_NUM_CHANNELS,
    MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_PIXEL_ASPECT_RATIO,
    MF_MT_SUBTYPE, MF_MT_USER_DATA, MF_SINK_WRITER_DISABLE_THROTTLING, MF_TRANSCODE_CONTAINERTYPE,
    MFAudioFormat_AAC, MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFMediaType_Audio, MFMediaType_Video, MFSampleExtension_CleanPoint,
    MFTranscodeContainerType_MPEG4, MFVideoInterlace_Progressive,
};
use windows::core::HSTRING;

use crate::audio::AAC_PROFILE_LEVEL;
use crate::com::Runtime;
use crate::encoder::{pack, subtype};
use crate::error::{CaptureError, Result};
use crate::format::{AudioFormat, Packet, Track, VideoFormat};
use crate::ring::{Clip, Frame};

const PREFETCH_PACKETS: usize = 120;

fn video_type(format: &VideoFormat) -> Result<IMFMediaType> {
    let media_type = unsafe { MFCreateMediaType() }?;
    let subtype = subtype(format.codec);
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
        media_type.SetUINT32(&MF_MT_AVG_BITRATE, format.bitrate)?;
        media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack(format.width, format.height))?;
        media_type.SetUINT64(&MF_MT_FRAME_RATE, pack(format.fps, 1))?;
        media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media_type.SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &format.sequence_header)?;
    }
    Ok(media_type)
}

fn audio_type(format: &AudioFormat) -> Result<IMFMediaType> {
    let media_type = unsafe { MFCreateMediaType() }?;
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        media_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, format.sample_rate)?;
        media_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, format.channels)?;
        media_type.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, format.bytes_per_second)?;
        media_type.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        media_type.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, AAC_PROFILE_LEVEL)?;
        if !format.user_data.is_empty() {
            media_type.SetBlob(&MF_MT_USER_DATA, &format.user_data)?;
        }
    }
    Ok(media_type)
}

struct Layout {
    video: u32,
    audio: Vec<u32>,
    origin: i64,
}

impl Layout {
    fn stream(&self, track: Track) -> Option<u32> {
        match track {
            Track::Video => Some(self.video),
            Track::Audio(index) => self.audio.get(index).copied(),
        }
    }
}

pub struct Mp4Writer {
    writer: IMFSinkWriter,
    layout: Layout,
}

struct Prepared {
    stream: u32,
    sample: IMFSample,
}

unsafe impl Send for Prepared {}

fn sample(
    time: i64,
    duration: i64,
    keyframe: bool,
    length: usize,
    fill: impl FnOnce(&mut [u8]) -> Result<()>,
) -> Result<IMFSample> {
    let size = u32::try_from(length).map_err(|_| CaptureError::Empty)?;
    let buffer = unsafe { MFCreateMemoryBuffer(size) }?;
    let mut data = std::ptr::null_mut();
    unsafe { buffer.Lock(&mut data, None, None) }?;
    let filled = match (data.is_null(), length) {
        (_, 0) => fill(&mut []),
        (true, _) => Err(CaptureError::Empty),
        (false, _) => fill(unsafe { std::slice::from_raw_parts_mut(data, length) }),
    };
    unsafe { buffer.Unlock() }?;
    filled?;
    unsafe { buffer.SetCurrentLength(size) }?;
    let sample = unsafe { MFCreateSample() }?;
    unsafe {
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(time)?;
        sample.SetSampleDuration(duration)?;
        if keyframe {
            sample.SetUINT32(&MFSampleExtension_CleanPoint, 1)?;
        }
    }
    Ok(sample)
}

impl Mp4Writer {
    pub fn create(
        path: &Path,
        video: &VideoFormat,
        audio: &[AudioFormat],
        origin: i64,
    ) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut attributes: Option<IMFAttributes> = None;
        unsafe { MFCreateAttributes(&mut attributes, 2) }?;
        let attributes = attributes.ok_or(CaptureError::NoFormat)?;
        unsafe {
            attributes.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
            attributes.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
        }
        let writer = unsafe {
            MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attributes)
        }?;
        let video_type = video_type(video)?;
        let video_index = unsafe { writer.AddStream(&video_type) }?;
        unsafe { writer.SetInputMediaType(video_index, &video_type, None) }?;
        let mut audio_indices = Vec::with_capacity(audio.len());
        for format in audio {
            let audio_type = audio_type(format)?;
            let index = unsafe { writer.AddStream(&audio_type) }?;
            unsafe { writer.SetInputMediaType(index, &audio_type, None) }?;
            audio_indices.push(index);
        }
        unsafe { writer.BeginWriting() }?;
        Ok(Self {
            writer,
            layout: Layout {
                video: video_index,
                audio: audio_indices,
                origin,
            },
        })
    }

    pub fn write(&mut self, packet: &Packet) -> Result<()> {
        let Some(stream) = self.layout.stream(packet.track) else {
            return Ok(());
        };
        if packet.pts < self.layout.origin {
            return Ok(());
        }
        let sample = sample(
            packet.pts - self.layout.origin,
            packet.duration,
            packet.keyframe,
            packet.data.len(),
            |into| {
                into.copy_from_slice(&packet.data);
                Ok(())
            },
        )?;
        unsafe { self.writer.WriteSample(stream, &sample) }?;
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        unsafe { self.writer.Finalize() }?;
        Ok(())
    }
}

fn prepare(
    clip: &Clip,
    layout: &Layout,
    track: Track,
    frame: &Frame,
    duration: i64,
) -> Result<Option<Prepared>> {
    let Some(stream) = layout.stream(track) else {
        return Ok(None);
    };
    if frame.pts < layout.origin {
        return Ok(None);
    }
    let sample = sample(
        frame.pts - layout.origin,
        duration,
        frame.keyframe,
        frame.size(),
        |into| clip.fill(frame, into),
    )?;
    Ok(Some(Prepared { stream, sample }))
}

fn produce(clip: &Clip, layout: &Layout, mut emit: impl FnMut(Result<Prepared>) -> bool) {
    let mut send = |track: Track, frame: &Frame, duration: i64| match prepare(
        clip, layout, track, frame, duration,
    ) {
        Ok(Some(prepared)) => emit(Ok(prepared)),
        Ok(None) => true,
        Err(error) => {
            emit(Err(error));
            false
        }
    };
    let mut cursors = vec![0usize; clip.audio.len()];
    for (position, frame) in clip.video.iter().enumerate() {
        for (index, (track, cursor)) in clip.audio.iter().zip(cursors.iter_mut()).enumerate() {
            while let Some(sound) = track.get(*cursor).filter(|sound| sound.pts <= frame.pts) {
                if !send(Track::Audio(index), sound, sound.duration) {
                    return;
                }
                *cursor += 1;
            }
        }
        let duration = match clip.video.get(position + 1) {
            Some(next) if next.pts > frame.pts => next.pts - frame.pts,
            _ => frame.duration,
        };
        if !send(Track::Video, frame, duration) {
            return;
        }
    }
    let last = clip
        .video
        .last()
        .map_or(i64::MIN, |frame| frame.pts + frame.duration);
    for (index, (track, cursor)) in clip.audio.iter().zip(cursors).enumerate() {
        for sound in track[cursor..].iter().take_while(|sound| sound.pts <= last) {
            if !send(Track::Audio(index), sound, sound.duration) {
                return;
            }
        }
    }
}

pub fn write_clip(
    path: &Path,
    video: &VideoFormat,
    audio: &[AudioFormat],
    clip: &Clip,
) -> Result<()> {
    let writer = Mp4Writer::create(path, video, audio, clip.start)?;
    let (sender, samples) = mpsc::sync_channel::<Result<Prepared>>(PREFETCH_PACKETS);
    let layout = &writer.layout;
    thread::scope(|scope| {
        thread::Builder::new()
            .name("rewind-prefetch".to_owned())
            .spawn_scoped(scope, move || match Runtime::enter() {
                Ok(_runtime) => produce(clip, layout, |item| sender.send(item).is_ok()),
                Err(error) => {
                    sender.send(Err(error)).ok();
                }
            })?;
        for item in samples {
            let Prepared { stream, sample } = item?;
            unsafe { writer.writer.WriteSample(stream, &sample) }?;
        }
        Ok::<(), CaptureError>(())
    })?;
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::HNS_PER_SECOND;
    use crate::ring::Ring;
    use std::sync::Arc;
    use std::time::Duration;

    const FRAME: i64 = HNS_PER_SECOND / 10;

    fn packet(track: Track, pts: i64, keyframe: bool, byte: u8) -> Packet {
        Packet {
            track,
            pts,
            duration: FRAME,
            keyframe,
            data: Arc::from(vec![byte; 16]),
        }
    }

    #[test]
    fn samples_come_out_interleaved_with_stretched_video_frames() {
        let _runtime = Runtime::enter().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut ring = Ring::new(Duration::from_secs(60), 1 << 20, 2, Some(directory.path()));
        for (index, pts) in [0, FRAME, 3 * FRAME, 4 * FRAME].into_iter().enumerate() {
            ring.push(packet(Track::Video, pts, index == 0, index as u8));
        }
        for step in 0..6 {
            ring.push(packet(Track::Audio(0), step * FRAME, true, 100));
        }
        ring.push(packet(Track::Audio(1), 2 * FRAME, true, 200));
        let clip = ring.snapshot_range(0, 3 * FRAME).unwrap();
        let layout = Layout {
            video: 7,
            audio: vec![8],
            origin: clip.start,
        };
        let mut written = Vec::new();
        produce(&clip, &layout, |item| {
            let Prepared { stream, sample } = item.unwrap();
            let time = unsafe { sample.GetSampleTime() }.unwrap();
            let duration = unsafe { sample.GetSampleDuration() }.unwrap();
            let buffer = unsafe { sample.ConvertToContiguousBuffer() }.unwrap();
            let length = unsafe { buffer.GetCurrentLength() }.unwrap();
            let mut data = std::ptr::null_mut();
            unsafe { buffer.Lock(&mut data, None, None) }.unwrap();
            let first = unsafe { *data };
            unsafe { buffer.Unlock() }.unwrap();
            written.push((stream, time / FRAME, duration / FRAME, length, first));
            true
        });
        assert_eq!(
            written,
            vec![
                (8, 0, 1, 16, 100),
                (7, 0, 1, 16, 0),
                (8, 1, 1, 16, 100),
                (7, 1, 2, 16, 1),
                (8, 2, 1, 16, 100),
                (8, 3, 1, 16, 100),
                (7, 3, 1, 16, 2),
            ]
        );
    }

    #[test]
    fn an_overwritten_frame_stops_the_stream_with_its_error() {
        let _runtime = Runtime::enter().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut ring = Ring::new(Duration::from_secs(60), 200, 0, Some(directory.path()));
        ring.push(packet(Track::Video, 0, true, 1));
        ring.push(packet(Track::Video, FRAME, false, 2));
        let clip = ring.snapshot(Duration::from_secs(1)).unwrap();
        for index in 2..30 {
            ring.push(packet(Track::Video, index * FRAME, true, 3));
        }
        let layout = Layout {
            video: 0,
            audio: Vec::new(),
            origin: clip.start,
        };
        let mut items = Vec::new();
        produce(&clip, &layout, |item| {
            items.push(item.is_ok());
            true
        });
        assert_eq!(items, vec![false]);
    }
}
