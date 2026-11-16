use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAP_FLAG_DO_NOT_WAIT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_WAS_STILL_DRAWING;
use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFMediaEngine, IMFMediaEngineClassFactory,
    IMFMediaEngineNotify, IMFMediaEngineNotify_Impl, MF_MEDIA_ENGINE_CALLBACK,
    MF_MEDIA_ENGINE_DXGI_MANAGER, MF_MEDIA_ENGINE_EVENT_ENDED, MF_MEDIA_ENGINE_EVENT_ERROR,
    MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA, MF_MEDIA_ENGINE_EVENT_SEEKED,
    MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MFCreateAttributes, MFCreateDXGIDeviceManager,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{BSTR, Interface, implement};

use crate::error::{MediaError, Result};
use crate::runtime::Runtime;

const TICK: Duration = Duration::from_millis(4);
const IDLE: Duration = Duration::from_millis(50);
const HEARTBEAT: Duration = Duration::from_millis(100);
const SEEK_EPSILON: f64 = 0.0005;
const READBACK_DEPTH: usize = 3;

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerState {
    pub ready: bool,
    pub playing: bool,
    pub ended: bool,
    pub failed: bool,
    pub position: f64,
    pub duration: f64,
}

impl PlayerState {
    fn same_mode(&self, other: &Self) -> bool {
        self.ready == other.ready
            && self.playing == other.playing
            && self.ended == other.ended
            && self.failed == other.failed
    }
}

enum Command {
    Play,
    Pause,
    Seek(f64),
    Volume(f64),
    Muted(bool),
    Rate(f64),
    Resize((u32, u32)),
    Wake,
    Close,
}

#[derive(Default)]
struct Flags {
    loaded: AtomicBool,
    seeked: AtomicBool,
    ended: AtomicBool,
    failed: AtomicBool,
}

#[implement(IMFMediaEngineNotify)]
struct Notify {
    flags: Arc<Flags>,
    wake: Sender<Command>,
}

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, _param1: usize, _param2: u32) -> windows::core::Result<()> {
        let flag = match event as i32 {
            value if value == MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA.0 => &self.flags.loaded,
            value if value == MF_MEDIA_ENGINE_EVENT_SEEKED.0 => &self.flags.seeked,
            value if value == MF_MEDIA_ENGINE_EVENT_ENDED.0 => &self.flags.ended,
            value if value == MF_MEDIA_ENGINE_EVENT_ERROR.0 => &self.flags.failed,
            _ => return Ok(()),
        };
        flag.store(true, Ordering::Release);
        self.wake.send(Command::Wake).ok();
        Ok(())
    }
}

pub struct Player {
    commands: Sender<Command>,
    state: Arc<Mutex<PlayerState>>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    pub fn open(
        path: &Path,
        bounds: (u32, u32),
        on_frame: impl Fn(Frame) + Send + 'static,
    ) -> Self {
        Self::open_with_state(path, bounds, on_frame, || {})
    }

    pub fn open_with_state(
        path: &Path,
        bounds: (u32, u32),
        on_frame: impl Fn(Frame) + Send + 'static,
        on_state: impl Fn() + Send + 'static,
    ) -> Self {
        let (commands, receiver) = mpsc::channel();
        let wake = commands.clone();
        let state = Arc::new(Mutex::new(PlayerState::default()));
        let shared = state.clone();
        let path = path.to_path_buf();
        let thread = thread::Builder::new()
            .name("rewind-player".to_owned())
            .spawn(move || {
                let callbacks = Callbacks {
                    on_frame: &on_frame,
                    on_state: &on_state,
                };
                if run(&path, bounds, &receiver, wake, &shared, &callbacks).is_err() {
                    shared.lock().failed = true;
                    on_state();
                }
            })
            .ok();
        Self {
            commands,
            state,
            thread,
        }
    }

    pub fn state(&self) -> PlayerState {
        *self.state.lock()
    }

    pub fn play(&self) {
        self.commands.send(Command::Play).ok();
    }

