use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use rewind_trace::{Traced, trace};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_DEVICE_REMOVED};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, GetCurrentThread, INFINITE,
    SetThreadPriority, SetWaitableTimer, THREAD_PRIORITY_ABOVE_NORMAL, TIMER_ALL_ACCESS,
    WaitForSingleObject,
};
use windows::core::{HSTRING, w};

use crate::audio::{self, AudioControls, AudioSetup};
use crate::clock;
use crate::com::Runtime;
use crate::convert::{Converter, fit};
use crate::device::Gpu;
use crate::duplication::{Acquired, Duplicator};
use crate::encoder::Encoder;
use crate::error::{CaptureError, Result};
use crate::foreground;
use crate::format::{AudioFormat, HNS_PER_SECOND, Packet, VideoCodec, VideoFormat};
use crate::mux::{Mp4Writer, write_clip};
use crate::naming::{self, Kind};
use crate::ring::Ring;
use crate::snapshot::Image;
use crate::window::WindowCapture;

const RESTART_DELAY: Duration = Duration::from_secs(1);
const IDLE_DELAY: Duration = Duration::from_millis(100);
const IDLE_WAIT: Duration = Duration::from_secs(1);
const ENCODER_POLL: Duration = Duration::from_millis(1);
const FOREGROUND_CHECK: Duration = Duration::from_millis(500);
const STATS_INTERVAL: Duration = Duration::from_secs(1);
const STALE_FRAME: Duration = Duration::from_millis(250);
const IDLE_FRAME: Duration = Duration::from_millis(500);
const KEY_INTERVAL: Duration = Duration::from_secs(1);
const FORMAT_WAIT: Duration = Duration::from_secs(5);
const MAX_LAG_FRAMES: i64 = 4;
const THUMBNAIL_WIDTH: u32 = 320;
const RING_HEADROOM: f64 = 1.5;
const AUDIO_BYTES_PER_SECOND: u64 = 72_000;
const DISK_RESERVE: u64 = 10 * 1024 * 1024 * 1024;
const MEMORY_SHARE: u64 = 4;
const STARTUP_GRACE: Duration = Duration::from_secs(4);
const STALL_AFTER: Duration = Duration::from_secs(5);
const AREA: &str = "recorder";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioLayout {
    Single,
    Separate,
}

