use std::path::{Path, PathBuf};

use windows::Win32::Media::MediaFoundation::{
    IMFMediaType, IMFSinkWriter, IMFSourceReader, MF_MT_AUDIO_NUM_CHANNELS,
    MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_MPEG_SEQUENCE_HEADER,
    MF_MT_SUBTYPE, MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_ANY_STREAM,
    MF_SOURCE_READERF_ENDOFSTREAM, MFAudioFormat_AAC, MFAudioFormat_Dolby_AC3, MFAudioFormat_Float,
    MFAudioFormat_MP3, MFAudioFormat_PCM, MFCreateSinkWriterFromURL, MFMediaType_Audio,
    MFMediaType_Video, MFSampleExtension_CleanPoint, MFVideoFormat_AV1, MFVideoFormat_H264,
    MFVideoFormat_HEVC,
};
use windows::core::{GUID, HSTRING};

use crate::error::{MediaError, Result};
use crate::probe::{duration, open_reader};
use crate::span::Span;
use crate::trim::{shift, writer_attributes};

#[derive(Clone, Debug, PartialEq, Eq)]
struct VideoFormat {
    codec: GUID,
    width: u32,
    height: u32,
    header: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AudioFormat {
    codec: GUID,
    channels: u32,
    rate: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Layout {
    video: Vec<VideoFormat>,
    audio: Vec<AudioFormat>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Video,
    Audio,
}

struct Stream {
    index: u32,
    kind: Kind,
    ordinal: usize,
    native: IMFMediaType,
}

struct Clip {
    name: String,
    reader: IMFSourceReader,
    streams: Vec<Stream>,
    layout: Layout,
    duration: i64,
}

struct Outputs {
    video: Vec<u32>,
    audio: Vec<u32>,
}

impl Outputs {
    fn of(&self, stream: &Stream) -> Option<u32> {
        match stream.kind {
            Kind::Video => self.video.get(stream.ordinal).copied(),
            Kind::Audio => self.audio.get(stream.ordinal).copied(),
        }
    }
}

fn codec_name(codec: &GUID) -> String {
    let known = [
        (MFVideoFormat_H264, "H.264"),
        (MFVideoFormat_HEVC, "HEVC"),
        (MFVideoFormat_AV1, "AV1"),
        (MFAudioFormat_AAC, "AAC"),
        (MFAudioFormat_MP3, "MP3"),
        (MFAudioFormat_Dolby_AC3, "AC-3"),
        (MFAudioFormat_PCM, "PCM"),
        (MFAudioFormat_Float, "PCM float"),
    ];
    known
        .iter()
        .find(|(guid, _)| guid == codec)
        .map_or_else(|| format!("{codec:?}"), |(_, name)| (*name).to_owned())
}

fn video_difference(first: &VideoFormat, other: &VideoFormat) -> Option<String> {
    if first.codec != other.codec {
        return Some(format!(
            "видеокодек {} и {}",
            codec_name(&first.codec),
            codec_name(&other.codec)
        ));
    }
    if (first.width, first.height) != (other.width, other.height) {
        return Some(format!(
            "разрешение {}×{} и {}×{}",
            first.width, first.height, other.width, other.height
        ));
    }
    if first.header != other.header {
        return Some("параметры кодирования видео".to_owned());
    }
    None
}

fn audio_difference(first: &AudioFormat, other: &AudioFormat) -> Option<String> {
    if first.codec != other.codec {
        return Some(format!(
            "аудиокодек {} и {}",
            codec_name(&first.codec),
            codec_name(&other.codec)
        ));
    }
    if first.rate != other.rate {
        return Some(format!(
            "частота звука {} Гц и {} Гц",
            first.rate, other.rate
        ));
    }
    if first.channels != other.channels {
        return Some(format!(
            "число каналов звука {} и {}",
            first.channels, other.channels
        ));
    }
    None
}

fn layout_difference(first: &Layout, other: &Layout) -> Option<String> {
    if first.video.len() != other.video.len() {
        return Some(format!(
            "количество видеодорожек {} и {}",
            first.video.len(),
            other.video.len()
        ));
    }
    if let Some(difference) = first
        .video
        .iter()
        .zip(&other.video)
        .find_map(|(a, b)| video_difference(a, b))
    {
        return Some(difference);
    }
    if first.audio.len() != other.audio.len() {
        return Some(format!(
            "количество звуковых дорожек {} и {}",
            first.audio.len(),
            other.audio.len()
        ));
    }
    first
        .audio
        .iter()
        .zip(&other.audio)
        .find_map(|(a, b)| audio_difference(a, b))
}

fn incompatibility(first: &Layout, other: &Layout) -> Option<MediaError> {
    layout_difference(first, other).map(|difference| {
        MediaError::Incompatible(format!(
            "Клипы записаны с разными настройками: {difference}"
        ))
    })
}

fn fraction(done: i64, total: i64) -> f32 {
    if total <= 0 {
        return 0.0;
    }
    (done as f64 / total as f64).clamp(0.0, 1.0) as f32
}

fn header_of(native: &IMFMediaType) -> Result<Vec<u8>> {
    let Ok(size) = (unsafe { native.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) }) else {
        return Ok(Vec::new());
    };
    let mut header = vec![0u8; size as usize];
    unsafe { native.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut header, None) }?;
    Ok(header)
}

