use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

use rewind_trace::{Traced, trace};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE,
    EDataFlow, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0, eCapture,
    eConsole, eRender,
};
use windows::Win32::Media::KernelStreaming::{
    SPEAKER_FRONT_LEFT, SPEAKER_FRONT_RIGHT, WAVE_FORMAT_EXTENSIBLE,
};
use windows::Win32::Media::MediaFoundation::{
    IMFMediaBuffer, IMFMediaType, IMFSample, IMFTransform, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, MF_MT_AAC_PAYLOAD_TYPE,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_MT_USER_DATA, MFAudioFormat_AAC, MFAudioFormat_PCM, MFCreateMediaType, MFCreateMemoryBuffer,
    MFCreateSample, MFMediaType_Audio, MFT_CATEGORY_AUDIO_ENCODER, MFT_ENUM_FLAG_SORTANDFILTER,
    MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_OUTPUT_DATA_BUFFER,
};
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};

use crate::clock;
use crate::com::{Runtime, first_transform};
use crate::discord::{self, Capture};
use crate::encoder::blob;
use crate::error::{CaptureError, Result};
use crate::format::{AudioFormat, HNS_PER_SECOND, Packet, Track};

pub const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u32 = 2;
const AAC_BYTES_PER_SECOND: u32 = 24_000;
pub const AAC_PROFILE_LEVEL: u32 = 0x29;
const CLIENT_BUFFER_HNS: i64 = HNS_PER_SECOND / 10;
const LATENCY_HNS: i64 = HNS_PER_SECOND / 20;
const TICK: Duration = Duration::from_millis(20);
const DEVICE_CHECK_TICKS: u32 = 100;
const DISCORD_CHECK_TICKS: u32 = 150;
const DISCORD_SEARCH_CHECKS: u32 = 3;
const DISCORD_REVIEW_CHECKS: u32 = 10;
const SEPARATE_TRACKS: usize = 3;
const MAX_BACKLOG_FRAMES: usize = (SAMPLE_RATE / 5) as usize;
const KEPT_SAMPLES: usize = (SAMPLE_RATE / 20 * CHANNELS) as usize;
const SLACK_SAMPLES: usize = 2 * KEPT_SAMPLES;
const LATE_SAMPLES: usize = 4 * KEPT_SAMPLES;
const QUIET_LEVEL: f32 = 0.001;
const AAC_FRAME: i64 = 1024;
const SETTLED_FRAMES: i64 = 4 * AAC_FRAME;
const QUIET_AFTER_FRAMES: i64 = (SAMPLE_RATE / 2) as i64;

pub struct AudioControls {
    system_gain: AtomicU32,
    mic_gain: AtomicU32,
    mic_enabled: AtomicBool,
    mic_gate: Box<dyn Fn() -> bool + Send + Sync>,
}

impl AudioControls {
    pub fn new(
        system_gain: f32,
        mic_gain: f32,
        mic_gate: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            system_gain: AtomicU32::new(system_gain.to_bits()),
            mic_gain: AtomicU32::new(mic_gain.to_bits()),
            mic_enabled: AtomicBool::new(true),
            mic_gate: Box::new(mic_gate),
        }
    }

    pub fn set_gains(&self, system: f32, mic: f32) {
        self.system_gain.store(system.to_bits(), Ordering::Relaxed);
        self.mic_gain.store(mic.to_bits(), Ordering::Relaxed);
    }

    pub fn set_mic_enabled(&self, enabled: bool) {
        self.mic_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn mic_enabled(&self) -> bool {
        self.mic_enabled.load(Ordering::Relaxed)
    }

    fn system(&self) -> f32 {
        f32::from_bits(self.system_gain.load(Ordering::Relaxed))
    }

    fn mic(&self) -> f32 {
        if self.mic_enabled() && (self.mic_gate)() {
            f32::from_bits(self.mic_gain.load(Ordering::Relaxed))
        } else {
            0.0
        }
    }
}

fn enumerator() -> Result<IMMDeviceEnumerator> {
    Ok(unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?)
}

fn device_name(device: &IMMDevice) -> Option<String> {
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }.ok()?;
    let value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.ok()?;
    let text = unsafe { PropVariantToStringAlloc(&value) }.ok()?;
    let name = unsafe { text.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(text.0 as *const _)) };
    name
}

