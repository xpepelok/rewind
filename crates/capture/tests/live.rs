#![cfg(windows)]

use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rewind_capture::{
    AudioControls, AudioLayout, Recorder, RecorderConfig, RecorderEvent, ReplayHealth, VideoCodec,
};
use rewind_media::{Edit, Runtime, Span, export, probe};

const WARMUP: Duration = Duration::from_secs(4);
const EVENT_TIMEOUT: Duration = Duration::from_secs(15);
const RECORDING: Duration = Duration::from_secs(3);
const MIN_CLIP: Duration = Duration::from_secs(2);
const MAX_CLIP: Duration = Duration::from_secs(5);
const RESTART_ROUNDS: u32 = 40;
const RESTART_PAUSE: Duration = Duration::from_millis(150);
const FIRST_PACKET: Duration = Duration::from_millis(250);

static DUPLICATION: Mutex<()> = Mutex::new(());

fn exclusive() -> MutexGuard<'static, ()> {
    DUPLICATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn config(root: &std::path::Path, codec: VideoCodec, audio: AudioLayout) -> RecorderConfig {
    RecorderConfig {
        codec,
        fps: 60,
        bitrate: 12_000_000,
        height: Some(1080),
        replay: Duration::from_secs(3),
        audio,
        microphone: None,
        capture_desktop: true,
        window_only: false,
        screen: None,
        gallery: root.join("gallery"),
        thumbnails: root.join("thumbnails"),
        temp: root.join("temp"),
    }
}

fn wait_for(
    events: &mpsc::Receiver<RecorderEvent>,
    matches: impl Fn(&RecorderEvent) -> bool,
) -> RecorderEvent {
    loop {
        let event = events.recv_timeout(EVENT_TIMEOUT).expect("event in time");
        if let RecorderEvent::Failed(message) | RecorderEvent::ReplayFailed(message) = &event {
            panic!("recorder failed: {message}");
        }
        if matches(&event) {
            return event;
        }
    }
}