fn video_format(native: &IMFMediaType) -> Result<VideoFormat> {
    let size = unsafe { native.GetUINT64(&MF_MT_FRAME_SIZE) }.unwrap_or(0);
    Ok(VideoFormat {
        codec: unsafe { native.GetGUID(&MF_MT_SUBTYPE) }?,
        width: (size >> 32) as u32,
        height: (size & 0xffff_ffff) as u32,
        header: header_of(native)?,
    })
}

fn audio_format(native: &IMFMediaType) -> Result<AudioFormat> {
    Ok(AudioFormat {
        codec: unsafe { native.GetGUID(&MF_MT_SUBTYPE) }?,
        channels: unsafe { native.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) }.unwrap_or(0),
        rate: unsafe { native.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND) }.unwrap_or(0),
    })
}

fn inspect(path: &Path) -> Result<Clip> {
    let reader = open_reader(path)?;
    unsafe { reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false) }?;
    let mut streams = Vec::new();
    let mut layout = Layout::default();
    for index in 0.. {
        let Ok(native) = (unsafe { reader.GetNativeMediaType(index, 0) }) else {
            break;
        };
        let Ok(major) = (unsafe { native.GetGUID(&MF_MT_MAJOR_TYPE) }) else {
            continue;
        };
        let (kind, ordinal) = if major == MFMediaType_Video {
            layout.video.push(video_format(&native)?);
            (Kind::Video, layout.video.len() - 1)
        } else if major == MFMediaType_Audio {
            layout.audio.push(audio_format(&native)?);
            (Kind::Audio, layout.audio.len() - 1)
        } else {
            continue;
        };
        unsafe { reader.SetStreamSelection(index, true) }?;
        streams.push(Stream {
            index,
            kind,
            ordinal,
            native,
        });
    }
    if layout.video.is_empty() {
        return Err(MediaError::NoVideo);
    }
    Ok(Clip {
        name: path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        duration: Span::hns(duration(&reader)?),
        reader,
        streams,
        layout,
    })
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn open_writer(destination: &Path, first: &Clip) -> Result<(IMFSinkWriter, Outputs)> {
    let writer = unsafe {
        MFCreateSinkWriterFromURL(
            &HSTRING::from(destination.as_os_str()),
            None,
            &writer_attributes()?,
        )
    }?;
    let mut outputs = Outputs {
        video: Vec::new(),
        audio: Vec::new(),
    };
    for stream in &first.streams {
        let output = unsafe { writer.AddStream(&stream.native) }?;
        unsafe { writer.SetInputMediaType(output, &stream.native, None) }?;
        match stream.kind {
            Kind::Video => outputs.video.push(output),
            Kind::Audio => outputs.audio.push(output),
        }
    }
    Ok((writer, outputs))
}