fn device_id(device: &IMMDevice) -> Option<String> {
    let id = unsafe { device.GetId() }.ok()?;
    let text = unsafe { id.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(id.0 as *const _)) };
    text
}

fn endpoints(enumerator: &IMMDeviceEnumerator, flow: EDataFlow) -> Vec<IMMDevice> {
    let Ok(collection) = (unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) })
    else {
        return Vec::new();
    };
    let count = unsafe { collection.GetCount() }.unwrap_or(0);
    (0..count)
        .filter_map(|index| unsafe { collection.Item(index) }.ok())
        .collect()
}

pub fn microphones() -> Vec<String> {
    let Ok(_runtime) = Runtime::enter() else {
        return Vec::new();
    };
    let Ok(enumerator) = enumerator() else {
        return Vec::new();
    };
    endpoints(&enumerator, eCapture)
        .iter()
        .filter_map(device_name)
        .collect()
}

fn capture_format() -> WAVEFORMATEXTENSIBLE {
    let block = (CHANNELS * 4) as u16;
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: CHANNELS as u16,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * u32::from(block),
            nBlockAlign: block,
            wBitsPerSample: 32,
            cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: 32,
        },
        dwChannelMask: SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT,
        SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
    }
}

struct Source {
    _client: IAudioClient,
    capture: IAudioCaptureClient,
    id: Option<String>,
    fifo: VecDeque<f32>,
}

impl Source {
    fn open(device: &IMMDevice, loopback: bool) -> Result<Self> {
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }?;
        Self::start(client, loopback, device_id(device))
    }

    fn process(process: u32, capture: Capture, id: Option<String>) -> Result<Self> {
        Self::start(discord::client(process, capture)?, true, id)
    }

    fn start(client: IAudioClient, loopback: bool, id: Option<String>) -> Result<Self> {
        let format = capture_format();
        let mut flags =
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        if loopback {
            flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
        }
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                CLIENT_BUFFER_HNS,
                0,
                &format.Format,
                None,
            )
        }?;
        let capture: IAudioCaptureClient = unsafe { client.GetService() }?;
        unsafe { client.Start() }?;
        Ok(Self {
            _client: client,
            capture,
            id,
            fifo: VecDeque::with_capacity(SAMPLE_RATE as usize),
        })
    }

    fn read(&mut self) -> Result<()> {
        while unsafe { self.capture.GetNextPacketSize() }? > 0 {
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            unsafe {
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
            }?;
            let count = (frames * CHANNELS) as usize;
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                self.fifo.extend(std::iter::repeat_n(0.0, count));
            } else {
                let samples = unsafe { std::slice::from_raw_parts(data as *const f32, count) };
                self.fifo.extend(samples);
            }
            unsafe { self.capture.ReleaseBuffer(frames) }?;
        }
        let limit = MAX_BACKLOG_FRAMES * CHANNELS as usize;
        if self.fifo.len() > limit * 2 {
            let excess = self.fifo.len() - limit;
            self.fifo.drain(..excess - excess % CHANNELS as usize);
        }
        Ok(())
    }

    fn flush(&mut self) {
        let _ = self.read();
        self.fifo.clear();
    }
}

const AREA: &str = "audio";

fn catch_up(fifo: &mut VecDeque<f32>) {
    if fifo.len() <= SLACK_SAMPLES {
        return;
    }
    let excess = fifo.len() - KEPT_SAMPLES;
    let excess = excess - excess % CHANNELS as usize;
    let quiet = || {
        fifo.iter()
            .take(excess)
            .all(|sample| sample.abs() < QUIET_LEVEL)
    };
    let late = fifo.len() > LATE_SAMPLES;
    if late || quiet() {
        if late {
            trace!(
                AREA,
                "captured sound is {} samples behind, dropping {excess} of them",
                fifo.len()
            );
        }
        fifo.drain(..excess);
    }
}

fn take(source: Option<&mut Source>, count: usize, gain: f32, into: &mut Vec<f32>) {
    let mut fifo = source.map(|source| &mut source.fifo);
    drain(fifo.as_deref_mut(), count, gain, into);
    if let Some(fifo) = fifo {
        catch_up(fifo);
    }
}