impl AudioLayout {
    pub const fn tracks(self) -> usize {
        audio::track_count(matches!(self, Self::Separate))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecorderConfig {
    pub codec: VideoCodec,
    pub fps: u32,
    pub bitrate: u32,
    pub height: Option<u32>,
    pub replay: Duration,
    pub audio: AudioLayout,
    pub microphone: Option<String>,
    pub capture_desktop: bool,
    pub window_only: bool,
    pub screen: Option<crate::device::Screen>,
    pub gallery: PathBuf,
    pub thumbnails: PathBuf,
    pub temp: PathBuf,
}

pub fn replay_bytes(bitrate: u32, replay: Duration) -> u64 {
    let per_second = u64::from(bitrate) / 8 + AUDIO_BYTES_PER_SECOND;
    (per_second as f64 * replay.as_secs_f64() * RING_HEADROOM) as u64
}

pub fn free_space(directory: &Path) -> Option<u64> {
    let mut probe = directory.to_path_buf();
    while !probe.exists() {
        probe = probe.parent()?.to_path_buf();
    }
    let mut available = 0u64;
    unsafe {
        GetDiskFreeSpaceExW(
            &HSTRING::from(probe.as_os_str()),
            Some(&mut available),
            None,
            None,
        )
    }
    .ok()?;
    Some(available)
}

fn spool_cap(needed: u64, free: Option<u64>) -> u64 {
    match free {
        Some(free) => needed.min(free.saturating_sub(DISK_RESERVE)),
        None => needed,
    }
}

pub fn free_memory() -> Option<u64> {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some(status.ullAvailPhys)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RingPlan {
    pub bytes: u64,
    pub on_disk: bool,
}

fn memory_budget() -> u64 {
    free_memory().map_or(0, |free| free / MEMORY_SHARE)
}

pub fn ring_plan(needed: u64, disk_free: Option<u64>, memory_free: Option<u64>) -> RingPlan {
    let disk = spool_cap(needed, disk_free);
    let memory = memory_free.map_or(0, |free| needed.min(free / MEMORY_SHARE));
    if disk >= memory {
        RingPlan {
            bytes: disk,
            on_disk: true,
        }
    } else {
        RingPlan {
            bytes: memory,
            on_disk: false,
        }
    }
}

impl RecorderConfig {
    fn ring_plan(&self) -> RingPlan {
        ring_plan(
            replay_bytes(self.bitrate, self.replay),
            free_space(&self.temp),
            free_memory(),
        )
    }

    fn ring(&self) -> Ring {
        let plan = self.ring_plan();
        Ring::new(
            self.replay,
            usize::try_from(plan.bytes).unwrap_or(usize::MAX),
            self.audio.tracks(),
            plan.on_disk.then_some(self.temp.as_path()),
        )
        .with_memory_budget(memory_budget)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecorderEvent {
    Encoder(String),
    ReplaySaved { path: PathBuf, thumbnail: PathBuf },
    ReplayFailed(String),
    RecordingStarted,
    RecordingSaved { path: PathBuf, thumbnail: PathBuf },
    ScreenshotSaved(PathBuf),
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub display_fps: u32,
    pub encoded_fps: u32,
    pub dropped: u32,
    pub buffered: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayHealth {
    Off,
    Starting,
    Running,
    Waiting,
    Stalled(Option<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pulse {
    pub age: Duration,
    pub active: bool,
    pub encoded_fps: u32,
}

pub fn classify(
    replay: bool,
    uptime: Duration,
    pulse: Option<Pulse>,
    failure: Option<String>,
) -> ReplayHealth {
    if !replay {
        return ReplayHealth::Off;
    }
    match pulse {
        Some(pulse) if pulse.age <= STALL_AFTER => {
            if !pulse.active {
                ReplayHealth::Waiting
            } else if pulse.encoded_fps > 0 {
                ReplayHealth::Running
            } else if uptime < STARTUP_GRACE {
                ReplayHealth::Starting
            } else {
                ReplayHealth::Stalled(failure)
            }
        }
        _ if uptime < STARTUP_GRACE => ReplayHealth::Starting,
        _ => ReplayHealth::Stalled(failure),
    }
}

#[derive(Default)]
struct Requests {
    keyframe: bool,
    screenshot: Option<PathBuf>,
    thumbnail: Option<PathBuf>,
}

struct Recording {
    sender: mpsc::Sender<Packet>,
    path: PathBuf,
    started: Instant,
}

type Events = Box<dyn Fn(RecorderEvent) + Send + Sync>;

#[derive(Default)]
struct Wake {
    generation: Mutex<u64>,
    condvar: Condvar,
}

impl Wake {
    fn seen(&self) -> u64 {
        *self.generation.lock()
    }

    fn nudge(&self) {
        *self.generation.lock() += 1;
        self.condvar.notify_all();
    }

    fn wait(&self, seen: u64, timeout: Duration) {
        let mut generation = self.generation.lock();
        if *generation == seen {
            self.condvar.wait_for(&mut generation, timeout);
        }
    }
}

struct Shared {
    config: RecorderConfig,
    controls: Arc<AudioControls>,
    stop: AtomicBool,
    replay: AtomicBool,
    saving: AtomicBool,
    monitor: AtomicBool,
    ring: Mutex<Ring>,
    video_format: Mutex<Option<VideoFormat>>,
    audio_formats: Mutex<Vec<AudioFormat>>,
    recording: Mutex<Option<Recording>>,
    requests: Mutex<Requests>,
    stats: Mutex<Stats>,
    pulse: Mutex<Option<(Instant, bool)>>,
    failure: Mutex<Option<String>>,
    marks: Mutex<Vec<i64>>,
    failed_window: Mutex<Option<isize>>,
    started: Instant,
    wake: Wake,
    sink: Mutex<Option<mpsc::Sender<Vec<Packet>>>>,
    events: Events,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayWindow {
    pub start: i64,
    pub end: i64,
    pub marks: Vec<i64>,
}

impl Shared {
    fn push(&self, packet: Packet) {
        self.push_all(vec![packet]);
    }

    fn push_all(&self, packets: Vec<Packet>) {
        if let Some(sink) = self.sink.lock().as_ref() {
            sink.send(packets).ok();
        }
    }

    fn store(&self, packet: Packet) {
        if self.replay.load(Ordering::Relaxed) {
            self.ring.lock().push(packet.clone());
        }
        if let Some(recording) = self.recording.lock().as_ref() {
            recording.sender.send(packet).ok();
        }
    }

    fn encoding(&self) -> bool {
        self.replay.load(Ordering::Relaxed) || self.recording.lock().is_some()
    }

    fn emit(&self, event: RecorderEvent) {
        (self.events)(event);
    }

    fn begin_save(&self) -> bool {
        !self.saving.swap(true, Ordering::AcqRel)
    }

    fn end_save(&self, outcome: Result<Vec<RecorderEvent>>) {
        self.saving.store(false, Ordering::Release);
        match outcome {
            Ok(events) => {
                trace!(AREA, "replay save finished: {events:?}");
                events.into_iter().for_each(|event| self.emit(event));
            }
            Err(error) => {
                trace!(AREA, "FAILED replay save: {error:?}");
                self.emit(RecorderEvent::ReplayFailed(error.to_string()));
            }
        }
    }

    fn take(
        &self,
        snapshot: impl FnOnce(&Ring) -> Option<crate::ring::Clip>,
        start_over: bool,
    ) -> Option<crate::ring::Clip> {
        let mut ring = self.ring.lock();
        let clip = snapshot(&ring);
        if start_over && clip.is_some() {
            self.start_over(&mut ring);
        }
        clip
    }

    fn start_over(&self, ring: &mut Ring) {
        ring.clear();
        self.marks.lock().clear();
        self.requests.lock().keyframe = true;
        self.wake.nudge();
    }

    fn fail(&self, error: &CaptureError) {
        let reason = error.to_string();
        trace!(AREA, "FAILED capture, reported to the app: {error:?}");
        *self.failure.lock() = Some(reason.clone());
        self.emit(RecorderEvent::Failed(reason));
    }
}

pub struct Recorder {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

#[derive(Clone)]
pub struct Previews {
    shared: Arc<Shared>,
}

impl Previews {
    pub fn render(&self, times: &[i64], directory: &Path, width: u32) -> Vec<Option<PathBuf>> {
        let Ok(_runtime) = Runtime::enter() else {
            return vec![None; times.len()];
        };
        let Some(video) = self.shared.video_format.lock().clone() else {
            return vec![None; times.len()];
        };
        std::fs::create_dir_all(directory).ok();
        times
            .iter()
            .enumerate()
            .map(|(index, time)| {
                let clip = self.shared.ring.lock().keyframe_at(*time)?;
                let movie = directory.join(format!("{index}.mp4"));
                let picture = directory.join(format!("{index}.png"));
                let made = write_clip(&movie, &video, &[], &clip).is_ok()
                    && rewind_media::thumbnail(&movie, Duration::ZERO, width, &picture).is_ok();
                std::fs::remove_file(&movie).ok();
                made.then_some(picture)
            })
            .collect()
    }
}

impl Recorder {
    pub fn start(
        config: RecorderConfig,
        controls: Arc<AudioControls>,
        replay: bool,
        events: impl Fn(RecorderEvent) + Send + Sync + 'static,
    ) -> Self {
        trace!(
            AREA,
            "recorder starting, replay {replay}, plan {:?}, free disk {:?}, free memory {:?}, config {config:?}",
            config.ring_plan(),
            free_space(&config.temp),
            free_memory()
        );
        let ring = config.ring();
        let (sink, packets) = mpsc::channel();
        let shared = Arc::new(Shared {
            config,
            controls,
            stop: AtomicBool::new(false),
            replay: AtomicBool::new(replay),
            saving: AtomicBool::new(false),
            monitor: AtomicBool::new(false),
            ring: Mutex::new(ring),
            video_format: Mutex::new(None),
            audio_formats: Mutex::new(Vec::new()),
            recording: Mutex::new(None),
            requests: Mutex::new(Requests::default()),
            stats: Mutex::new(Stats::default()),
            pulse: Mutex::new(None),
            failure: Mutex::new(None),
            marks: Mutex::new(Vec::new()),
            failed_window: Mutex::new(None),
            started: Instant::now(),
            wake: Wake::default(),
            sink: Mutex::new(Some(sink)),
            events: Box::new(events),
        });
        let video = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("rewind-video".to_owned())
                .spawn(move || video_loop(&shared))
        };
        let audio = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("rewind-audio".to_owned())
                .spawn(move || audio_loop(&shared))
        };
        let store = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("rewind-store".to_owned())
                .spawn(move || store_loop(&shared, &packets))
        };
        let threads = [video, audio, store]
            .into_iter()
            .filter_map(|thread| match thread {
                Ok(thread) => Some(thread),
                Err(error) => {
                    shared.fail(&error.into());
                    None
                }
            })
            .collect();
        Self { shared, threads }
    }

    pub fn config(&self) -> &RecorderConfig {
        &self.shared.config
    }

    pub fn stats(&self) -> Stats {
        *self.shared.stats.lock()
    }

    pub fn replay_health(&self) -> ReplayHealth {
        let pulse = (*self.shared.pulse.lock()).map(|(at, active)| Pulse {
            age: at.elapsed(),
            active,
            encoded_fps: self.shared.stats.lock().encoded_fps,
        });
        classify(
            self.replay_enabled(),
            self.shared.started.elapsed(),
            pulse,
            self.shared.failure.lock().clone(),
        )
    }

    pub fn replay_enabled(&self) -> bool {
        self.shared.replay.load(Ordering::Relaxed)
    }

    pub fn set_replay(&self, enabled: bool) {
        trace!(AREA, "replay switched to {enabled}");
        self.shared.replay.store(enabled, Ordering::Relaxed);
        if enabled {
            self.shared.requests.lock().keyframe = true;
        } else {
            self.shared.ring.lock().clear();
        }
        self.shared.wake.nudge();
    }

    pub fn set_monitor(&self, enabled: bool) {
        self.shared.monitor.store(enabled, Ordering::Relaxed);
        self.shared.wake.nudge();
    }

    pub fn is_recording(&self) -> bool {
        self.shared.recording.lock().is_some()
    }

    pub fn recording_elapsed(&self) -> Option<Duration> {
        self.shared
            .recording
            .lock()
            .as_ref()
            .map(|recording| recording.started.elapsed())
    }

    pub fn is_saving(&self) -> bool {
        self.shared.saving.load(Ordering::Acquire)
    }

    pub fn save_replay(&self) -> bool {
        if !self.shared.begin_save() {
            trace!(AREA, "replay save refused, another save is still running");
            return false;
        }
        let path = naming::next_path(&self.shared.config.gallery, Kind::Replay);
        trace!(
            AREA,
            "saving the replay to {path:?}, buffered {:?}, health {:?}",
            self.shared.ring.lock().buffered(),
            self.replay_health()
        );
        let thumbnail = naming::thumbnail_path(&self.shared.config.thumbnails, &path);
        self.shared.requests.lock().thumbnail = Some(thumbnail.clone());
        self.shared.wake.nudge();
        let clip = self
            .shared
            .take(|ring| ring.snapshot(self.shared.config.replay), true);
        let shared = self.shared.clone();
        let spawned = thread::Builder::new()
            .name("rewind-save".to_owned())
            .spawn(move || {
                let saved = write_snapshot(&shared, &path, clip)
                    .map(|()| vec![RecorderEvent::ReplaySaved { path, thumbnail }]);
                shared.end_save(saved);
            });
        if let Err(error) = spawned {
            self.shared.end_save(Err(error.into()));
        }
        true
    }

    pub fn previews(&self) -> Previews {
        Previews {
            shared: self.shared.clone(),
        }
    }

    pub fn mark(&self) -> bool {
        let Some(newest) = self.shared.ring.lock().newest() else {
            return false;
        };
        self.shared.marks.lock().push(newest);
        true
    }

    pub fn replay_window(&self) -> Option<ReplayWindow> {
        let (start, end) = self.shared.ring.lock().span()?;
        let mut marks = self.shared.marks.lock();
        marks.retain(|mark| *mark >= start);
        Some(ReplayWindow {
            start,
            end,
            marks: marks.clone(),
        })
    }

    pub fn save_replay_ranges(&self, ranges: Vec<(i64, i64)>, start_over: bool) -> bool {
        if !self.shared.begin_save() {
            trace!(
                AREA,
                "replay range save refused, another save is still running"
            );
            return false;
        }
        trace!(
            AREA,
            "saving replay ranges {ranges:?}, start over {start_over}, buffer span {:?}",
            self.shared.ring.lock().span()
        );
        let clips: Vec<_> = ranges
            .into_iter()
            .filter_map(|(from, to)| {
                self.shared
                    .take(|ring| ring.snapshot_range(from, to), false)
            })
            .collect();
        if start_over && !clips.is_empty() {
            self.shared.start_over(&mut self.shared.ring.lock());
        }
        let shared = self.shared.clone();
        let spawned = thread::Builder::new()
            .name("rewind-save".to_owned())
            .spawn(move || {
                let saved = save_clips(&shared, clips);
                shared.end_save(saved);
            });
        if let Err(error) = spawned {
            self.shared.end_save(Err(error.into()));
        }
        true
    }

    pub fn toggle_recording(&self) {
        let mut recording = self.shared.recording.lock();
        if let Some(active) = recording.take() {
            trace!(
                AREA,
                "recording stop requested for {:?} after {:?}",
                active.path,
                active.started.elapsed()
            );
            let thumbnail = naming::thumbnail_path(&self.shared.config.thumbnails, &active.path);
            self.shared.requests.lock().thumbnail = Some(thumbnail);
            self.shared.wake.nudge();
            return;
        }
        let path = naming::next_path(&self.shared.config.gallery, Kind::Recording);
        trace!(AREA, "recording start requested, file {path:?}");
        let (sender, receiver) = mpsc::channel();
        let shared = self.shared.clone();
        let writer_path = path.clone();
        let spawned = thread::Builder::new()
            .name("rewind-record".to_owned())
            .spawn(move || {
                let thumbnail = naming::thumbnail_path(&shared.config.thumbnails, &writer_path);
                match record(&shared, &writer_path, &receiver) {
                    Ok(()) => {
                        trace!(AREA, "recording written to {writer_path:?}");
                        shared.emit(RecorderEvent::RecordingSaved {
                            path: writer_path,
                            thumbnail,
                        });
                    }
                    Err(error) => {
                        trace!(AREA, "FAILED recording to {writer_path:?}: {error:?}");
                        shared.fail(&error);
                    }
                }
            });
        match spawned {
            Ok(_) => {
                *recording = Some(Recording {
                    sender,
                    path,
                    started: Instant::now(),
                });
                self.shared.requests.lock().keyframe = true;
                self.shared.wake.nudge();
                self.shared.emit(RecorderEvent::RecordingStarted);
            }
            Err(error) => self.shared.fail(&error.into()),
        }
    }

    pub fn screenshot(&self) {
        let path = naming::next_path(&self.shared.config.gallery, Kind::Screenshot);
        trace!(AREA, "screenshot requested, file {path:?}");
        self.shared.requests.lock().screenshot = Some(path);
        self.shared.wake.nudge();
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        trace!(
            AREA,
            "recorder stopping, recording active {}, replay {}",
            self.is_recording(),
            self.replay_enabled()
        );
        self.shared.recording.lock().take();
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.sink.lock().take();
        self.shared.wake.nudge();
        for thread in self.threads.drain(..) {
            if thread.join().is_err() {
                trace!(AREA, "FAILED a recorder thread panicked");
            }
        }
        trace!(AREA, "recorder stopped");
    }
}

fn save_clips(shared: &Shared, clips: Vec<crate::ring::Clip>) -> Result<Vec<RecorderEvent>> {
    if clips.is_empty() {
        return Err(CaptureError::Empty);
    }
    let mut saved = Vec::with_capacity(clips.len());
    for clip in clips {
        let path = naming::next_path(&shared.config.gallery, Kind::Replay);
        let thumbnail = naming::thumbnail_path(&shared.config.thumbnails, &path);
        write_snapshot(shared, &path, Some(clip))?;
        rewind_media::thumbnail(&path, Duration::ZERO, THUMBNAIL_WIDTH, &thumbnail)
            .traced(AREA, "make a thumbnail of the saved replay")
            .ok();
        saved.push(RecorderEvent::ReplaySaved { path, thumbnail });
    }
    Ok(saved)
}

fn write_snapshot(shared: &Shared, path: &Path, clip: Option<crate::ring::Clip>) -> Result<()> {
    let _runtime = Runtime::enter().traced(AREA, "enter the COM runtime for a save")?;
    let clip = clip
        .ok_or(CaptureError::Empty)
        .traced(AREA, "take frames out of the replay buffer, it is empty")?;
    let video = shared
        .video_format
        .lock()
        .clone()
        .ok_or(CaptureError::NoFormat)
        .traced(
            AREA,
            "read the video format, the encoder has produced nothing yet",
        )?;
    let audio = shared.audio_formats.lock().clone();
    trace!(
        AREA,
        "writing a clip to {path:?}, video {}x{}, audio tracks {}",
        video.width,
        video.height,
        audio.len()
    );
    write_clip(path, &video, &audio, &clip).traced(AREA, "write the clip file")
}

fn wait_for_formats(shared: &Shared) -> Result<(VideoFormat, Vec<AudioFormat>)> {
    let deadline = Instant::now() + FORMAT_WAIT;
    loop {
        if let Some(video) = shared.video_format.lock().clone() {
            return Ok((video, shared.audio_formats.lock().clone()));
        }
        if Instant::now() > deadline {
            trace!(
                AREA,
                "FAILED the encoder gave no video format in {FORMAT_WAIT:?}"
            );
            return Err(CaptureError::NoFormat);
        }
        thread::sleep(IDLE_DELAY);
    }
}

fn record(shared: &Shared, path: &Path, packets: &mpsc::Receiver<Packet>) -> Result<()> {
    let _runtime = Runtime::enter().traced(AREA, "enter the COM runtime for a recording")?;
    let first = loop {
        let packet = packets.recv().map_err(|_| CaptureError::Empty).traced(
            AREA,
            "wait for the first keyframe, the recording ended before it",
        )?;
        if packet.track == crate::format::Track::Video && packet.keyframe {
            break packet;
        }
    };
    let (video, audio) = wait_for_formats(shared)?;
    let mut writer = Mp4Writer::create(path, &video, &audio, first.pts)
        .traced(AREA, "create the recording file")?;
    trace!(
        AREA,
        "recording file {path:?} created, video {}x{}, audio tracks {}",
        video.width,
        video.height,
        audio.len()
    );
    let mut pending = first;
    while let Ok(packet) = packets.recv() {
        if packet.track == crate::format::Track::Video {
            let mut previous = std::mem::replace(&mut pending, packet);
            if pending.pts > previous.pts {
                previous.duration = pending.pts - previous.pts;
            }
            writer
                .write(&previous)
                .traced(AREA, "write a video packet to the recording")?;
        } else {
            writer
                .write(&packet)
                .traced(AREA, "write an audio packet to the recording")?;
        }
    }
    writer
        .write(&pending)
        .traced(AREA, "write the last video packet to the recording")?;
    writer.finish().traced(AREA, "finalize the recording file")
}

fn store_loop(shared: &Shared, batches: &mpsc::Receiver<Vec<Packet>>) {
    while let Ok(batch) = batches.recv() {
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        for packet in batch {
            shared.store(packet);
        }
    }
}

fn audio_loop(shared: &Shared) {
    let setup = AudioSetup {
        separate_tracks: shared.config.audio == AudioLayout::Separate,
        microphone: shared.config.microphone.clone(),
    };
    while !shared.stop.load(Ordering::Relaxed) {
        let seen = shared.wake.seen();
        if !shared.encoding() {
            shared.wake.wait(seen, IDLE_WAIT);
            continue;
        }
        let result = audio::run(
            &setup,
            &shared.controls,
            || !shared.stop.load(Ordering::Relaxed) && shared.encoding(),
            |formats| *shared.audio_formats.lock() = formats,
            |packets| shared.push_all(packets),
        );
        if let Err(error) = result {
            trace!(
                AREA,
                "audio session ended with an error, restarting in {RESTART_DELAY:?}"
            );
            shared.fail(&error);
            thread::sleep(RESTART_DELAY);
        }
    }
}

fn video_loop(shared: &Shared) {
    let _boost = Boost::enter();
    while !shared.stop.load(Ordering::Relaxed) {
        let seen = shared.wake.seen();
        if !wanted(shared) {
            shared.wake.wait(seen, IDLE_WAIT);
            continue;
        }
        if let Err(error) = video_session(shared) {
            let lost = matches!(
                &error,
                CaptureError::Windows(inner)
                    if inner.code() == DXGI_ERROR_ACCESS_LOST || inner.code() == DXGI_ERROR_DEVICE_REMOVED
            );
            trace!(
                AREA,
                "FAILED video session, desktop access lost {lost}, restarting: {error:?}"
            );
            if !lost {
                shared.fail(&error);
            }
            thread::sleep(if lost { IDLE_DELAY } else { RESTART_DELAY });
        }
    }
}

struct Boost {
    task: Option<HANDLE>,
}

impl Boost {
    fn enter() -> Self {
        let mut index = 0u32;
        let task = unsafe { AvSetMmThreadCharacteristicsW(w!("Capture"), &mut index) }.ok();
        let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) };
        Self { task }
    }
}

impl Drop for Boost {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            let _ = unsafe { AvRevertMmThreadCharacteristics(task) };
        }
    }
}

struct Pacer {
    timer: HANDLE,
    interval: i64,
    next: i64,
}

impl Pacer {
    fn new(fps: u32) -> Result<Self> {
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                None,
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }?;
        Ok(Self {
            timer,
            interval: HNS_PER_SECOND / i64::from(fps.max(1)),
            next: clock::now(),
        })
    }

    fn catch_up(&mut self, tick: i64) -> i64 {
        let now = clock::now();
        if now - tick < self.interval {
            return tick;
        }
        self.next = now + self.interval;
        now
    }

    fn wait(&mut self) -> i64 {
        let now = clock::now();
        if self.next > now {
            let due = -(self.next - now);
            if unsafe { SetWaitableTimer(self.timer, &due, 0, None, None, false) }.is_ok() {
                unsafe { WaitForSingleObject(self.timer, INFINITE) };
            }
        } else if now - self.next > self.interval * MAX_LAG_FRAMES {
            self.next = now;
        }
        let tick = self.next;
        self.next += self.interval;
        tick
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.timer) };
    }
}