    pub fn pause(&self) {
        self.commands.send(Command::Pause).ok();
    }

    pub fn seek(&self, seconds: f64) {
        self.state.lock().position = seconds;
        self.commands.send(Command::Seek(seconds)).ok();
    }

    pub fn set_volume(&self, volume: f64) {
        self.commands.send(Command::Volume(volume)).ok();
    }

    pub fn set_muted(&self, muted: bool) {
        self.commands.send(Command::Muted(muted)).ok();
    }

    pub fn set_rate(&self, rate: f64) {
        self.commands.send(Command::Rate(rate)).ok();
    }

    pub fn resize(&self, bounds: (u32, u32)) {
        self.commands.send(Command::Resize(bounds)).ok();
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.commands.send(Command::Close).ok();
        if let Some(thread) = self.thread.take() {
            thread.join().ok();
        }
    }
}

struct Callbacks<'a, F: Fn(Frame), S: Fn()> {
    on_frame: &'a F,
    on_state: &'a S,
}

enum Readback {
    Ready(Frame),
    Busy,
    Failed,
}

struct Surface {
    target: ID3D11Texture2D,
    staging: Vec<ID3D11Texture2D>,
    queue: VecDeque<usize>,
    width: u32,
    height: u32,
}

fn device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }?;
    let (Some(device), Some(context)) = (device, context) else {
        return Err(MediaError::NoVideo);
    };
    let multithread: ID3D11Multithread = device.cast()?;
    let _ = unsafe { multithread.SetMultithreadProtected(true) };
    Ok((device, context))
}

fn texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    staging: bool,
) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: if staging {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        BindFlags: if staging {
            0
        } else {
            D3D11_BIND_RENDER_TARGET.0 as u32
        },
        CPUAccessFlags: if staging {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }?;
    texture.ok_or(MediaError::NoFrame)
}

pub fn fit(native: (u32, u32), bounds: (u32, u32)) -> (u32, u32) {
    let (width, height) = (native.0.max(1) as f64, native.1.max(1) as f64);
    let scale = (bounds.0 as f64 / width)
        .min(bounds.1 as f64 / height)
        .min(1.0);
    let even = |value: f64| ((value.round() as u32).max(2) + 1) & !1;
    (even(width * scale), even(height * scale))
}

impl Surface {
    fn new(device: &ID3D11Device, size: (u32, u32)) -> Result<Self> {
        Ok(Self {
            target: texture(device, size.0, size.1, false)?,
            staging: (0..READBACK_DEPTH)
                .map(|_| texture(device, size.0, size.1, true))
                .collect::<Result<_>>()?,
            queue: VecDeque::with_capacity(READBACK_DEPTH),
            width: size.0,
            height: size.1,
        })
    }

    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn pending(&self) -> bool {
        !self.queue.is_empty()
    }

    fn submit(
        &mut self,
        context: &ID3D11DeviceContext,
        engine: &IMFMediaEngine,
    ) -> Result<Option<Frame>> {
        let rect = RECT {
            left: 0,
            top: 0,
            right: self.width as i32,
            bottom: self.height as i32,
        };
        unsafe { engine.TransferVideoFrame(&self.target, None, &rect, None) }?;
        let mut oldest = None;
        if self.queue.len() >= self.staging.len()
            && let Some(slot) = self.queue.pop_front()
            && let Readback::Ready(frame) = self.read(context, slot, true)
        {
            oldest = Some(frame);
        }
        let slot = (0..self.staging.len())
            .find(|slot| !self.queue.contains(slot))
            .ok_or(MediaError::NoFrame)?;
        unsafe {
            context.CopyResource(&self.staging[slot], &self.target);
            context.Flush();
        }
        self.queue.push_back(slot);
        Ok(oldest)
    }

    fn collect(&mut self, context: &ID3D11DeviceContext) -> Option<Frame> {
        for index in (0..self.queue.len()).rev() {
            match self.read(context, self.queue[index], false) {
                Readback::Ready(frame) => {
                    self.queue.drain(..=index);
                    return Some(frame);
                }
                Readback::Busy => {}
                Readback::Failed => {
                    self.queue.remove(index);
                }
            }
        }
        None
    }