fn drain(fifo: Option<&mut VecDeque<f32>>, count: usize, gain: f32, into: &mut Vec<f32>) {
    into.clear();
    if let Some(fifo) = fifo {
        let ready = count.min(fifo.len());
        into.extend(fifo.drain(..ready));
    }
    into.resize(count, 0.0);
    into.iter_mut().for_each(|sample| *sample *= gain);
}

#[derive(Default)]
struct Quiet {
    frame: Option<Arc<[u8]>>,
    previous: Option<Arc<[u8]>>,
    zeros: i64,
    owed: i64,
    skipping: bool,
}

impl Quiet {
    fn heard(&mut self, silent: bool, frames: i64) {
        if silent {
            self.zeros += frames;
        } else {
            self.zeros = 0;
            self.previous = None;
        }
    }

    fn learn(&mut self, data: &Arc<[u8]>) {
        if self.frame.is_some() || self.zeros < SETTLED_FRAMES {
            return;
        }
        if self.previous.as_ref() == Some(data) {
            self.frame = Some(data.clone());
        }
        self.previous = Some(data.clone());
    }

    fn settle(&mut self) {
        self.skipping = self.frame.is_some() && self.zeros >= QUIET_AFTER_FRAMES;
    }

    fn skip(&mut self, frames: i64) -> impl Iterator<Item = Arc<[u8]>> + use<> {
        self.owed += frames;
        let whole = self.owed / AAC_FRAME;
        self.owed %= AAC_FRAME;
        let frame = self.frame.clone();
        (0..whole).filter_map(move |_| frame.clone())
    }

    fn resume(&mut self) -> usize {
        self.skipping = false;
        std::mem::take(&mut self.owed) as usize
    }
}

fn frame_pts(origin: i64, index: i64) -> i64 {
    origin + index * AAC_FRAME * HNS_PER_SECOND / i64::from(SAMPLE_RATE)
}

struct Aac {
    transform: IMFTransform,
    track: usize,
    output_size: u32,
    output: Option<(IMFSample, IMFMediaBuffer)>,
    format: AudioFormat,
    quiet: Quiet,
    fed: i64,
    started: Option<i64>,
    origin: Option<i64>,
    index: i64,
}

fn pcm_type() -> Result<IMFMediaType> {
    let media_type = unsafe { MFCreateMediaType() }?;
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        media_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)?;
        media_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, CHANNELS * 2)?;
        media_type.SetUINT32(
            &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
            SAMPLE_RATE * CHANNELS * 2,
        )?;
    }
    Ok(media_type)
}

fn aac_type() -> Result<IMFMediaType> {
    let media_type = unsafe { MFCreateMediaType() }?;
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        media_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)?;
        media_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)?;
        media_type.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES_PER_SECOND)?;
        media_type.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        media_type.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, AAC_PROFILE_LEVEL)?;
    }
    Ok(media_type)
}