struct Counters {
    window: Instant,
    presents: u32,
    encoded: u32,
    dropped: u32,
}

impl Counters {
    fn new() -> Self {
        Self {
            window: Instant::now(),
            presents: 0,
            encoded: 0,
            dropped: 0,
        }
    }

    fn publish(&mut self, shared: &Shared, active: bool) {
        let elapsed = self.window.elapsed();
        if elapsed < STATS_INTERVAL {
            return;
        }
        *shared.pulse.lock() = Some((Instant::now(), active));
        let scale = |count: u32| (f64::from(count) / elapsed.as_secs_f64()).round() as u32;
        let fresh = shared.ring.try_lock().map(|ring| ring.buffered());
        let mut stats = shared.stats.lock();
        let buffered = fresh.unwrap_or(stats.buffered);
        *stats = Stats {
            display_fps: scale(self.presents),
            encoded_fps: scale(self.encoded),
            dropped: self.dropped,
            buffered,
        };
        *self = Self::new();
    }
}

fn snapshot_requests(shared: &Shared, gpu: &Gpu, texture: &ID3D11Texture2D) -> Result<()> {
    let (screenshot, thumbnail) = {
        let mut requests = shared.requests.lock();
        (requests.screenshot.take(), requests.thumbnail.take())
    };
    if screenshot.is_none() && thumbnail.is_none() {
        return Ok(());
    }
    let image = Arc::new(Image::capture(&gpu.device, &gpu.context, texture)?);
    let saved = screenshot.clone();
    let targets = [(screenshot, None), (thumbnail, Some(THUMBNAIL_WIDTH))];
    for (path, max_width) in targets {
        let Some(path) = path else {
            continue;
        };
        let image = image.clone();
        thread::Builder::new()
            .name("rewind-snapshot".to_owned())
            .spawn(move || {
                image
                    .save(&[(&path, max_width)])
                    .traced(AREA, "save a screenshot or thumbnail")
                    .ok();
            })?;
    }
    if let Some(path) = saved {
        shared.emit(RecorderEvent::ScreenshotSaved(path));
    }
    Ok(())
}

