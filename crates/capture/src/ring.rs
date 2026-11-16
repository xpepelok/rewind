use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::error::{CaptureError, Result};
use crate::format::{HNS_PER_SECOND, Packet, Track};

const AUDIO_SLACK_HNS: i64 = HNS_PER_SECOND;
const SPOOL_RETRY_HNS: i64 = 5 * HNS_PER_SECOND;
const SPOOL_PREFIX: &str = "Rewind.replay";

#[cfg(windows)]
const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
#[cfg(windows)]
const FILE_SHARE_ALL: u32 = 0x7;

trait Storage: Send + Sync {
    fn read_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<usize>;
    fn write_at(&self, buffer: &[u8], offset: u64) -> io::Result<usize>;
    fn truncate(&self, len: u64) -> io::Result<()>;
}

impl Storage for File {
    #[cfg(windows)]
    fn read_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
        std::os::windows::fs::FileExt::seek_read(self, buffer, offset)
    }

    #[cfg(windows)]
    fn write_at(&self, buffer: &[u8], offset: u64) -> io::Result<usize> {
        std::os::windows::fs::FileExt::seek_write(self, buffer, offset)
    }

    #[cfg(unix)]
    fn read_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
        std::os::unix::fs::FileExt::read_at(self, buffer, offset)
    }

    #[cfg(unix)]
    fn write_at(&self, buffer: &[u8], offset: u64) -> io::Result<usize> {
        std::os::unix::fs::FileExt::write_at(self, buffer, offset)
    }

    fn truncate(&self, len: u64) -> io::Result<()> {
        self.set_len(len)
    }
}

fn read_exact_at(file: &dyn Storage, mut buffer: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !buffer.is_empty() {
        match file.read_at(buffer, offset)? {
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            read => {
                buffer = &mut buffer[read..];
                offset += read as u64;
            }
        }
    }
    Ok(())
}

fn write_all_at(file: &dyn Storage, mut buffer: &[u8], mut offset: u64) -> io::Result<()> {
    while !buffer.is_empty() {
        match file.write_at(buffer, offset)? {
            0 => return Err(io::ErrorKind::WriteZero.into()),
            written => {
                buffer = &buffer[written..];
                offset += written as u64;
            }
        }
    }
    Ok(())
}

static SPOOL_SERIAL: AtomicU64 = AtomicU64::new(0);