impl Aac {
    fn new(track: usize) -> Result<Self> {
        let transform = first_transform(
            MFT_CATEGORY_AUDIO_ENCODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            (MFMediaType_Audio, MFAudioFormat_PCM),
            (MFMediaType_Audio, MFAudioFormat_AAC),
            None,
        )?
        .ok_or(CaptureError::NoAudioEncoder)?
        .transform;
        let input = pcm_type()?;
        let output = aac_type()?;
        let output_first = unsafe { transform.SetOutputType(0, &output, 0) }
            .and_then(|()| unsafe { transform.SetInputType(0, &input, 0) });
        if output_first.is_err() {
            unsafe {
                transform.SetInputType(0, &input, 0)?;
                transform.SetOutputType(0, &output, 0)?;
            }
        }
        let current = unsafe { transform.GetOutputCurrentType(0) }?;
        let format = AudioFormat {
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            bytes_per_second: unsafe { current.GetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND) }
                .unwrap_or(AAC_BYTES_PER_SECOND),
            user_data: blob(&current, &MF_MT_USER_DATA).unwrap_or_default(),
        };
        let info = unsafe { transform.GetOutputStreamInfo(0) }?;
        unsafe {
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        }
        Ok(Self {
            transform,
            track,
            output_size: info.cbSize.max(8192),
            output: None,
            format,
            quiet: Quiet::default(),
            fed: 0,
            started: None,
            origin: None,
            index: 0,
        })
    }

    fn packet(&mut self, origin: i64, data: Arc<[u8]>) -> Packet {
        let pts = frame_pts(origin, self.index);
        self.index += 1;
        Packet {
            track: Track::Audio(self.track),
            pts,
            duration: frame_pts(origin, self.index) - pts,
            keyframe: true,
            data,
        }
    }

    fn encode(&mut self, pcm: &[i16], pts: i64, push: &mut impl FnMut(Packet)) -> Result<()> {
        let frames = (pcm.len() / CHANNELS as usize) as i64;
        let silent = pcm.iter().all(|sample| *sample == 0);
        if silent
            && self.quiet.skipping
            && let Some(origin) = self.origin
        {
            for data in self.quiet.skip(frames) {
                push(self.packet(origin, data));
            }
            return Ok(());
        }
        if self.quiet.skipping {
            let owed = self.quiet.resume();
            self.feed(&vec![0; owed * CHANNELS as usize], pts, push)?;
        }
        self.quiet.heard(silent, frames);
        self.feed(pcm, pts, push)?;
        self.quiet.settle();
        Ok(())
    }

    fn feed(&mut self, pcm: &[i16], pts: i64, push: &mut impl FnMut(Packet)) -> Result<()> {
        if pcm.is_empty() {
            return Ok(());
        }
        let pts =
            self.fed * HNS_PER_SECOND / i64::from(SAMPLE_RATE) + *self.started.get_or_insert(pts);
        let bytes = std::mem::size_of_val(pcm) as u32;
        let buffer = unsafe { MFCreateMemoryBuffer(bytes) }?;
        let mut data = std::ptr::null_mut();
        unsafe { buffer.Lock(&mut data, None, None) }?;
        unsafe { std::ptr::copy_nonoverlapping(pcm.as_ptr() as *const u8, data, bytes as usize) };
        unsafe {
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes)?;
        }
        let frames = (pcm.len() / CHANNELS as usize) as i64;
        let sample = unsafe { MFCreateSample() }?;
        unsafe {
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            sample.SetSampleDuration(frames * HNS_PER_SECOND / i64::from(SAMPLE_RATE))?;
            self.transform.ProcessInput(0, &sample, 0)?;
        }
        self.fed += frames;
        while let Some((pts, data)) = self.drain()? {
            let origin = *self.origin.get_or_insert(pts);
            self.quiet.learn(&data);
            push(self.packet(origin, data));
        }
        Ok(())
    }

    fn drain(&mut self) -> Result<Option<(i64, Arc<[u8]>)>> {
        let (sample, buffer) = match self.output.take() {
            Some(reused) => reused,
            None => {
                let sample = unsafe { MFCreateSample() }?;
                let buffer = unsafe { MFCreateMemoryBuffer(self.output_size) }?;
                unsafe { sample.AddBuffer(&buffer) }?;
                (sample, buffer)
            }
        };
        unsafe { buffer.SetCurrentLength(0) }?;
        let mut output = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(Some(sample.clone())),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        }];
        let mut status = 0u32;
        let result = unsafe { self.transform.ProcessOutput(0, &mut output, &mut status) };
        drop(unsafe { ManuallyDrop::take(&mut output[0].pSample) });
        drop(unsafe { ManuallyDrop::take(&mut output[0].pEvents) });
        match result {
            Ok(()) => {}
            Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                self.output = Some((sample, buffer));
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        }
        let pts = unsafe { sample.GetSampleTime() }?;
        let mut data = std::ptr::null_mut();
        let mut length = 0u32;
        unsafe { buffer.Lock(&mut data, None, Some(&mut length)) }?;
        let bytes: Arc<[u8]> =
            Arc::from(unsafe { std::slice::from_raw_parts(data, length as usize) });
        unsafe { buffer.Unlock() }?;
        self.output = Some((sample, buffer));
        Ok(Some((pts, bytes)))
    }
}