fn wanted(shared: &Shared) -> bool {
    shared.encoding() || shared.monitor.load(Ordering::Relaxed) || pending_snapshot(shared)
}

fn pending_snapshot(shared: &Shared) -> bool {
    let requests = shared.requests.lock();
    requests.screenshot.is_some() || requests.thumbnail.is_some()
}

enum Source {
    Desktop(Duplicator),
    Window(WindowCapture),
}

impl Source {
    fn open(shared: &Shared, gpu: &Gpu) -> Result<Self> {
        if shared.config.window_only
            && let Some(window) = foreground::game_window()
            && *shared.failed_window.lock() != Some(window.0 as isize)
        {
            match WindowCapture::new(&gpu.device, window)
                .traced(AREA, "capture only the game window")
            {
                Ok(capture) => {
                    trace!(AREA, "capturing only the game window {:?}", window.0);
                    return Ok(Self::Window(capture));
                }
                Err(_) => {
                    trace!(
                        AREA,
                        "window {:?} cannot be captured alone, using the whole screen for it",
                        window.0
                    );
                    *shared.failed_window.lock() = Some(window.0 as isize);
                }
            }
        }
        trace!(AREA, "capturing the whole screen");
        Ok(Self::Desktop(
            Duplicator::new(&gpu.device, &gpu.output).traced(AREA, "start desktop duplication")?,
        ))
    }

