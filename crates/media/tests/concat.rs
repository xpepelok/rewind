#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rewind_media::{MediaError, Runtime, Span, audio_tracks, concat, frames_at, probe, trim};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaType, IMFSinkWriter, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS,
    MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO,
    MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SINK_WRITER_DISABLE_THROTTLING,
    MF_TRANSCODE_CONTAINERTYPE, MFAudioFormat_AAC, MFAudioFormat_PCM, MFCreateAttributes,
    MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL,
    MFCreateSourceReaderFromURL, MFMediaType_Audio, MFMediaType_Video,
    MFTranscodeContainerType_MPEG4, MFVideoFormat_H264, MFVideoFormat_RGB32,
    MFVideoInterlace_Progressive,
};
use windows::core::{GUID, HSTRING};

const HNS: i64 = 10_000_000;
const FPS: u32 = 30;
const RATE: u32 = 48_000;
const CHANNELS: u32 = 2;

fn pack(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn video_type(subtype: &GUID, width: u32, height: u32) -> IMFMediaType {
    unsafe {
        let media_type = MFCreateMediaType().unwrap();
        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .unwrap();
        media_type.SetGUID(&MF_MT_SUBTYPE, subtype).unwrap();
        media_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))
            .unwrap();
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack(FPS, 1))
            .unwrap();
        media_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))
            .unwrap();
        media_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .unwrap();
        media_type
    }
}

fn audio_type(subtype: &GUID, bytes_per_second: u32) -> IMFMediaType {
    unsafe {
        let media_type = MFCreateMediaType().unwrap();
        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .unwrap();
        media_type.SetGUID(&MF_MT_SUBTYPE, subtype).unwrap();
        media_type
            .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .unwrap();
        media_type
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)
            .unwrap();
        media_type
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)
            .unwrap();
        media_type
            .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bytes_per_second)
            .unwrap();
        if *subtype == MFAudioFormat_PCM {
            media_type
                .SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, CHANNELS * 2)
                .unwrap();
        }
        media_type
    }
}

fn write(writer: &IMFSinkWriter, stream: u32, bytes: &[u8], time: i64, duration: i64) {
    unsafe {
        let buffer = MFCreateMemoryBuffer(bytes.len() as u32).unwrap();
        let mut data = std::ptr::null_mut();
        buffer.Lock(&mut data, None, None).unwrap();
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
        buffer.Unlock().unwrap();
        buffer.SetCurrentLength(bytes.len() as u32).unwrap();
        let sample = MFCreateSample().unwrap();
        sample.AddBuffer(&buffer).unwrap();
        sample.SetSampleTime(time).unwrap();
        sample.SetSampleDuration(duration).unwrap();
        writer.WriteSample(stream, &sample).unwrap();
    }
}

fn make_clip(path: &Path, width: u32, height: u32, frames: u32, audio: &[u32]) {
    unsafe {
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 3).unwrap();
        let attributes = attributes.unwrap();
        attributes
            .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
            .unwrap();
        attributes
            .SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)
            .unwrap();
        attributes
            .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
            .unwrap();
        let writer =
            MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attributes).unwrap();
        let encoded = video_type(&MFVideoFormat_H264, width, height);
        encoded.SetUINT32(&MF_MT_AVG_BITRATE, 2_000_000).unwrap();
        let video = writer.AddStream(&encoded).unwrap();
        let input = video_type(&MFVideoFormat_RGB32, width, height);
        input.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4).unwrap();
        writer.SetInputMediaType(video, &input, None).unwrap();
        let audio: Vec<u32> = audio
            .iter()
            .map(|rate| {
                let track = writer
                    .AddStream(&audio_type(&MFAudioFormat_AAC, *rate))
                    .unwrap();
                writer
                    .SetInputMediaType(
                        track,
                        &audio_type(&MFAudioFormat_PCM, RATE * CHANNELS * 2),
                        None,
                    )
                    .unwrap();
                track
            })
            .collect();
        writer.BeginWriting().unwrap();

        let frame_hns = HNS / i64::from(FPS);
        let samples_per_frame = (RATE / FPS) as usize;
        for index in 0..frames {
            let shade = (index * 255 / frames.max(1)) as u8;
            let pixels: Vec<u8> = (0..width * height)
                .flat_map(|_| [shade, 255 - shade, 128, 255])
                .collect();
            let time = i64::from(index) * frame_hns;
            write(&writer, video, &pixels, time, frame_hns);
            let pcm: Vec<u8> = (0..samples_per_frame * CHANNELS as usize)
                .flat_map(|sample| {
                    let value = ((sample as f32 * 0.05).sin() * 8_000.0) as i16;
                    value.to_le_bytes()
                })
                .collect();
            for track in &audio {
                write(&writer, *track, &pcm, time, frame_hns);
            }
        }
        writer.Finalize().unwrap();
    }
}