fn to_pcm(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
}

fn open_microphone(
    enumerator: &IMMDeviceEnumerator,
    name: Option<&str>,
    report: bool,
) -> Option<Source> {
    let named = name.and_then(|name| {
        endpoints(enumerator, eCapture)
            .into_iter()
            .find(|device| device_name(device).as_deref() == Some(name))
    });
    let device = match named {
        Some(device) => device,
        None => unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) }.ok()?,
    };
    let opened = Source::open(&device, false);
    if report {
        trace!(
            AREA,
            "microphone wanted {name:?}, using {:?}, opened {}",
            device_name(&device),
            opened.is_ok()
        );
        return opened.traced(AREA, "open the microphone").ok();
    }
    opened.ok()
}

fn open_loopback(enumerator: &IMMDeviceEnumerator) -> Option<Source> {
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .traced(AREA, "find the default playback device")
        .ok()?;
    trace!(
        AREA,
        "opening loopback of the playback device {:?}",
        device_name(&device)
    );
    Source::open(&device, true)
        .traced(AREA, "open loopback of the playback device")
        .ok()
}

fn default_render_id(enumerator: &IMMDeviceEnumerator) -> Option<String> {
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }.ok()?;
    device_id(&device)
}

fn open_outputs(
    enumerator: &IMMDeviceEnumerator,
    discord: Option<u32>,
) -> (Option<Source>, Option<Source>) {
    if let Some(process) = discord {
        let id = default_render_id(enumerator);
        trace!(
            AREA,
            "opening separate Discord capture, process {process}, playback device {id:?}"
        );
        if let Ok(mut voice) = Source::process(process, Capture::Only, id.clone())
            .traced(AREA, "capture only the Discord process tree")
            && let Ok(system) = Source::process(process, Capture::Without, id)
                .traced(AREA, "capture everything except the Discord process tree")
        {
            voice.flush();
            trace!(AREA, "Discord and system sound are captured separately");
            return (Some(system), Some(voice));
        }
        trace!(
            AREA,
            "separate Discord capture is unavailable, falling back to device loopback"
        );
    } else {
        trace!(AREA, "no Discord process, capturing device loopback");
    }
    (open_loopback(enumerator), None)
}

const fn searches(known: bool, check: u32) -> bool {
    let every = if known {
        DISCORD_REVIEW_CHECKS
    } else {
        DISCORD_SEARCH_CHECKS
    };
    check.is_multiple_of(every)
}

fn discord_now(known: Option<u32>, check: u32) -> Option<u32> {
    let alive = known.and_then(discord::alive);
    if alive == Some(false)
        || alive.is_none() && known.is_some()
        || searches(known.is_some(), check)
    {
        discord::running()
    } else {
        known
    }
}

pub const fn track_count(separate: bool) -> usize {
    if separate { SEPARATE_TRACKS } else { 1 }
}

fn route(system: &[f32], mic: &[f32], voice: &[f32], tracks: &mut [Vec<i16>]) {
    match tracks {
        [single] => single.extend(
            system
                .iter()
                .zip(mic)
                .zip(voice)
                .map(|((system, mic), voice)| to_pcm(system + mic + voice)),
        ),
        [system_track, mic_track, voice_track] => {
            system_track.extend(system.iter().copied().map(to_pcm));
            mic_track.extend(mic.iter().copied().map(to_pcm));
            voice_track.extend(voice.iter().copied().map(to_pcm));
        }
        _ => {}
    }
}

fn due_frames(elapsed: i64, produced: i64) -> i64 {
    elapsed * i64::from(SAMPLE_RATE) / HNS_PER_SECOND - produced
}

fn chunk_pts(start: i64, produced: i64) -> i64 {
    start + produced * HNS_PER_SECOND / i64::from(SAMPLE_RATE)
}

pub struct AudioSetup {
    pub separate_tracks: bool,
    pub microphone: Option<String>,
}