    fn read(&self, context: &ID3D11DeviceContext, slot: usize, wait: bool) -> Readback {
        let staging = &self.staging[slot];
        let flags = if wait {
            0
        } else {
            D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32
        };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        match unsafe { context.Map(staging, 0, D3D11_MAP_READ, flags, Some(&mut mapped)) } {
            Ok(()) => {}
            Err(error) if error.code() == DXGI_ERROR_WAS_STILL_DRAWING => return Readback::Busy,
            Err(_) => return Readback::Failed,
        }
        let row = (self.width * 4) as usize;
        let rows = self.height as usize;
        let pitch = mapped.RowPitch as usize;
        let base = mapped.pData as *const u8;
        let mut pixels = Vec::with_capacity(row * rows);
        if pitch == row {
            pixels.extend_from_slice(unsafe { std::slice::from_raw_parts(base, row * rows) });
        } else {
            for line in 0..rows {
                let start = unsafe { base.add(line * pitch) };
                pixels.extend_from_slice(unsafe { std::slice::from_raw_parts(start, row) });
            }
        }
        unsafe { context.Unmap(staging, 0) };
        Readback::Ready(Frame {
            width: self.width,
            height: self.height,
            pixels,
        })
    }
}

struct Engine(IMFMediaEngine);

impl std::ops::Deref for Engine {
    type Target = IMFMediaEngine;

    fn deref(&self) -> &IMFMediaEngine {
        &self.0
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = unsafe { self.0.Shutdown() };
    }
}

fn engine(device: &ID3D11Device, flags: &Arc<Flags>, wake: Sender<Command>) -> Result<Engine> {
    let mut token = 0u32;
    let mut manager = None;
    unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager) }?;
    let manager = manager.ok_or(MediaError::NoVideo)?;
    unsafe { manager.ResetDevice(device, token) }?;
    let notify: IMFMediaEngineNotify = Notify {
        flags: flags.clone(),
        wake,
    }
    .into();
    let mut attributes: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attributes, 3) }?;
    let attributes = attributes.ok_or(MediaError::NoVideo)?;
    unsafe {
        attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
        attributes.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &manager)?;
        attributes.SetUINT32(
            &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
            DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
        )?;
    }
    let factory: IMFMediaEngineClassFactory =
        unsafe { CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER) }?;
    Ok(Engine(unsafe { factory.CreateInstance(0, &attributes) }?))
}

fn accept(command: Command, bounds: &mut (u32, u32), pending: &mut Vec<Command>) -> bool {
    match command {
        Command::Close => return false,
        Command::Resize(size) => *bounds = size,
        Command::Wake => {}
        command => pending.push(command),
    }
    true
}