    fn size(&self) -> (u32, u32) {
        match self {
            Self::Desktop(duplicator) => (duplicator.width, duplicator.height),
            Self::Window(capture) => (capture.width, capture.height),
        }
    }

    fn acquire(&mut self, wait: Duration) -> Result<Option<Acquired>> {
        match self {
            Self::Desktop(duplicator) => duplicator
                .acquire(u32::try_from(wait.as_millis()).unwrap_or(u32::MAX))
                .map(Some),
            Self::Window(capture) => capture.acquire(),
        }
    }

    fn release(&mut self) {
        match self {
            Self::Desktop(duplicator) => duplicator.release(),
            Self::Window(capture) => capture.release(),
        }
    }

    fn outdated(&self, shared: &Shared) -> bool {
        if !shared.config.window_only {
            return false;
        }
        match self {
            Self::Window(capture) => !capture.is_open(),
            Self::Desktop(_) => foreground::game_window()
                .is_some_and(|window| *shared.failed_window.lock() != Some(window.0 as isize)),
        }
    }

    const fn is_window(&self) -> bool {
        matches!(self, Self::Window(_))
    }
}

fn video_session(shared: &Shared) -> Result<()> {
    trace!(AREA, "video session starting");
    let _runtime = Runtime::enter().traced(AREA, "enter the COM runtime for video")?;
    let config = &shared.config;
    let gpu = Gpu::open(config.screen).traced(AREA, "open the graphics adapter")?;
    let mut capture = Source::open(shared, &gpu)?;
    let source = capture.size();
    let target = fit(source, config.height);
    trace!(AREA, "capture size {source:?}, encoded size {target:?}");
    let mut converter = Converter::new(&gpu.device, &gpu.context, source, target, config.fps)
        .traced(AREA, "create the colour converter")?;
    let mut encoder = Encoder::new(
        &gpu.device,
        gpu.luid,
        config.codec,
        target,
        config.fps,
        config.bitrate,
    )
    .traced(AREA, "create the video encoder")?;
    shared.failure.lock().take();
    shared.emit(RecorderEvent::Encoder(encoder.name.clone()));
    let previous = shared.video_format.lock().clone();
    if previous.is_some_and(|format| (format.width, format.height) != target) {
        shared.ring.lock().clear();
    }

    let mut pacer = Pacer::new(config.fps).traced(AREA, "create the frame timer")?;
    let mut counters = Counters::new();
    let mut last: Option<ID3D11Texture2D> = None;
    let mut force_keyframe = true;
    let mut allowed = true;
    let mut checked = Instant::now() - FOREGROUND_CHECK;
    let mut fresh = Instant::now();
    let mut published = false;
    let mut last_encoded = Instant::now() - IDLE_FRAME;
    let mut last_key = Instant::now() - KEY_INTERVAL;
    let mut retry = false;

    let frame_interval = Duration::from_secs(1) / config.fps.max(1);

    while !shared.stop.load(Ordering::Relaxed) {
        let mut fresh_frame = false;
        let encoding = shared.encoding();
        if !wanted(shared) {
            trace!(AREA, "video session idle, nothing needs frames");
            return Ok(());
        }
        let mut pts = pacer.wait();
        if checked.elapsed() >= FOREGROUND_CHECK {
            checked = Instant::now();
            if capture.outdated(shared) {
                trace!(
                    AREA,
                    "the captured window changed, reopening the video session"
                );
                return Ok(());
            }
            let now_allowed =
                capture.is_window() || config.capture_desktop || foreground::is_fullscreen_app();
            if now_allowed && !allowed {
                force_keyframe = true;
            }
            if now_allowed != allowed {
                trace!(AREA, "capture allowed is now {now_allowed}");
            }
            allowed = now_allowed;
        }
        let monitor = shared.monitor.load(Ordering::Relaxed);
        let wait =
            if capture.is_window() || !(encoding && allowed || monitor) || pending_snapshot(shared)
            {
                Duration::ZERO
            } else if !encoder.settled() {
                frame_interval
            } else if encoding && allowed && last.is_some() {
                IDLE_DELAY.min(IDLE_FRAME.saturating_sub(last_encoded.elapsed()))
            } else {
                IDLE_DELAY
            };
        let Some(acquired) = capture.acquire(wait).traced(AREA, "acquire a frame")? else {
            trace!(
                AREA,
                "the captured window closed, reopening the video session"
            );
            return Ok(());
        };
        if !wait.is_zero() {
            pts = pacer.catch_up(pts);
        }
        match acquired {
            Acquired::Frame { texture, presents } => {
                fresh = Instant::now();
                counters.presents += presents;
                snapshot_requests(shared, &gpu, &texture)
                    .traced(AREA, "take a screenshot or thumbnail of the frame")?;
                if encoding && allowed {
                    if retry {
                        counters.dropped += 1;
                    }
                    last = Some(
                        converter
                            .convert(&texture)
                            .traced(AREA, "convert a frame for the encoder")?,
                    );
                    fresh_frame = true;
                }
            }
            Acquired::Timeout => {
                if !capture.is_window() && pending_snapshot(shared) && fresh.elapsed() > STALE_FRAME
                {
                    capture.release();
                    drop(capture);
                    trace!(
                        AREA,
                        "no fresh frame for a pending snapshot, restarting duplication"
                    );
                    capture = Source::Desktop(
                        Duplicator::new(&gpu.device, &gpu.output)
                            .traced(AREA, "restart desktop duplication")?,
                    );
                    fresh = Instant::now();
                }
            }
        }
        capture.release();
        if !(encoding && allowed) {
            encoder
                .poll(&mut |packet| shared.push(packet))
                .traced(AREA, "collect encoder output")?;
            last = None;
            retry = false;
            counters.publish(shared, false);
            if !shared.monitor.load(Ordering::Relaxed) && !pending_snapshot(shared) {
                thread::sleep(IDLE_DELAY);
            }
            continue;
        }
        let Some(frame) = &last else {
            continue;
        };
        if !fresh_frame && !retry && last_encoded.elapsed() < IDLE_FRAME {
            encoder
                .poll(&mut |packet| shared.push(packet))
                .traced(AREA, "collect encoder output")?;
            counters.publish(shared, true);
            continue;
        }
        encoder
            .poll(&mut |packet| shared.push(packet))
            .traced(AREA, "collect encoder output")?;
        let busy_since = Instant::now();
        while !encoder.ready() && busy_since.elapsed() < frame_interval {
            thread::sleep(ENCODER_POLL);
            encoder
                .poll(&mut |packet| shared.push(packet))
                .traced(AREA, "collect encoder output")?;
        }
        let requested = std::mem::take(&mut shared.requests.lock().keyframe);
        let keyframe = force_keyframe || requested || last_key.elapsed() >= KEY_INTERVAL;
        if encoder
            .encode(frame, pts, keyframe, &mut |packet| shared.push(packet))
            .traced(AREA, "encode a frame")?
        {
            counters.encoded += 1;
            retry = false;
            force_keyframe = false;
            last_encoded = Instant::now();
            if keyframe {
                last_key = last_encoded;
            }
        } else {
            retry = true;
        }
        if !published && let Some(format) = encoder.format() {
            trace!(
                AREA,
                "first encoded frame, video format {}x{}", format.width, format.height
            );
            *shared.video_format.lock() = Some(format);
            published = true;
        }
        counters.publish(shared, true);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spool_never_eats_the_reserve() {
        let gib = 1024 * 1024 * 1024;
        assert_eq!(spool_cap(5 * gib, Some(100 * gib)), 5 * gib);
        assert_eq!(spool_cap(500 * gib, Some(100 * gib)), 90 * gib);
        assert_eq!(spool_cap(5 * gib, Some(4 * gib)), 0);
        assert_eq!(spool_cap(5 * gib, None), 5 * gib);
        assert!(free_space(&std::env::temp_dir()).is_some_and(|free| free > 0));
    }

    #[test]
    fn a_disk_below_the_reserve_keeps_the_replay_in_memory() {
        let gib = 1024 * 1024 * 1024;
        let needed = replay_bytes(130_000_000, Duration::from_secs(300));
        let plan = ring_plan(needed, Some(8 * gib), Some(7 * gib));
        assert!(!plan.on_disk);
        assert_eq!(plan.bytes, 7 * gib / MEMORY_SHARE);
        let roomy = ring_plan(needed, Some(100 * gib), Some(7 * gib));
        assert_eq!(
            roomy,
            RingPlan {
                bytes: needed,
                on_disk: true
            }
        );
        let small = ring_plan(gib, Some(8 * gib), Some(32 * gib));
        assert_eq!(
            small,
            RingPlan {
                bytes: gib,
                on_disk: false
            }
        );
        assert_eq!(ring_plan(needed, None, None).bytes, needed);
        assert!(free_memory().is_some_and(|free| free > 0));
    }

    #[test]
    fn a_full_disk_still_buffers_the_whole_replay() {
        use crate::format::Track;
        let frame = HNS_PER_SECOND / 10;
        let bytes_per_frame = 1_000usize;
        let replay = Duration::from_secs(30);
        let bitrate = (bytes_per_frame * 10 * 8) as u32;
        let needed = replay_bytes(bitrate, replay);
        let disk_free = Some(DISK_RESERVE - 1);
        let plan = ring_plan(needed, disk_free, Some(needed * MEMORY_SHARE));
        let directory = tempfile::tempdir().unwrap();
        let mut ring = Ring::new(
            replay,
            plan.bytes as usize,
            1,
            plan.on_disk.then_some(directory.path()),
        );
        let fill = |ring: &mut Ring| {
            for index in 0..600i64 {
                ring.push(Packet {
                    track: Track::Video,
                    pts: index * frame,
                    duration: frame,
                    keyframe: index % 10 == 0,
                    data: Arc::from(vec![0u8; bytes_per_frame]),
                });
            }
        };
        fill(&mut ring);
        assert!(ring.buffered() >= Duration::from_secs(29));
        let clip = ring.snapshot(replay).unwrap();
        assert!(clip.video.len() >= 290);
        let mut starved = Ring::new(replay, spool_cap(needed, disk_free) as usize, 1, None);
        fill(&mut starved);
        assert!(starved.buffered() < Duration::from_secs(1));
    }

    fn pulse(age: u64, active: bool, encoded_fps: u32) -> Option<Pulse> {
        Some(Pulse {
            age: Duration::from_secs(age),
            active,
            encoded_fps,
        })
    }

    const LONG: Duration = Duration::from_secs(60);

    #[test]
    fn each_layout_has_its_track_count() {
        assert_eq!(AudioLayout::Single.tracks(), 1);
        assert_eq!(AudioLayout::Separate.tracks(), 3);
    }

    #[test]
    fn the_replay_estimate_covers_three_audio_tracks() {
        let ten = Duration::from_secs(10);
        let audio_only = replay_bytes(0, ten);
        assert!(audio_only as f64 >= 3.0 * 24_000.0 * 10.0);
    }

    #[test]
    fn replay_switched_off_is_off() {
        assert_eq!(
            classify(false, LONG, pulse(0, true, 30), None),
            ReplayHealth::Off
        );
    }

    #[test]
    fn fresh_encoding_pulse_is_running() {
        assert_eq!(
            classify(true, LONG, pulse(1, true, 30), None),
            ReplayHealth::Running
        );
    }

    #[test]
    fn nothing_to_capture_is_waiting_not_broken() {
        assert_eq!(
            classify(true, LONG, pulse(1, false, 0), None),
            ReplayHealth::Waiting
        );
    }

    #[test]
    fn a_new_recorder_gets_a_grace_period() {
        assert_eq!(
            classify(true, Duration::from_secs(1), None, None),
            ReplayHealth::Starting
        );
    }

    #[test]
    fn a_silent_capture_thread_is_stalled_with_its_reason() {
        let reason = Some("no hardware video encoder".to_owned());
        assert_eq!(
            classify(true, LONG, None, reason.clone()),
            ReplayHealth::Stalled(reason.clone())
        );
        assert_eq!(
            classify(true, LONG, pulse(30, true, 30), reason.clone()),
            ReplayHealth::Stalled(reason)
        );
    }

    #[test]
    fn an_encoder_that_stops_producing_is_stalled() {
        assert_eq!(
            classify(true, LONG, pulse(1, true, 0), None),
            ReplayHealth::Stalled(None)
        );
    }
}