struct Progress<'a> {
    report: &'a dyn Fn(f32),
    total: i64,
    last: f32,
}

impl Progress<'_> {
    fn advance(&mut self, done: i64) {
        let value = fraction(done, self.total);
        if value > self.last {
            self.last = value;
            (self.report)(value);
        }
    }
}

fn append(
    clip: &Clip,
    writer: &IMFSinkWriter,
    outputs: &Outputs,
    offset: i64,
    needs_keyframe: bool,
    progress: &mut Progress,
) -> Result<i64> {
    let mut done = vec![false; clip.streams.len()];
    let mut checked = !needs_keyframe;
    let mut end = offset + clip.duration;
    while done.iter().any(|finished| !finished) {
        let mut index = 0u32;
        let mut flags = 0u32;
        let mut time = 0i64;
        let mut sample = None;
        unsafe {
            clip.reader.ReadSample(
                MF_SOURCE_READER_ANY_STREAM.0 as u32,
                0,
                Some(&mut index),
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )
        }?;
        let Some(slot) = clip.streams.iter().position(|stream| stream.index == index) else {
            break;
        };
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            done[slot] = true;
        }
        let Some(sample) = sample else {
            continue;
        };
        let stream = &clip.streams[slot];
        if stream.kind == Kind::Video && !checked {
            let keyframe =
                unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }.unwrap_or(0) != 0;
            if !keyframe {
                return Err(MediaError::Incompatible(format!(
                    "Клип {} начинается не с ключевого кадра",
                    clip.name
                )));
            }
            checked = true;
        }
        let Some(output) = outputs.of(stream) else {
            continue;
        };
        let shifted = offset + time;
        let length = unsafe { sample.GetSampleDuration() }.unwrap_or(0);
        shift(&sample, time, offset)?;
        unsafe { writer.WriteSample(output, &sample) }?;
        end = end.max(shifted + length);
        progress.advance(shifted + length);
    }
    Ok(end)
}

fn write(clips: Vec<Clip>, destination: &Path, progress: &mut Progress) -> Result<()> {
    let Some(first) = clips.first() else {
        return Err(MediaError::TooFewClips);
    };
    let (writer, outputs) = open_writer(destination, first)?;
    unsafe { writer.BeginWriting() }?;
    let mut offset = 0i64;
    for (position, clip) in clips.into_iter().enumerate() {
        offset = append(&clip, &writer, &outputs, offset, position > 0, progress)?;
    }
    unsafe { writer.Finalize() }?;
    Ok(())
}