fn receive(
    commands: &Receiver<Command>,
    wait: Duration,
    bounds: &mut (u32, u32),
    pending: &mut Vec<Command>,
) -> bool {
    match commands.recv_timeout(wait) {
        Ok(command) => {
            if !accept(command, bounds, pending) {
                return false;
            }
        }
        Err(RecvTimeoutError::Timeout) => {}
        Err(RecvTimeoutError::Disconnected) => return false,
    }
    loop {
        match commands.try_recv() {
            Ok(command) => {
                if !accept(command, bounds, pending) {
                    return false;
                }
            }
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

fn run<F: Fn(Frame), S: Fn()>(
    path: &Path,
    mut bounds: (u32, u32),
    commands: &Receiver<Command>,
    wake: Sender<Command>,
    state: &Mutex<PlayerState>,
    callbacks: &Callbacks<'_, F, S>,
) -> Result<()> {
    let _runtime = Runtime::enter()?;
    let (device, context) = device()?;
    let flags = Arc::new(Flags::default());
    let engine = engine(&device, &flags, wake)?;
    unsafe { engine.SetSource(&BSTR::from(path.to_string_lossy().as_ref())) }?;
    let mut surface: Option<Surface> = None;
    let mut native = (0, 0);
    let mut last_pts = i64::MIN;
    let mut refresh = false;
    let mut pending: Vec<Command> = Vec::new();
    let mut reported: Option<PlayerState> = None;
    let mut signalled = Instant::now();
    let mut wait = Duration::ZERO;

    loop {
        if !receive(commands, wait, &mut bounds, &mut pending) {
            return Ok(());
        }
        if flags.failed.load(Ordering::Acquire) {
            return Err(MediaError::NoFrame);
        }
        if surface.is_none() && flags.loaded.load(Ordering::Acquire) {
            let (mut width, mut height) = (0, 0);
            unsafe { engine.GetNativeVideoSize(Some(&mut width), Some(&mut height)) }?;
            native = (width, height);
            surface = Some(Surface::new(&device, fit(native, bounds))?);
            refresh = true;
        }
        let wanted = fit(native, bounds);
        if surface
            .as_ref()
            .is_some_and(|current| current.size() != wanted)
        {
            surface = Some(Surface::new(&device, wanted)?);
            refresh = true;
        }
        if surface.is_some() {
            for command in pending.drain(..) {
                match command {
                    Command::Play => {
                        if unsafe { engine.IsEnded() }.as_bool() {
                            unsafe { engine.SetCurrentTime(0.0) }?;
                        }
                        unsafe { engine.Play() }?;
                        flags.ended.store(false, Ordering::Release);
                    }
                    Command::Pause => unsafe { engine.Pause() }?,
                    Command::Seek(seconds) => {
                        let current = unsafe { engine.GetCurrentTime() };
                        if (current - seconds).abs() > SEEK_EPSILON {
                            unsafe { engine.SetCurrentTime(seconds) }?;
                        }
                        flags.ended.store(false, Ordering::Release);
                        refresh = true;
                    }
                    Command::Volume(volume) => unsafe { engine.SetVolume(volume) }?,
                    Command::Muted(muted) => unsafe { engine.SetMuted(muted) }?,
                    Command::Rate(rate) => unsafe { engine.SetPlaybackRate(rate) }?,
                    Command::Resize(_) | Command::Wake | Command::Close => {}
                }
            }
        }
        if flags.seeked.swap(false, Ordering::AcqRel) {
            refresh = true;
        }
        if let Some(surface) = &mut surface {
            let mut frame = None;
            if let Ok(pts) = unsafe { engine.OnVideoStreamTick() }
                && (pts != last_pts || refresh)
            {
                last_pts = pts;
                refresh = false;
                frame = surface.submit(&context, &engine).ok().flatten();
            }
            if let Some(newer) = surface.collect(&context) {
                frame = Some(newer);
            }
            if let Some(frame) = frame {
                (callbacks.on_frame)(frame);
                signalled = Instant::now();
            }
        }
        let snapshot = {
            let mut shared = state.lock();
            shared.ready = surface.is_some();
            shared.playing = !unsafe { engine.IsPaused() }.as_bool();
            shared.ended = flags.ended.load(Ordering::Acquire);
            shared.duration = unsafe { engine.GetDuration() };
            if shared.playing || refresh {
                shared.position = unsafe { engine.GetCurrentTime() };
            }
            *shared
        };
        let changed = reported.is_none_or(|old| !old.same_mode(&snapshot));
        if changed || (snapshot.playing && signalled.elapsed() >= HEARTBEAT) {
            (callbacks.on_state)();
            signalled = Instant::now();
            reported = Some(snapshot);
        }
        let busy = snapshot.playing
            || refresh
            || !pending.is_empty()
            || surface.as_ref().is_none_or(Surface::pending);
        wait = if busy { TICK } else { IDLE };
    }
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit((1920, 1080), (960, 960)), (960, 540));
        assert_eq!(fit((1280, 720), (1920, 1080)), (1280, 720));
        assert_eq!(fit((1080, 1920), (960, 540)), (304, 540));
    }
}