#[test]
#[ignore = "needs media foundation encoder"]
fn joins_clips_without_reencoding() {
    let _runtime = Runtime::enter().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first.mp4");
    let second = temp.path().join("second.mp4");
    let other = temp.path().join("other.mp4");
    make_clip(&first, 640, 360, 30, &[24_000]);
    make_clip(&second, 640, 360, 45, &[24_000]);
    make_clip(&other, 320, 240, 15, &[24_000]);
    let sources: Vec<PathBuf> = vec![first.clone(), second.clone()];
    let expected = probe(&first).unwrap().duration + probe(&second).unwrap().duration;

    let joined = temp.path().join("joined.mp4");
    let progress = Mutex::new(Vec::new());
    concat(&sources, &joined, &|value| {
        progress.lock().unwrap().push(value)
    })
    .unwrap();

    let info = probe(&joined).unwrap();
    assert_eq!((info.width, info.height), (640, 360));
    let gap = info.duration.abs_diff(expected);
    assert!(
        gap <= Duration::from_millis(150),
        "{info:?} vs {expected:?}"
    );
    assert_eq!(audio_tracks(&joined).unwrap(), 1);
    let frames = frames_at(
        &joined,
        &[Duration::from_millis(500), Duration::from_millis(2000)],
        Some((64, 36)),
    )
    .unwrap();
    assert_eq!(frames.len(), 2);
    let progress = progress.into_inner().unwrap();
    assert_eq!(progress.first(), Some(&0.0));
    assert_eq!(progress.last(), Some(&1.0));
    assert!(progress.windows(2).all(|pair| pair[0] <= pair[1]));

    let mismatch = concat(&[first, other], &temp.path().join("bad.mp4"), &|_| {});
    match mismatch {
        Err(MediaError::Incompatible(message)) => assert_eq!(
            message,
            "Клипы записаны с разными настройками: разрешение 640×360 и 320×240"
        ),
        other => panic!("{other:?}"),
    }
    assert!(!temp.path().join("bad.mp4").exists());
}

fn audio_rates(path: &Path) -> Vec<u32> {
    let reader =
        unsafe { MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None) }.unwrap();
    (0..)
        .map_while(|index| unsafe { reader.GetNativeMediaType(index, 0) }.ok())
        .filter(|native| unsafe { native.GetGUID(&MF_MT_MAJOR_TYPE) }.unwrap() == MFMediaType_Audio)
        .map(|native| unsafe { native.GetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND) }.unwrap())
        .collect()
}

#[test]
#[ignore = "needs media foundation encoder"]
fn trimming_and_joining_keep_the_audio_tracks_in_their_order() {
    let _runtime = Runtime::enter().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.mp4");
    make_clip(&source, 640, 360, 60, &[12_000, 16_000, 24_000]);
    let rates = audio_rates(&source);
    assert_eq!(rates.len(), 3);
    assert_ne!(rates[0], rates[2]);

    let trimmed = temp.path().join("trimmed.mp4");
    let span = Span {
        start: Duration::ZERO,
        end: Duration::from_secs(1),
    };
    trim(&source, span, &trimmed).unwrap();
    assert_eq!(audio_rates(&trimmed), rates);

    let joined = temp.path().join("joined.mp4");
    concat(&[source, trimmed], &joined, &|_| {}).unwrap();
    assert_eq!(audio_rates(&joined), rates);
}