pub fn run(
    setup: &AudioSetup,
    controls: &AudioControls,
    active: impl Fn() -> bool,
    on_formats: impl FnOnce(Vec<AudioFormat>),
    mut push: impl FnMut(Vec<Packet>),
) -> Result<()> {
    trace!(
        AREA,
        "audio session starting, separate tracks {}, microphone {:?}",
        setup.separate_tracks,
        setup.microphone
    );
    let _runtime = Runtime::enter().traced(AREA, "enter the COM runtime")?;
    let enumerator = enumerator().traced(AREA, "create the device enumerator")?;
    let separate = setup.separate_tracks;
    let mut discord = if separate { discord::running() } else { None };
    let (mut loopback, mut voice) = open_outputs(&enumerator, discord);
    let mut microphone = open_microphone(&enumerator, setup.microphone.as_deref(), true);
    let mut encoders = (0..track_count(separate))
        .map(Aac::new)
        .collect::<Result<Vec<_>>>()
        .traced(AREA, "create the AAC encoders")?;
    trace!(
        AREA,
        "audio session ready, system {}, Discord {}, microphone {}, tracks {}",
        loopback.is_some(),
        voice.is_some(),
        microphone.is_some(),
        encoders.len()
    );
    on_formats(
        encoders
            .iter()
            .map(|encoder| encoder.format.clone())
            .collect(),
    );
    [&mut loopback, &mut voice, &mut microphone]
        .into_iter()
        .flatten()
        .for_each(Source::flush);
    let start = clock::now();
    let mut produced: i64 = 0;
    let mut mixed: Vec<Vec<i16>> = vec![Vec::with_capacity(4096); encoders.len()];
    let mut system = Vec::with_capacity(4096);
    let mut mic = Vec::with_capacity(4096);
    let mut speech = Vec::with_capacity(4096);
    let mut ticks = 0u32;
    let mut batch = Vec::new();

    while active() {
        thread::sleep(TICK);
        ticks = ticks.wrapping_add(1);
        let mut reopen = false;
        if separate && ticks.is_multiple_of(DISCORD_CHECK_TICKS) {
            let current = discord_now(discord, ticks / DISCORD_CHECK_TICKS);
            if current != discord {
                trace!(
                    AREA,
                    "Discord process changed from {discord:?} to {current:?}"
                );
                discord = current;
                reopen = true;
            }
        }
        if ticks.is_multiple_of(DEVICE_CHECK_TICKS) {
            let current = default_render_id(&enumerator);
            if loopback.as_ref().map(|source| &source.id) != Some(&current) {
                trace!(
                    AREA,
                    "default playback device is now {current:?}, reopening the outputs"
                );
                reopen = true;
            }
            if microphone.is_none() {
                microphone = open_microphone(&enumerator, setup.microphone.as_deref(), false);
                if microphone.is_some() {
                    trace!(AREA, "the microphone is available again");
                }
            }
        }
        if !reopen {
            reopen = [&mut loopback, &mut voice].into_iter().any(|source| {
                source.as_mut().is_some_and(|source| {
                    source
                        .read()
                        .traced(AREA, "read captured playback sound")
                        .is_err()
                })
            });
        }
        if reopen {
            (loopback, voice) = open_outputs(&enumerator, discord);
        }
        if microphone
            .as_mut()
            .is_some_and(|source| source.read().traced(AREA, "read the microphone").is_err())
        {
            trace!(
                AREA,
                "the microphone is lost, recording continues without it"
            );
            microphone = None;
        }

        let elapsed = clock::now() - start - LATENCY_HNS;
        let due = due_frames(elapsed, produced);
        if due <= 0 {
            continue;
        }
        let system_gain = controls.system();
        let mic_gain = controls.mic();
        mixed.iter_mut().for_each(Vec::clear);
        let samples = (due * i64::from(CHANNELS)) as usize;
        take(loopback.as_mut(), samples, system_gain, &mut system);
        take(microphone.as_mut(), samples, mic_gain, &mut mic);
        take(voice.as_mut(), samples, system_gain, &mut speech);
        route(&system, &mic, &speech, &mut mixed);
        let pts = chunk_pts(start, produced);
        for (encoder, pcm) in encoders.iter_mut().zip(&mixed) {
            encoder
                .encode(pcm, pts, &mut |packet| batch.push(packet))
                .traced(AREA, "encode sound to AAC")?;
        }
        if !batch.is_empty() {
            push(std::mem::take(&mut batch));
        }
        produced += due;
    }
    trace!(AREA, "audio session finished after {produced} frames");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backlog(quiet: usize, loud: usize) -> VecDeque<f32> {
        std::iter::repeat_n(0.0, quiet)
            .chain(std::iter::repeat_n(0.5, loud))
            .collect()
    }

    #[test]
    fn an_ordinary_backlog_is_left_alone() {
        let mut fifo = backlog(SLACK_SAMPLES, 0);
        catch_up(&mut fifo);
        assert_eq!(fifo.len(), SLACK_SAMPLES);
    }

    #[test]
    fn a_quiet_backlog_is_dropped_so_the_sound_after_it_is_not_late() {
        let mut fifo = backlog(SLACK_SAMPLES, KEPT_SAMPLES);
        catch_up(&mut fifo);
        assert_eq!(fifo.len(), KEPT_SAMPLES);
        assert!(fifo.iter().all(|sample| *sample == 0.5));
    }

    #[test]
    fn a_loud_backlog_is_kept_until_it_is_too_late_to_play() {
        let mut fifo = backlog(0, LATE_SAMPLES);
        catch_up(&mut fifo);
        assert_eq!(fifo.len(), LATE_SAMPLES);
        fifo.extend([0.5, 0.5]);
        catch_up(&mut fifo);
        assert_eq!(fifo.len(), KEPT_SAMPLES);
    }

    #[test]
    fn pcm_conversion_clamps() {
        assert_eq!(to_pcm(2.0), i16::MAX);
        assert_eq!(to_pcm(-2.0), -i16::MAX);
        assert_eq!(to_pcm(0.0), 0);
    }

    #[test]
    fn separate_tracks_are_system_mic_and_discord() {
        assert_eq!(track_count(false), 1);
        assert_eq!(track_count(true), 3);
    }

    #[test]
    fn a_single_track_mixes_everything_in() {
        let mut tracks = vec![Vec::new()];
        route(&[0.25, 0.5], &[0.25, 0.0], &[0.25, 0.125], &mut tracks);
        assert_eq!(tracks[0], vec![to_pcm(0.75), to_pcm(0.625)]);
    }

    #[test]
    fn an_absent_discord_still_fills_its_track_with_silence() {
        let mut tracks = vec![Vec::new(); 3];
        route(&[0.5; 480], &[0.25; 480], &[0.0; 480], &mut tracks);
        assert!(tracks.iter().all(|track| track.len() == 480));
        assert!(tracks[0].iter().all(|sample| *sample == to_pcm(0.5)));
        assert!(tracks[1].iter().all(|sample| *sample == to_pcm(0.25)));
        assert!(tracks[2].iter().all(|sample| *sample == 0));
    }

    #[test]
    fn a_short_source_is_padded_with_silence_and_keeps_its_remainder() {
        let mut fifo: VecDeque<f32> = [0.5, -0.5, 0.25].into();
        let mut samples = vec![9.0];
        drain(Some(&mut fifo), 2, 2.0, &mut samples);
        assert_eq!(samples, vec![1.0, -1.0]);
        assert_eq!(fifo, [0.25]);
        drain(Some(&mut fifo), 4, 2.0, &mut samples);
        assert_eq!(samples, vec![0.5, 0.0, 0.0, 0.0]);
        assert!(fifo.is_empty());
        drain(None, 3, 2.0, &mut samples);
        assert_eq!(samples, vec![0.0; 3]);
    }

    const CHUNK: usize = 941;

    fn tone(from: usize, frames: usize) -> Vec<i16> {
        (from..from + frames)
            .flat_map(|frame| {
                let level =
                    (frame as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin();
                [to_pcm(level * 0.3); CHANNELS as usize]
            })
            .collect()
    }

    fn program() -> Vec<Vec<i16>> {
        let parts = [
            (true, 50),
            (false, 160),
            (true, 50),
            (false, 40),
            (true, 50),
            (false, 90),
        ];
        let mut at = 0;
        let mut chunks = Vec::new();
        for (sound, count) in parts {
            for _ in 0..count {
                chunks.push(if sound {
                    tone(at, CHUNK)
                } else {
                    vec![0; CHUNK * CHANNELS as usize]
                });
                at += CHUNK;
            }
        }
        chunks
    }

    fn encoded(skipping: bool) -> Vec<Packet> {
        let _runtime = Runtime::enter().unwrap();
        let mut encoder = Aac::new(0).unwrap();
        if !skipping {
            encoder.quiet.zeros = i64::MIN / 2;
        }
        let mut packets = Vec::new();
        for (index, chunk) in program().iter().enumerate() {
            let pts = chunk_pts(7_000_000, (index * CHUNK) as i64);
            encoder
                .encode(chunk, pts, &mut |packet| packets.push(packet))
                .unwrap();
        }
        packets
    }

    #[test]
    fn skipped_silence_leaves_the_stream_exactly_as_the_encoder_would_write_it() {
        let reference = encoded(false);
        let skipped = encoded(true);
        assert_eq!(skipped.len(), reference.len());
        let shared = skipped
            .windows(2)
            .filter(|pair| Arc::ptr_eq(&pair[0].data, &pair[1].data))
            .count();
        assert!(shared > 150, "only {shared} packets were skipped");
        for (index, (ours, theirs)) in skipped.iter().zip(&reference).enumerate() {
            assert_eq!(ours.pts, theirs.pts, "packet {index}");
            assert_eq!(ours.duration, theirs.duration, "packet {index}");
            assert_eq!(ours.data, theirs.data, "packet {index}");
        }
    }

    #[test]
    fn packets_sit_on_an_exact_grid_of_aac_frames() {
        let packets = encoded(true);
        let origin = packets[0].pts;
        for (index, packet) in packets.iter().enumerate() {
            assert_eq!(packet.pts, frame_pts(origin, index as i64));
            assert_eq!(
                packet.pts + packet.duration,
                frame_pts(origin, index as i64 + 1)
            );
        }
    }

    #[test]
    fn skipped_frames_come_out_whole_and_the_rest_is_owed() {
        let mut quiet = Quiet {
            frame: Some(Arc::from([1u8, 2].as_slice())),
            ..Quiet::default()
        };
        assert_eq!(quiet.skip(1000).count(), 0);
        assert_eq!(quiet.skip(1100).count(), 2);
        assert_eq!(quiet.resume(), 52);
        assert_eq!(quiet.owed, 0);
    }

    #[test]
    fn silence_is_only_trusted_after_two_equal_frames_of_it() {
        let frame: Arc<[u8]> = Arc::from([3u8].as_slice());
        let mut quiet = Quiet::default();
        quiet.heard(true, SETTLED_FRAMES - 1);
        quiet.learn(&frame);
        quiet.learn(&frame);
        assert!(quiet.frame.is_none());
        quiet.heard(true, 1);
        quiet.learn(&frame);
        quiet.learn(&Arc::from([4u8].as_slice()));
        assert!(quiet.frame.is_none());
        quiet.learn(&Arc::from([4u8].as_slice()));
        assert!(quiet.frame.is_some());
        quiet.settle();
        assert!(!quiet.skipping);
        quiet.heard(true, QUIET_AFTER_FRAMES);
        quiet.settle();
        assert!(quiet.skipping);
        quiet.heard(false, 10);
        quiet.settle();
        assert!(!quiet.skipping);
    }

    #[test]
    fn discord_is_searched_for_rarely_and_reviewed_even_more_rarely() {
        let searched = |known| (1..=30).filter(|check| searches(known, *check)).count();
        assert_eq!(searched(false), 10);
        assert_eq!(searched(true), 3);
    }

    #[test]
    fn chunks_follow_each_other_without_gaps() {
        let start = 1_000_000;
        let mut produced = 0;
        let mut previous_end = chunk_pts(start, 0);
        for tick in 1..=500 {
            let elapsed = tick * 100_037;
            let due = due_frames(elapsed, produced);
            assert!(due >= 0);
            let pts = chunk_pts(start, produced);
            assert_eq!(pts, previous_end);
            produced += due;
            previous_end = chunk_pts(start, produced);
        }
        let expected = 500 * 100_037 * i64::from(SAMPLE_RATE) / HNS_PER_SECOND;
        assert_eq!(produced, expected);
    }
}