fn eventually(check: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + EVENT_TIMEOUT;
    while std::time::Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn duration_of(path: &Path) -> Duration {
    let _runtime = Runtime::enter().unwrap();
    probe(path).unwrap().duration
}

fn start(
    root: &Path,
    codec: VideoCodec,
    audio: AudioLayout,
) -> (Recorder, mpsc::Receiver<RecorderEvent>) {
    let (sender, events) = mpsc::channel();
    let controls = Arc::new(AudioControls::new(1.0, 1.0, || true));
    let recorder = Recorder::start(config(root, codec, audio), controls, true, move |event| {
        sender.send(event).ok();
    });
    std::thread::sleep(WARMUP);
    assert!(
        eventually(|| recorder.stats().encoded_fps > 0),
        "{:?}",
        recorder.stats()
    );
    assert_eq!(recorder.replay_health(), ReplayHealth::Running);
    (recorder, events)
}

fn run(codec: VideoCodec, audio: AudioLayout) {
    let root = tempfile::tempdir().unwrap();
    let (recorder, events) = start(root.path(), codec, audio);
    recorder.save_replay();
    recorder.screenshot();
    let mut replay = None;
    let mut screenshot = None;
    while replay.is_none() || screenshot.is_none() {
        match wait_for(&events, |_| true) {
            RecorderEvent::ReplaySaved { path, thumbnail } => replay = Some((path, thumbnail)),
            RecorderEvent::ScreenshotSaved(path) => screenshot = Some(path),
            _ => {}
        }
    }
    let (path, thumbnail) = replay.unwrap();
    let screenshot = screenshot.unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size > 100_000, "clip too small: {size}");
    let duration = duration_of(&path);
    assert!(
        (MIN_CLIP..=MAX_CLIP).contains(&duration),
        "replay lasts {duration:?}"
    );
    assert!(eventually(|| thumbnail.exists()));
    assert!(eventually(
        || std::fs::metadata(&screenshot).is_ok_and(|meta| meta.len() > 0)
    ));
    let keep = std::env::var_os("REWIND_KEEP").map(std::path::PathBuf::from);
    if let Some(keep) = keep {
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::copy(&path, keep.join(path.file_name().unwrap())).unwrap();
    }
    drop(recorder);
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn h264_replay_with_single_track() {
    let _live = exclusive();
    run(VideoCodec::H264, AudioLayout::Single);
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn hevc_replay_with_separate_tracks() {
    let _live = exclusive();
    run(VideoCodec::Hevc, AudioLayout::Separate);
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn a_separate_replay_has_game_mic_and_discord_tracks() {
    let _live = exclusive();
    let root = tempfile::tempdir().unwrap();
    let (recorder, events) = start(root.path(), VideoCodec::H264, AudioLayout::Separate);
    recorder.save_replay();
    let RecorderEvent::ReplaySaved { path, .. } = wait_for(&events, |event| {
        matches!(event, RecorderEvent::ReplaySaved { .. })
    }) else {
        unreachable!()
    };
    drop(recorder);
    let _runtime = Runtime::enter().unwrap();
    assert_eq!(rewind_media::audio_tracks(&path).unwrap(), 3);
    let duration = probe(&path).unwrap().duration;
    assert!(
        (MIN_CLIP..=MAX_CLIP).contains(&duration),
        "replay lasts {duration:?}"
    );
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn recording_round_trip() {
    let _live = exclusive();
    let root = tempfile::tempdir().unwrap();
    let (recorder, events) = start(root.path(), VideoCodec::H264, AudioLayout::Single);
    recorder.toggle_recording();
    wait_for(&events, |event| {
        matches!(event, RecorderEvent::RecordingStarted)
    });
    std::thread::sleep(RECORDING);
    recorder.toggle_recording();
    let RecorderEvent::RecordingSaved { path, .. } = wait_for(&events, |event| {
        matches!(event, RecorderEvent::RecordingSaved { .. })
    }) else {
        unreachable!()
    };
    let duration = duration_of(&path);
    assert!(
        duration >= RECORDING - Duration::from_secs(1),
        "recording lasts {duration:?}"
    );
    drop(recorder);
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn restarting_the_encoder_many_times_is_safe() {
    let _live = exclusive();
    let root = tempfile::tempdir().unwrap();
    for round in 0..RESTART_ROUNDS {
        let mut settings = config(root.path(), VideoCodec::H264, AudioLayout::Single);
        settings.fps = if round % 2 == 0 { 120 } else { 60 };
        settings.height = Some(if round % 3 == 0 { 1440 } else { 1080 });
        let controls = Arc::new(AudioControls::new(1.0, 1.0, || true));
        let recorder = Recorder::start(settings, controls, true, |_| {});
        std::thread::sleep(RESTART_PAUSE * (round % 4 + 1));
        drop(recorder);
    }
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn health_follows_the_replay_switch() {
    let _live = exclusive();
    let root = tempfile::tempdir().unwrap();
    let (recorder, _events) = start(root.path(), VideoCodec::H264, AudioLayout::Single);
    recorder.set_replay(false);
    assert_eq!(recorder.replay_health(), ReplayHealth::Off);
    recorder.set_replay(true);
    std::thread::sleep(WARMUP);
    assert_eq!(recorder.replay_health(), ReplayHealth::Running);
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn marks_cut_clips_out_of_the_buffer() {
    let _live = exclusive();
    let root = tempfile::tempdir().unwrap();
    let (recorder, events) = start(root.path(), VideoCodec::H264, AudioLayout::Single);
    assert!(recorder.mark());
    let window = recorder.replay_window().unwrap();
    assert_eq!(window.marks.len(), 1);
    let mark = window.marks[0];
    let second = 10_000_000;
    recorder.save_replay_ranges(
        vec![(mark - 2 * second, mark), (window.start, window.end)],
        false,
    );
    let mut saved = Vec::new();
    while saved.len() < 2 {
        if let RecorderEvent::ReplaySaved { path, thumbnail } = wait_for(&events, |event| {
            matches!(event, RecorderEvent::ReplaySaved { .. })
        }) {
            assert!(thumbnail.exists());
            saved.push(path);
        }
    }
    let around = duration_of(&saved[0]);
    assert!(
        (Duration::from_millis(1500)..=Duration::from_secs(4)).contains(&around),
        "mark clip lasts {around:?}"
    );
    assert!(duration_of(&saved[1]) >= MIN_CLIP);
    drop(recorder);
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn a_compressed_copy_keeps_to_its_bitrate() {
    let _live = exclusive();
    let root = tempfile::tempdir().unwrap();
    let (recorder, events) = start(root.path(), VideoCodec::H264, AudioLayout::Single);
    recorder.save_replay();
    let RecorderEvent::ReplaySaved { path, .. } = wait_for(&events, |event| {
        matches!(event, RecorderEvent::ReplaySaved { .. })
    }) else {
        unreachable!()
    };
    drop(recorder);
    let duration = duration_of(&path);
    let mut edit = Edit::new(Span::whole(duration));
    edit.bitrate = Some(500_000);
    edit.height = Some(480);
    let target = root.path().join("shared.mp4");
    {
        let _runtime = Runtime::enter().unwrap();
        export(&path, &edit, &target, &|_| {}).unwrap();
    }
    let budget =
        (500_000 + rewind_media::AUDIO_BITS_PER_SECOND) as f64 / 8.0 * duration.as_secs_f64();
    let size = std::fs::metadata(&target).unwrap().len() as f64;
    assert!(size < budget * 1.5, "{size} vs budget {budget}");
}

#[test]
#[ignore = "needs a real windows session"]
fn an_exiting_program_is_reported_right_away() {
    let (sender, events) = mpsc::channel();
    let sender = std::sync::Mutex::new(sender);
    rewind_capture::watch_programs(move |event| {
        sender.lock().unwrap().send(event).ok();
    });
    let mut child = std::process::Command::new("ping")
        .args(["-n", "2", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let process = child.id();
    assert!(rewind_capture::running_programs().contains_key("ping.exe"));
    rewind_capture::watch_exit(process);
    let started = std::time::Instant::now();
    loop {
        let event = events.recv_timeout(EVENT_TIMEOUT).expect("exit in time");
        if event == rewind_capture::ProgramEvent::Exited(process) {
            break;
        }
    }
    child.wait().unwrap();
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn the_buffer_renders_preview_frames() {
    let _live = exclusive();
    let root = tempfile::tempdir().unwrap();
    let (recorder, _events) = start(root.path(), VideoCodec::H264, AudioLayout::Single);
    let window = recorder.replay_window().unwrap();
    let middle = (window.start + window.end) / 2;
    let frames = recorder.previews().render(
        &[window.start, middle, window.end],
        &root.path().join("previews"),
        160,
    );
    assert_eq!(frames.len(), 3);
    for frame in frames {
        let frame = frame.expect("a preview frame");
        assert!(std::fs::metadata(&frame).unwrap().len() > 1_000);
    }
    drop(recorder);
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn two_replays_join_into_one_clip() {
    let _live = exclusive();
    let root = tempfile::tempdir().unwrap();
    let mut clips = Vec::new();
    for _ in 0..2 {
        let (recorder, events) = start(root.path(), VideoCodec::H264, AudioLayout::Separate);
        recorder.save_replay();
        if let RecorderEvent::ReplaySaved { path, .. } = wait_for(&events, |event| {
            matches!(event, RecorderEvent::ReplaySaved { .. })
        }) {
            clips.push(path);
        }
        drop(recorder);
    }
    let total: Duration = clips.iter().map(|clip| duration_of(clip)).sum();
    let joined = root.path().join("joined.mp4");
    {
        let _runtime = Runtime::enter().unwrap();
        rewind_media::concat(&clips, &joined, &|_| {}).unwrap();
    }
    let length = duration_of(&joined);
    let gap = length.abs_diff(total);
    assert!(gap < Duration::from_millis(300), "{length:?} vs {total:?}");
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn the_first_frame_is_encoded_right_after_the_encoder_opens() {
    let _live = exclusive();
    let root = tempfile::tempdir().unwrap();
    let (sender, events) = mpsc::channel();
    let controls = Arc::new(AudioControls::new(1.0, 1.0, || true));
    let settings = config(root.path(), VideoCodec::H264, AudioLayout::Single);
    let recorder = Recorder::start(settings, controls, true, move |event| {
        sender.send(event).ok();
    });
    wait_for(&events, |event| matches!(event, RecorderEvent::Encoder(_)));
    let opened = Instant::now();
    while recorder.replay_window().is_none() && opened.elapsed() < EVENT_TIMEOUT {
        std::thread::sleep(Duration::from_millis(5));
    }
    let latency = opened.elapsed();
    assert!(latency < FIRST_PACKET, "first packet after {latency:?}");
    drop(recorder);
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn a_high_rate_replay_keeps_real_time() {
    let _live = exclusive();
    let root = tempfile::tempdir().unwrap();
    let (sender, events) = mpsc::channel();
    let controls = Arc::new(AudioControls::new(1.0, 1.0, || true));
    let mut settings = config(root.path(), VideoCodec::H264, AudioLayout::Single);
    settings.fps = 120;
    let recorder = Recorder::start(settings, controls, true, move |event| {
        sender.send(event).ok();
    });
    std::thread::sleep(WARMUP);
    recorder.save_replay();
    let RecorderEvent::ReplaySaved { path, .. } = wait_for(&events, |event| {
        matches!(event, RecorderEvent::ReplaySaved { .. })
    }) else {
        unreachable!()
    };
    drop(recorder);
    let duration = duration_of(&path);
    assert!(
        (MIN_CLIP..=MAX_CLIP).contains(&duration),
        "high rate replay lasts {duration:?}"
    );
    let _runtime = Runtime::enter().unwrap();
    let frames = rewind_media::frames_at(&path, &[Duration::from_millis(500), duration / 2], None);
    assert!(frames.is_ok_and(|frames| frames.len() == 2));
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn saving_a_replay_starts_the_buffer_over_and_refuses_a_second_save_meanwhile() {
    let _live = exclusive();
    let root = tempfile::tempdir().unwrap();
    let (sender, events) = mpsc::channel();
    let controls = Arc::new(AudioControls::new(1.0, 1.0, || true));
    let mut settings = config(root.path(), VideoCodec::H264, AudioLayout::Single);
    settings.replay = Duration::from_secs(60);
    let recorder = Recorder::start(settings, controls, true, move |event| {
        sender.send(event).ok();
    });
    std::thread::sleep(Duration::from_secs(8));
    let second = 10_000_000;
    let before = recorder.replay_window().unwrap();
    assert!(before.end - before.start >= 6 * second, "{before:?}");
    assert!(recorder.mark());

    assert!(!recorder.is_saving());
    assert!(recorder.save_replay());
    assert!(!recorder.save_replay(), "a second save is refused");
    assert!(!recorder.save_replay_ranges(vec![(before.start, before.end)], true));
    let held = recorder
        .replay_window()
        .map_or(0, |window| window.end - window.start);
    assert!(held < 2 * second, "the buffer started over: {held}");

    let saved = wait_for(&events, |event| {
        matches!(event, RecorderEvent::ReplaySaved { .. })
    });
    let RecorderEvent::ReplaySaved { path, .. } = saved else {
        unreachable!()
    };
    assert!(!recorder.is_saving());
    assert!(duration_of(&path) >= Duration::from_secs(6));

    std::thread::sleep(Duration::from_secs(3));
    let after = recorder.replay_window().unwrap();
    assert!(after.start > before.end, "{before:?} {after:?}");
    assert!(after.marks.is_empty(), "{after:?}");
    let length = after.end - after.start;
    assert!((2 * second..5 * second).contains(&length), "{length}");

    assert!(recorder.save_replay(), "saving works again");
    let again = wait_for(&events, |event| {
        matches!(event, RecorderEvent::ReplaySaved { .. })
    });
    let RecorderEvent::ReplaySaved { path, .. } = again else {
        unreachable!()
    };
    let clip = duration_of(&path);
    assert!(
        (Duration::from_secs(2)..Duration::from_secs(5)).contains(&clip),
        "{clip:?}"
    );
    drop(recorder);
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn every_screen_keeps_its_own_replay() {
    let _live = exclusive();
    let screens = rewind_capture::screens();
    let roots: Vec<_> = screens
        .iter()
        .map(|_| tempfile::tempdir().unwrap())
        .collect();
    let running: Vec<_> = screens
        .iter()
        .zip(&roots)
        .map(|(screen, root)| {
            let (sender, events) = mpsc::channel();
            let controls = Arc::new(AudioControls::new(1.0, 1.0, || true));
            let config = RecorderConfig {
                screen: Some(*screen),
                height: None,
                ..config(root.path(), VideoCodec::H264, AudioLayout::Single)
            };
            let recorder = Recorder::start(config, controls, true, move |event| {
                sender.send(event).ok();
            });
            (recorder, events)
        })
        .collect();
    for (recorder, _) in &running {
        assert!(
            eventually(|| recorder.stats().buffered >= MIN_CLIP),
            "{:?}",
            recorder.stats()
        );
    }
    for ((recorder, events), screen) in running.iter().zip(&screens) {
        assert!(recorder.save_replay());
        let RecorderEvent::ReplaySaved { path, .. } = wait_for(events, |event| {
            matches!(event, RecorderEvent::ReplaySaved { .. })
        }) else {
            unreachable!()
        };
        let _runtime = Runtime::enter().unwrap();
        let info = probe(&path).unwrap();
        assert_eq!((info.width, info.height), (screen.width, screen.height));
        assert!(
            info.duration >= MIN_CLIP,
            "replay lasts {:?}",
            info.duration
        );
    }
}

#[test]
#[ignore = "needs a real display and gpu encoder"]
fn the_verbose_log_follows_a_recording_and_a_replay() {
    let _live = exclusive();
    let root = tempfile::tempdir().unwrap();
    rewind_trace::open(&root.path().join("logs"));
    rewind_trace::set_enabled(true);
    let (recorder, events) = start(root.path(), VideoCodec::H264, AudioLayout::Separate);
    recorder.toggle_recording();
    wait_for(&events, |event| {
        matches!(event, RecorderEvent::RecordingStarted)
    });
    std::thread::sleep(RECORDING);
    recorder.toggle_recording();
    wait_for(&events, |event| {
        matches!(event, RecorderEvent::RecordingSaved { .. })
    });
    assert!(recorder.save_replay());
    wait_for(&events, |event| {
        matches!(event, RecorderEvent::ReplaySaved { .. })
    });
    drop(recorder);
    rewind_trace::set_enabled(false);
    let log = std::fs::read_to_string(root.path().join("logs").join(rewind_trace::FILE)).unwrap();
    println!("{log}");
    for step in [
        "recorder starting, replay true",
        "[audio] (rewind-audio) audio session ready",
        "[encoder] (rewind-video) using encoder",
        "[recorder] (rewind-video) first encoded frame",
        "recording start requested",
        "recording written to",
        "saving the replay to",
        "replay save finished",
        "recorder stopped",
    ] {
        assert!(log.contains(step), "the log misses {step:?}");
    }
    assert!(!log.contains("FAILED"), "{log}");
}