pub fn concat(sources: &[PathBuf], destination: &Path, progress: &dyn Fn(f32)) -> Result<()> {
    if sources.len() < 2 {
        return Err(MediaError::TooFewClips);
    }
    if sources.iter().any(|source| same_file(source, destination)) {
        return Err(MediaError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Нельзя сохранить склейку поверх исходного клипа",
        )));
    }
    let clips = sources
        .iter()
        .map(|source| inspect(source))
        .collect::<Result<Vec<_>>>()?;
    if let Some(error) = clips
        .iter()
        .skip(1)
        .find_map(|clip| incompatibility(&clips[0].layout, &clip.layout))
    {
        return Err(error);
    }
    progress(0.0);
    let mut tracker = Progress {
        report: progress,
        total: clips.iter().map(|clip| clip.duration).sum(),
        last: 0.0,
    };
    let result = write(clips, destination, &mut tracker);
    if result.is_err() {
        std::fs::remove_file(destination).ok();
        return result;
    }
    if tracker.last < 1.0 {
        progress(1.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(width: u32, height: u32) -> VideoFormat {
        VideoFormat {
            codec: MFVideoFormat_H264,
            width,
            height,
            header: vec![0, 0, 0, 1, 0x67],
        }
    }

    fn stereo() -> AudioFormat {
        AudioFormat {
            codec: MFAudioFormat_AAC,
            channels: 2,
            rate: 48_000,
        }
    }

    fn layout(video: VideoFormat, audio: Vec<AudioFormat>) -> Layout {
        Layout {
            video: vec![video],
            audio,
        }
    }

    fn message(first: &Layout, other: &Layout) -> Option<String> {
        incompatibility(first, other).map(|error| error.to_string())
    }

    #[test]
    fn identical_layouts_are_compatible() {
        let first = layout(video(1920, 1080), vec![stereo(), stereo()]);
        assert_eq!(message(&first, &first.clone()), None);
    }

    #[test]
    fn resolution_mismatch_is_named_in_russian() {
        let first = layout(video(1920, 1080), vec![stereo()]);
        let other = layout(video(2560, 1440), vec![stereo()]);
        assert_eq!(
            message(&first, &other).as_deref(),
            Some("Клипы записаны с разными настройками: разрешение 1920×1080 и 2560×1440")
        );
    }

    #[test]
    fn codec_mismatch_wins_over_resolution() {
        let first = layout(video(1920, 1080), vec![]);
        let other = layout(
            VideoFormat {
                codec: MFVideoFormat_HEVC,
                ..video(1280, 720)
            },
            vec![],
        );
        assert_eq!(
            message(&first, &other).as_deref(),
            Some("Клипы записаны с разными настройками: видеокодек H.264 и HEVC")
        );
    }

    #[test]
    fn audio_track_count_must_match() {
        let first = layout(video(1920, 1080), vec![stereo(), stereo()]);
        let other = layout(video(1920, 1080), vec![stereo()]);
        assert_eq!(
            message(&first, &other).as_deref(),
            Some("Клипы записаны с разными настройками: количество звуковых дорожек 2 и 1")
        );
    }

    #[test]
    fn audio_format_must_match() {
        let first = layout(video(1920, 1080), vec![stereo()]);
        let rate = layout(
            video(1920, 1080),
            vec![AudioFormat {
                rate: 44_100,
                ..stereo()
            }],
        );
        let mono = layout(
            video(1920, 1080),
            vec![AudioFormat {
                channels: 1,
                ..stereo()
            }],
        );
        assert_eq!(
            message(&first, &rate).as_deref(),
            Some("Клипы записаны с разными настройками: частота звука 48000 Гц и 44100 Гц")
        );
        assert_eq!(
            message(&first, &mono).as_deref(),
            Some("Клипы записаны с разными настройками: число каналов звука 2 и 1")
        );
    }

    #[test]
    fn sequence_header_must_match() {
        let first = layout(video(1920, 1080), vec![]);
        let other = layout(
            VideoFormat {
                header: vec![0, 0, 0, 1, 0x68],
                ..video(1920, 1080)
            },
            vec![],
        );
        assert_eq!(
            message(&first, &other).as_deref(),
            Some("Клипы записаны с разными настройками: параметры кодирования видео")
        );
    }

    #[test]
    fn unknown_codecs_fall_back_to_guid() {
        assert_eq!(codec_name(&MFAudioFormat_AAC), "AAC");
        assert!(!codec_name(&GUID::zeroed()).is_empty());
    }

    #[test]
    fn fraction_is_clamped() {
        assert_eq!(fraction(5, 10), 0.5);
        assert_eq!(fraction(20, 10), 1.0);
        assert_eq!(fraction(-1, 10), 0.0);
        assert_eq!(fraction(1, 0), 0.0);
    }

    #[test]
    fn progress_only_moves_forward() {
        let seen = std::cell::RefCell::new(Vec::new());
        let report = |value: f32| seen.borrow_mut().push(value);
        let mut progress = Progress {
            report: &report,
            total: 100,
            last: 0.0,
        };
        for done in [10, 5, 50, 50, 120] {
            progress.advance(done);
        }
        assert_eq!(*seen.borrow(), vec![0.1, 0.5, 1.0]);
    }

    #[test]
    fn too_few_sources_are_rejected() {
        let error = concat(&[PathBuf::from("a.mp4")], Path::new("out.mp4"), &|_| {});
        assert!(matches!(error, Err(MediaError::TooFewClips)));
    }
}
