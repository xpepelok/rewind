#![cfg(windows)]

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use rewind_media::{
    Area, Blur, Edit, Mix, Player, Runtime, Span, Stroke, Window, audio_tracks, export, frames_at,
    probe, thumbnail, trim, trim_in_place,
};

const SAMPLES: &str = "REWIND_SAMPLES";

fn samples() -> Vec<PathBuf> {
    let directory =
        std::env::var_os(SAMPLES).expect("set REWIND_SAMPLES to a folder of mp4 clips");
    std::fs::read_dir(directory)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "mp4"))
        .collect()
}

#[test]
#[ignore = "needs real clips"]
fn trims_every_sample_losslessly() {
    let _runtime = Runtime::enter().unwrap();
    let temp = tempfile::tempdir().unwrap();
    for sample in samples() {
        let info = probe(&sample).unwrap();
        let span = Span {
            start: Duration::from_millis(1000),
            end: Duration::from_millis(2500),
        };
        let destination = temp.path().join(sample.file_name().unwrap());
        trim(&sample, span, &destination).unwrap();
        let trimmed = probe(&destination).unwrap();
        assert_eq!(trimmed.codec, info.codec);
        assert_eq!((trimmed.width, trimmed.height), (info.width, info.height));
        assert!(
            trimmed.duration >= Duration::from_millis(1400),
            "{trimmed:?}"
        );
        assert!(
            trimmed.duration <= Duration::from_millis(2600),
            "{trimmed:?}"
        );

        let copy = temp.path().join("in-place.mp4");
        std::fs::copy(&sample, &copy).unwrap();
        trim_in_place(&copy, span).unwrap();
        assert!(probe(&copy).unwrap().duration < info.duration);

        let strip = frames_at(
            &sample,
            &[Duration::ZERO, Duration::from_secs(1)],
            Some((160, 90)),
        )
        .unwrap();
        assert_eq!(strip.len(), 2);
        assert!(
            strip
                .iter()
                .all(|frame| frame.width == 160 && frame.pixels.len() == 160 * 90 * 4)
        );

        let preview = temp.path().join("preview.png");
        thumbnail(&sample, Duration::from_millis(500), 320, &preview).unwrap();
        assert!(std::fs::metadata(&preview).unwrap().len() > 1000);
    }
}

#[test]
#[ignore = "needs real clips and a gpu"]
fn player_delivers_frames() {
    let sample = samples().into_iter().next().unwrap();
    let (sender, frames) = mpsc::channel();
    let player = Player::open(&sample, (640, 640), move |frame| {
        sender
            .send((frame.width, frame.height, frame.pixels.len()))
            .ok();
    });
    let (width, height, length) = frames.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(length as u32, width * height * 4);
    assert!(width <= 640 && height <= 640);
    player.play();
    std::thread::sleep(Duration::from_millis(800));
    let state = player.state();
    assert!(state.ready && state.duration > 1.0, "{state:?}");
    assert!(state.position > 0.2, "{state:?}");
    player.seek(0.5);
    std::thread::sleep(Duration::from_millis(300));
    assert!(frames.try_iter().count() > 5);
}

#[test]
#[ignore = "needs real clips"]
fn renders_every_effect_into_a_playable_clip() {
    let _runtime = Runtime::enter().unwrap();
    let temp = tempfile::tempdir().unwrap();
    for sample in samples() {
        let info = probe(&sample).unwrap();
        let span = Span {
            start: Duration::from_millis(500),
            end: Duration::from_millis(4500),
        };
        let window = Window {
            start: Duration::from_millis(1000),
            end: Duration::from_millis(3000),
        };
        let mut edit = Edit::new(span);
        edit.speed = 2.0;
        edit.crop = Some(Area {
            x: 0.1,
            y: 0.1,
            width: 0.5,
            height: 0.5,
        });
        edit.blurs.push(Blur {
            area: Area::centered(0.3, 0.3),
            window,
        });
        edit.strokes.push(Stroke {
            points: vec![(0.2, 0.2), (0.4, 0.4)],
            width: 0.01,
            color: [255, 0, 0],
            window,
        });
        edit.mutes.push(window);
        edit.mix = Mix {
            mic: 0.0,
            ..Mix::default()
        };
        let destination = temp.path().join(sample.file_name().unwrap());
        let progress = std::sync::Mutex::new(0.0f32);
        export(&sample, &edit, &destination, &|value| {
            *progress.lock().unwrap() = value;
        })
        .unwrap();
        assert_eq!(*progress.lock().unwrap(), 1.0);
        let rendered = probe(&destination).unwrap();
        let crop = edit.crop_area().pixels(info.width, info.height).even();
        assert_eq!((rendered.width, rendered.height), (crop.width, crop.height));
        assert!(
            rendered.duration >= Duration::from_millis(1700)
                && rendered.duration <= Duration::from_millis(2300),
            "{:?}",
            rendered.duration
        );
        assert_eq!(
            audio_tracks(&destination).unwrap().min(1),
            audio_tracks(&sample).unwrap().min(1)
        );
        if let Some(keep) = std::env::var_os("REWIND_KEEP") {
            std::fs::copy(
                &destination,
                PathBuf::from(keep).join(sample.file_name().unwrap()),
            )
            .unwrap();
        }
    }
}