fn open_spool(directory: &Path) -> io::Result<File> {
    std::fs::create_dir_all(directory)?;
    let serial = SPOOL_SERIAL.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(format!(
        "{SPOOL_PREFIX}.{}.{serial}.tmp",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
            .share_mode(FILE_SHARE_ALL);
    }
    let file = options.open(&path)?;
    #[cfg(unix)]
    std::fs::remove_file(&path).ok();
    Ok(file)
}

struct Spool {
    file: Arc<dyn Storage>,
    capacity: u64,
    head: Arc<AtomicU64>,
}

impl Spool {
    fn new(file: Arc<dyn Storage>, capacity: u64) -> Self {
        Self {
            file,
            capacity: capacity.max(1),
            head: Arc::new(AtomicU64::new(0)),
        }
    }

    fn write(&self, data: &[u8]) -> io::Result<u64> {
        let at = self.head.load(Ordering::Acquire);
        let end = at + data.len() as u64;
        self.head.store(end, Ordering::Release);
        let offset = at % self.capacity;
        let first = ((self.capacity - offset) as usize).min(data.len());
        let written = write_all_at(&*self.file, &data[..first], offset).and_then(|()| {
            if first < data.len() {
                write_all_at(&*self.file, &data[first..], 0)
            } else {
                Ok(())
            }
        });
        match written {
            Ok(()) => Ok(at),
            Err(error) => {
                if end <= self.capacity {
                    self.head.store(at, Ordering::Release);
                    self.file.truncate(at).ok();
                }
                Err(error)
            }
        }
    }

    fn reader(&self) -> SpoolReader {
        SpoolReader {
            file: self.file.clone(),
            capacity: self.capacity,
            head: self.head.clone(),
        }
    }
}

#[derive(Clone)]
pub struct SpoolReader {
    file: Arc<dyn Storage>,
    capacity: u64,
    head: Arc<AtomicU64>,
}

impl SpoolReader {
    fn read(&self, at: u64, data: &mut [u8]) -> Result<()> {
        let offset = at % self.capacity;
        let first = ((self.capacity - offset) as usize).min(data.len());
        read_exact_at(&*self.file, &mut data[..first], offset)?;
        if first < data.len() {
            read_exact_at(&*self.file, &mut data[first..], 0)?;
        }
        let head = self.head.load(Ordering::Acquire);
        if head.saturating_sub(self.capacity) > at {
            return Err(CaptureError::Empty);
        }
        Ok(())
    }
}

#[derive(Clone)]
enum Payload {
    Memory(Arc<[u8]>),
    Disk { at: u64, len: usize },
}

impl Payload {
    fn len(&self) -> usize {
        match self {
            Self::Memory(data) => data.len(),
            Self::Disk { len, .. } => *len,
        }
    }

    fn resident(&self) -> usize {
        match self {
            Self::Memory(data) => data.len(),
            Self::Disk { .. } => 0,
        }
    }
}

#[derive(Clone)]
pub struct Frame {
    pub pts: i64,
    pub duration: i64,
    pub keyframe: bool,
    payload: Payload,
}

pub struct Clip {
    pub start: i64,
    pub video: Vec<Frame>,
    pub audio: Vec<Vec<Frame>>,
    reader: Option<SpoolReader>,
}

impl Frame {
    pub fn size(&self) -> usize {
        self.payload.len()
    }
}

impl Clip {
    pub fn fill(&self, frame: &Frame, into: &mut [u8]) -> Result<()> {
        if into.len() != frame.size() {
            return Err(CaptureError::Empty);
        }
        match (&frame.payload, &self.reader) {
            (Payload::Memory(data), _) => {
                into.copy_from_slice(data);
                Ok(())
            }
            (Payload::Disk { at, .. }, Some(reader)) => reader.read(*at, into),
            (Payload::Disk { .. }, None) => Err(CaptureError::Empty),
        }
    }
}

fn oldest_on_disk(frames: &VecDeque<Frame>) -> Option<u64> {
    frames.iter().find_map(|frame| match frame.payload {
        Payload::Disk { at, .. } => Some(at),
        Payload::Memory(_) => None,
    })
}

type MemoryBudget = Box<dyn Fn() -> u64 + Send>;

pub struct Ring {
    window: i64,
    cap_bytes: usize,
    bytes: usize,
    resident: usize,
    resident_cap: usize,
    directory: Option<PathBuf>,
    spool: Option<Spool>,
    retry_at: Option<i64>,
    budget: MemoryBudget,
    video: VecDeque<Frame>,
    audio: Vec<VecDeque<Frame>>,
}

fn hns(duration: Duration) -> i64 {
    (duration.as_nanos() / 100).min(i64::MAX as u128) as i64
}

impl Ring {
    pub fn new(
        window: Duration,
        cap_bytes: usize,
        audio_tracks: usize,
        spool: Option<&Path>,
    ) -> Self {
        let directory = spool.map(Path::to_path_buf);
        let spool = spool
            .and_then(|directory| open_spool(directory).ok())
            .map(|file| Spool::new(Arc::new(file), cap_bytes as u64));
        Self {
            window: hns(window),
            cap_bytes,
            bytes: 0,
            resident: 0,
            resident_cap: usize::MAX,
            directory,
            spool,
            retry_at: None,
            budget: Box::new(|| u64::MAX),
            video: VecDeque::with_capacity(4096),
            audio: (0..audio_tracks)
                .map(|_| VecDeque::with_capacity(4096))
                .collect(),
        }
    }

    pub fn with_memory_budget(mut self, budget: impl Fn() -> u64 + Send + 'static) -> Self {
        self.budget = Box::new(budget);
        self
    }

    pub fn push(&mut self, packet: Packet) {
        if let Track::Audio(index) = packet.track
            && index >= self.audio.len()
        {
            return;
        }
        if packet.track == Track::Video && self.video.is_empty() && !packet.keyframe {
            return;
        }
        let frame = self.store(&packet);
        self.bytes += frame.payload.len();
        self.resident += frame.payload.resident();
        match packet.track {
            Track::Video => self.video.push_back(frame),
            Track::Audio(index) => self.audio[index].push_back(frame),
        }
        self.evict();
    }

    fn store(&mut self, packet: &Packet) -> Frame {
        let payload = self
            .spill(packet)
            .unwrap_or_else(|| Payload::Memory(packet.data.clone()));
        Frame {
            pts: packet.pts,
            duration: packet.duration,
            keyframe: packet.keyframe,
            payload,
        }
    }

    fn spill(&mut self, packet: &Packet) -> Option<Payload> {
        if let Some(retry_at) = self.retry_at
            && packet.pts < retry_at
            && packet.pts >= retry_at.saturating_sub(SPOOL_RETRY_HNS)
        {
            return None;
        }
        if self.spool.is_none() {
            let directory = self.directory.as_deref()?;
            match open_spool(directory) {
                Ok(file) => self.spool = Some(Spool::new(Arc::new(file), self.cap_bytes as u64)),
                Err(_) => {
                    self.degrade(packet.pts);
                    return None;
                }
            }
        }
        let len = packet.data.len();
        let floor = self
            .spool
            .as_ref()
            .filter(|spool| (len as u64) < spool.capacity)
            .map(|spool| {
                (spool.head.load(Ordering::Acquire) + len as u64).saturating_sub(spool.capacity)
            })?;
        self.make_room(floor);
        let written = self.spool.as_ref()?.write(&packet.data);
        match written {
            Ok(at) => {
                self.retry_at = None;
                Some(Payload::Disk { at, len })
            }
            Err(_) => {
                self.degrade(packet.pts);
                None
            }
        }
    }

    fn degrade(&mut self, pts: i64) {
        self.retry_at = Some(pts.saturating_add(SPOOL_RETRY_HNS));
        self.resident_cap = usize::try_from((self.budget)()).unwrap_or(usize::MAX);
    }

    fn make_room(&mut self, floor: u64) {
        while oldest_on_disk(&self.video).is_some_and(|at| at < floor) {
            let next_key = self
                .video
                .iter()
                .skip(1)
                .position(|frame| frame.keyframe)
                .map_or(self.video.len(), |offset| offset + 1);
            self.drop_front(next_key);
        }
        for index in 0..self.audio.len() {
            while oldest_on_disk(&self.audio[index]).is_some_and(|at| at < floor) {
                if let Some(frame) = self.audio[index].pop_front() {
                    self.release(&frame);
                }
            }
        }
    }

    fn release(&mut self, frame: &Frame) {
        self.bytes -= frame.payload.len();
        self.resident -= frame.payload.resident();
    }

    fn drop_front(&mut self, count: usize) {
        let (bytes, resident) =
            self.video
                .drain(..count)
                .fold((0, 0), |(bytes, resident), frame| {
                    (
                        bytes + frame.payload.len(),
                        resident + frame.payload.resident(),
                    )
                });
        self.bytes -= bytes;
        self.resident -= resident;
    }

    pub fn clear(&mut self) {
        self.video.clear();
        self.audio.iter_mut().for_each(VecDeque::clear);
        self.bytes = 0;
        self.resident = 0;
    }

    pub fn buffered(&self) -> Duration {
        let (Some(first), Some(last)) = (self.video.front(), self.video.back()) else {
            return Duration::ZERO;
        };
        Duration::from_nanos(((last.pts - first.pts).max(0) as u64) * 100)
    }

    pub fn newest(&self) -> Option<i64> {
        self.video.back().map(|frame| frame.pts)
    }

    pub fn span(&self) -> Option<(i64, i64)> {
        Some((self.video.front()?.pts, self.video.back()?.pts))
    }

    pub fn snapshot(&self, length: Duration) -> Option<Clip> {
        let newest = self.newest()?;
        self.snapshot_range(newest - hns(length), i64::MAX)
    }

    pub fn keyframe_at(&self, time: i64) -> Option<Clip> {
        let index = self
            .video
            .iter()
            .rposition(|frame| frame.keyframe && frame.pts <= time)
            .or_else(|| self.video.iter().position(|frame| frame.keyframe))?;
        let frame = self.video[index].clone();
        Some(Clip {
            start: frame.pts,
            video: vec![frame],
            audio: Vec::new(),
            reader: self.spool.as_ref().map(Spool::reader),
        })
    }

    pub fn snapshot_range(&self, from: i64, to: i64) -> Option<Clip> {
        let start_index = self
            .video
            .iter()
            .rposition(|frame| frame.keyframe && frame.pts <= from)
            .or_else(|| self.video.iter().position(|frame| frame.keyframe))?;
        let end_index = self.video.iter().rposition(|frame| frame.pts <= to)?;
        if end_index < start_index {
            return None;
        }
        let start = self.video[start_index].pts;
        let last = &self.video[end_index];
        let live = end_index + 1 == self.video.len();
        let heard = self
            .audio
            .iter()
            .filter_map(|track| track.back().map(|frame| frame.pts + frame.duration))
            .max()
            .unwrap_or(i64::MIN);
        let end = if live {
            (last.pts + last.duration).max(heard.min(to))
        } else {
            last.pts + last.duration
        };
        let mut video: Vec<Frame> = self.video.range(start_index..=end_index).cloned().collect();
        if let Some(tail) = video.last_mut() {
            tail.duration = end - tail.pts;
        }
        Some(Clip {
            start,
            video,
            audio: self
                .audio
                .iter()
                .map(|track| {
                    let first = track.partition_point(|frame| frame.pts < start);
                    let last = track.partition_point(|frame| frame.pts < end);
                    track.range(first..last.max(first)).cloned().collect()
                })
                .collect(),
            reader: self.spool.as_ref().map(Spool::reader),
        })
    }

    fn evict(&mut self) {
        if let Some(newest) = self.video.back().map(|frame| frame.pts) {
            while let Some(next_key) = self
                .video
                .iter()
                .skip(1)
                .position(|frame| frame.keyframe)
                .map(|offset| offset + 1)
            {
                let covered = newest - self.video[next_key].pts >= self.window;
                if !covered && self.bytes <= self.cap_bytes && self.resident <= self.resident_cap {
                    break;
                }
                self.drop_front(next_key);
            }
        }
        let floor = match self.video.front() {
            Some(first) => first.pts,
            None => {
                let newest_audio = self
                    .audio
                    .iter()
                    .filter_map(|track| track.back().map(|frame| frame.pts))
                    .max()
                    .unwrap_or(0);
                newest_audio - self.window
            }
        } - AUDIO_SLACK_HNS;
        for index in 0..self.audio.len() {
            while self.audio[index]
                .front()
                .is_some_and(|frame| frame.pts + frame.duration < floor)
            {
                if let Some(frame) = self.audio[index].pop_front() {
                    self.release(&frame);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: i64 = HNS_PER_SECOND / 10;

    fn video(pts: i64, keyframe: bool) -> Packet {
        Packet {
            track: Track::Video,
            pts,
            duration: FRAME,
            keyframe,
            data: Arc::from(vec![(pts / FRAME) as u8; 100]),
        }
    }

    fn audio(pts: i64) -> Packet {
        Packet {
            track: Track::Audio(0),
            pts,
            duration: FRAME,
            keyframe: true,
            data: Arc::from(vec![0u8; 10]),
        }
    }

    fn read(clip: &Clip, frame: &Frame) -> Result<Vec<u8>> {
        let mut data = vec![0u8; frame.size()];
        clip.fill(frame, &mut data)?;
        Ok(data)
    }

    fn fill(ring: &mut Ring, seconds: i64) {
        for frame in 0..seconds * 10 {
            let pts = frame * FRAME;
            ring.push(video(pts, frame % 10 == 0));
            ring.push(audio(pts));
        }
    }

    #[test]
    fn keeps_at_least_the_window() {
        let mut ring = Ring::new(Duration::from_secs(3), usize::MAX, 1, None);
        fill(&mut ring, 10);
        assert!(ring.buffered() >= Duration::from_millis(2900));
        assert!(ring.buffered() < Duration::from_secs(5));
    }

    #[test]
    fn snapshot_starts_on_keyframe_and_covers_request() {
        let mut ring = Ring::new(Duration::from_secs(5), usize::MAX, 1, None);
        fill(&mut ring, 10);
        let clip = ring.snapshot(Duration::from_secs(2)).unwrap();
        assert!(clip.video[0].keyframe);
        let newest = clip.video.last().unwrap().pts;
        assert!(newest - clip.start >= 2 * HNS_PER_SECOND);
        assert!(clip.audio[0].iter().all(|frame| frame.pts >= clip.start));
    }

    #[test]
    fn byte_cap_drops_whole_gops() {
        let mut ring = Ring::new(Duration::from_secs(60), 2_500, 1, None);
        fill(&mut ring, 10);
        assert!(ring.video.front().unwrap().keyframe);
        assert!(ring.bytes <= 2_500 + 10 * 110);
    }

    #[test]
    fn clear_resets_everything() {
        let mut ring = Ring::new(Duration::from_secs(5), usize::MAX, 1, None);
        fill(&mut ring, 2);
        ring.clear();
        assert!(ring.snapshot(Duration::from_secs(1)).is_none());
        assert_eq!(ring.bytes, 0);
    }

    #[test]
    fn spooled_frames_round_trip_across_wraps() {
        let directory = tempfile::tempdir().unwrap();
        let mut ring = Ring::new(Duration::from_secs(3), 4_000, 1, Some(directory.path()));
        assert!(ring.spool.is_some());
        fill(&mut ring, 10);
        let clip = ring.snapshot(Duration::from_secs(2)).unwrap();
        assert!(clip.video[0].keyframe);
        for frame in &clip.video {
            let data = read(&clip, frame).unwrap();
            assert_eq!(data.len(), 100);
            assert!(data.iter().all(|byte| *byte == (frame.pts / FRAME) as u8));
        }
    }

    #[test]
    fn a_range_starts_on_a_keyframe_and_stops_at_its_end() {
        let mut ring = Ring::new(Duration::from_secs(60), usize::MAX, 1, None);
        fill(&mut ring, 300);
        let (first, last) = ring.span().unwrap();
        assert!(first < last);
        let from = first + 3 * HNS_PER_SECOND;
        let to = first + 5 * HNS_PER_SECOND;
        let clip = ring.snapshot_range(from, to).unwrap();
        assert!(clip.video[0].keyframe);
        assert!(clip.start <= from);
        assert!(clip.video.iter().all(|frame| frame.pts <= to));
        let end = clip
            .video
            .last()
            .map(|frame| frame.pts + frame.duration)
            .unwrap();
        assert!(
            clip.audio[0]
                .iter()
                .all(|frame| frame.pts >= clip.start && frame.pts < end)
        );
        assert!(ring.snapshot_range(last + 1, last + 2).is_some());
        assert!(ring.snapshot_range(first, first - 1).is_none());
    }

    #[test]
    fn a_live_clip_keeps_the_sound_after_the_last_frame() {
        let mut ring = Ring::new(Duration::from_secs(60), usize::MAX, 1, None);
        ring.push(video(0, true));
        for step in 0..20 {
            ring.push(audio(step * FRAME));
        }
        let clip = ring.snapshot(Duration::from_secs(10)).unwrap();
        let end = clip.video[0].pts + clip.video[0].duration;
        assert_eq!(end, 20 * FRAME);
        assert_eq!(clip.audio[0].len(), 20);
        let early = ring.snapshot_range(0, 5 * FRAME).unwrap();
        assert_eq!(early.video[0].duration, 5 * FRAME);
    }

    #[test]
    fn a_keyframe_preview_holds_one_frame_and_no_audio() {
        let mut ring = Ring::new(Duration::from_secs(60), usize::MAX, 1, None);
        fill(&mut ring, 10);
        let clip = ring.keyframe_at(5 * HNS_PER_SECOND + FRAME * 3).unwrap();
        assert_eq!(clip.video.len(), 1);
        assert!(clip.video[0].keyframe);
        assert_eq!(clip.video[0].pts, 5 * HNS_PER_SECOND);
        assert!(clip.audio.is_empty());
    }

    #[test]
    fn spooled_audio_round_trips_and_leaves_memory_empty() {
        let directory = tempfile::tempdir().unwrap();
        let mut ring = Ring::new(Duration::from_secs(3), 4_000, 1, Some(directory.path()));
        fill(&mut ring, 10);
        assert!(
            ring.audio[0]
                .iter()
                .all(|frame| matches!(frame.payload, Payload::Disk { .. }))
        );
        let clip = ring.snapshot(Duration::from_secs(2)).unwrap();
        assert!(!clip.audio[0].is_empty());
        for frame in &clip.audio[0] {
            assert_eq!(read(&clip, frame).unwrap().len(), 10);
        }
    }

    #[test]
    fn spool_never_overwrites_live_frames() {
        let directory = tempfile::tempdir().unwrap();
        let mut ring = Ring::new(Duration::from_secs(60), 1_150, 1, Some(directory.path()));
        fill(&mut ring, 5);
        let clip = ring.snapshot(Duration::from_secs(60)).unwrap();
        assert!(clip.video[0].keyframe);
        assert!(clip.video.iter().all(|frame| read(&clip, frame).is_ok()));
    }

    #[test]
    fn filling_needs_a_buffer_of_the_frame_size() {
        let mut ring = Ring::new(Duration::from_secs(60), usize::MAX, 1, None);
        fill(&mut ring, 2);
        let clip = ring.snapshot(Duration::from_secs(1)).unwrap();
        let frame = &clip.video[0];
        let mut short = vec![0u8; frame.size() - 1];
        assert!(clip.fill(frame, &mut short).is_err());
        let data = read(&clip, frame).unwrap();
        assert!(data.iter().all(|byte| *byte == (frame.pts / FRAME) as u8));
    }

    struct Faulty {
        file: File,
        quota: AtomicU64,
        attempts: AtomicU64,
    }

    impl Faulty {
        fn new(directory: &Path, quota: u64) -> Arc<Self> {
            Arc::new(Self {
                file: tempfile::tempfile_in(directory).unwrap(),
                quota: AtomicU64::new(quota),
                attempts: AtomicU64::new(0),
            })
        }

        fn len(&self) -> u64 {
            self.file.metadata().unwrap().len()
        }
    }

    impl Storage for Faulty {
        fn read_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
            Storage::read_at(&self.file, buffer, offset)
        }

        fn write_at(&self, buffer: &[u8], offset: u64) -> io::Result<usize> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            let allowed = self.quota.load(Ordering::Relaxed).min(buffer.len() as u64);
            if allowed == 0 {
                return Err(io::Error::other("disk full"));
            }
            self.quota.fetch_sub(allowed, Ordering::Relaxed);
            Storage::write_at(&self.file, &buffer[..allowed as usize], offset)
        }

        fn truncate(&self, len: u64) -> io::Result<()> {
            self.file.set_len(len)
        }
    }

    fn faulty_ring(directory: &Path, cap: usize, quota: u64, budget: u64) -> (Ring, Arc<Faulty>) {
        let faulty = Faulty::new(directory, quota);
        let mut ring =
            Ring::new(Duration::from_secs(60), cap, 1, None).with_memory_budget(move || budget);
        ring.spool = Some(Spool::new(faulty.clone(), cap as u64));
        (ring, faulty)
    }

    fn feed(ring: &mut Ring, frames: std::ops::Range<i64>) {
        for frame in frames {
            let pts = frame * FRAME;
            ring.push(video(pts, frame % 10 == 0));
            ring.push(audio(pts));
        }
    }

    fn resident(ring: &Ring) -> usize {
        ring.video
            .iter()
            .chain(ring.audio.iter().flatten())
            .map(|frame| frame.payload.resident())
            .sum()
    }

    fn on_disk(frame: &Frame) -> bool {
        matches!(frame.payload, Payload::Disk { .. })
    }

    fn intact(clip: &Clip) -> bool {
        clip.video
            .iter()
            .all(|frame| read(clip, frame).unwrap() == vec![(frame.pts / FRAME) as u8; 100])
            && clip.audio[0]
                .iter()
                .all(|frame| read(clip, frame).unwrap() == vec![0u8; 10])
    }

    #[test]
    fn a_failing_spool_keeps_memory_within_the_budget() {
        let directory = tempfile::tempdir().unwrap();
        let (mut ring, faulty) = faulty_ring(directory.path(), 1_000_000, 5_500, 5_000);
        feed(&mut ring, 0..50);
        assert!(ring.retry_at.is_none());
        let written = faulty.attempts.load(Ordering::Relaxed);
        feed(&mut ring, 50..300);
        assert!(ring.retry_at.is_some());
        assert_eq!(ring.resident, resident(&ring));
        assert!(ring.resident <= 5_000 + 2 * 1_100);
        assert!(ring.buffered() >= Duration::from_secs(3));
        assert!(faulty.attempts.load(Ordering::Relaxed) - written <= 6);
        let clip = ring.snapshot(Duration::from_secs(60)).unwrap();
        assert!(intact(&clip));
    }

    #[test]
    fn memory_eviction_drops_whole_keyframe_groups() {
        let directory = tempfile::tempdir().unwrap();
        let (mut ring, _faulty) = faulty_ring(directory.path(), 1_000_000, 5_500, 2_000);
        feed(&mut ring, 0..300);
        assert!(ring.video.front().unwrap().keyframe);
        assert!(
            ring.video
                .iter()
                .zip(ring.video.iter().skip(1))
                .all(|(earlier, later)| later.pts - earlier.pts == FRAME)
        );
        assert!(
            ring.audio[0].iter().all(|frame| frame.pts + frame.duration
                >= ring.video.front().unwrap().pts - AUDIO_SLACK_HNS)
        );
    }

    #[test]
    fn a_clip_spans_spooled_and_resident_frames_and_the_spool_comes_back() {
        let directory = tempfile::tempdir().unwrap();
        let (mut ring, faulty) = faulty_ring(directory.path(), 1_000_000, 5_550, u64::MAX);
        feed(&mut ring, 0..100);
        let head = ring.spool.as_ref().unwrap().head.load(Ordering::Acquire);
        assert_eq!(head, 5_500);
        assert_eq!(faulty.len(), head);
        assert!(ring.video.range(..50).all(on_disk));
        assert!(!ring.video.range(50..).any(on_disk));
        let degraded = ring.snapshot(Duration::from_secs(60)).unwrap();
        assert_eq!(degraded.video.len(), 100);
        assert!(intact(&degraded));
        faulty.quota.store(u64::MAX, Ordering::Relaxed);
        feed(&mut ring, 100..200);
        assert!(ring.retry_at.is_none());
        assert!(ring.video.range(100..).all(on_disk));
        let recovered = ring.snapshot(Duration::from_secs(60)).unwrap();
        assert_eq!(recovered.video.len(), 200);
        assert!(intact(&recovered));
        assert!(intact(&degraded));
    }

    #[test]
    fn a_torn_write_never_feeds_stale_bytes_to_a_clip() {
        let directory = tempfile::tempdir().unwrap();
        let (mut ring, faulty) = faulty_ring(directory.path(), 4_000, u64::MAX, u64::MAX);
        for frame in 0..100 {
            ring.push(video(frame * FRAME, true));
        }
        let before = ring.snapshot(Duration::from_secs(60)).unwrap();
        assert_eq!(before.video.len(), 40);
        assert!(intact(&before));
        faulty.quota.store(30, Ordering::Relaxed);
        ring.push(video(100 * FRAME, true));
        assert!(ring.retry_at.is_some());
        assert_eq!(faulty.len(), 4_000);
        let mut torn = 0;
        for frame in &before.video {
            match read(&before, frame) {
                Ok(data) => assert_eq!(data, vec![(frame.pts / FRAME) as u8; 100]),
                Err(_) => torn += 1,
            }
        }
        assert_eq!(torn, 1);
        let after = ring.snapshot(Duration::from_secs(60)).unwrap();
        assert!(!on_disk(after.video.last().unwrap()));
        assert!(intact(&after));
    }

    #[test]
    fn an_unopenable_spool_stays_bounded_and_opens_later() {
        let directory = tempfile::tempdir().unwrap();
        let blocked = directory.path().join("spool");
        std::fs::write(&blocked, b"").unwrap();
        let mut ring = Ring::new(Duration::from_secs(60), 1_000_000, 1, Some(&blocked))
            .with_memory_budget(|| 2_000);
        assert!(ring.spool.is_none());
        feed(&mut ring, 0..100);
        assert!(ring.resident <= 2_000 + 2 * 1_100);
        std::fs::remove_file(&blocked).unwrap();
        feed(&mut ring, 100..200);
        assert!(ring.spool.is_some());
        assert!(ring.video.back().is_some_and(on_disk));
        let clip = ring.snapshot(Duration::from_secs(60)).unwrap();
        assert!(clip.video.iter().any(on_disk));
        assert!(intact(&clip));
    }
}
