use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize,
    CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode, ICodecAPI, IMFActivate,
    IMFAttributes, IMFDXGIDeviceManager, IMFMediaEventGenerator, IMFMediaType, IMFSample,
    IMFTransform, METransformDrainComplete, METransformHaveOutput, METransformNeedInput,
    MF_E_NO_EVENTS_AVAILABLE, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE,
    MF_EVENT_FLAG_NO_WAIT, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_MPEG2_PROFILE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK,
    MFCreateAttributes, MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFSampleExtension_CleanPoint,
    MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_ADAPTER_LUID, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_END_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFVideoFormat_AV1, MFVideoFormat_H264, MFVideoFormat_HEVC,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, eAVEncCommonRateControlMode_CBR,
    eAVEncH264VProfile_High, eAVEncH265VProfile_Main_420_8,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{GUID, Interface};

use rewind_trace::{Traced, trace};

use crate::com::{Runtime, Transform, first_transform, shut_down, transform_count};
use crate::error::{CaptureError, Result};
use crate::format::{HNS_PER_SECOND, Packet, Track, VideoCodec, VideoFormat, parameter_sets};

const PENDING_PTS: usize = 32;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const DRAIN_POLL: Duration = Duration::from_millis(2);
const RELEASE_DELAY: Duration = Duration::from_secs(1);

struct Retired {
    _transform: IMFTransform,
    _activate: IMFActivate,
    _events: Option<IMFMediaEventGenerator>,
    _manager: IMFDXGIDeviceManager,
}

unsafe impl Send for Retired {}

pub struct Encoder {
    transform: IMFTransform,
    activate: IMFActivate,
    codec_api: Option<ICodecAPI>,
    events: Option<IMFMediaEventGenerator>,
    _manager: IMFDXGIDeviceManager,
    pending_input: u32,
    provides_samples: bool,
    output_size: u32,
    frame_duration: i64,
    inputs: std::collections::VecDeque<i64>,
    format: VideoFormat,
    pub name: String,
}

pub fn pack(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

pub const fn subtype(codec: VideoCodec) -> GUID {
    match codec {
        VideoCodec::H264 => MFVideoFormat_H264,
        VideoCodec::Hevc => MFVideoFormat_HEVC,
        VideoCodec::Av1 => MFVideoFormat_AV1,
    }
}

pub fn hardware_encoder(codec: VideoCodec) -> bool {
    let _runtime = Runtime::enter().ok();
    transform_count(
        MFT_CATEGORY_VIDEO_ENCODER,
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
        (MFMediaType_Video, MFVideoFormat_NV12),
        (MFMediaType_Video, subtype(codec)),
    )
    .is_ok_and(|count| count > 0)
}

fn find(luid: LUID, codec: VideoCodec) -> Result<Option<Transform>> {
    let mut attributes: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attributes, 1) }?;
    let attributes = attributes.ok_or(CaptureError::NoEncoder)?;
    let adapter = pack(luid.HighPart as u32, luid.LowPart);
    unsafe { attributes.SetUINT64(&MFT_ENUM_ADAPTER_LUID, adapter) }?;
    first_transform(
        MFT_CATEGORY_VIDEO_ENCODER,
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
        (MFMediaType_Video, MFVideoFormat_NV12),
        (MFMediaType_Video, subtype(codec)),
        Some(&attributes),
    )
}

fn set_codec_value(api: &ICodecAPI, key: &GUID, value: u32) {
    let value = VARIANT::from(value);
    let _ = unsafe { api.SetValue(key, &value) };
}

const AREA: &str = "encoder";

impl Encoder {
    pub fn new(
        device: &ID3D11Device,
        luid: LUID,
        codec: VideoCodec,
        size: (u32, u32),
        fps: u32,
        bitrate: u32,
    ) -> Result<Self> {
        let (
            Transform {
                transform,
                activate,
                name,
            },
            codec,
        ) = {
            let mut wanted = Some(codec);
            loop {
                let Some(candidate) = wanted else {
                    trace!(
                        AREA,
                        "FAILED no hardware encoder for {codec:?} or its fallbacks"
                    );
                    return Err(CaptureError::NoEncoder);
                };
                if let Some(found) =
                    find(luid, candidate).traced(AREA, "look for a hardware encoder")?
                {
                    break (found, candidate);
                }
                trace!(
                    AREA,
                    "no hardware encoder for {candidate:?}, trying the fallback"
                );
                wanted = candidate.fallback();
            }
        };
        trace!(
            AREA,
            "using encoder {name:?} for {codec:?}, {}x{} at {fps} fps, {bitrate} bit/s",
            size.0,
            size.1
        );

        let attributes = unsafe { transform.GetAttributes() }?;
        let events = if unsafe { attributes.GetUINT32(&MF_TRANSFORM_ASYNC) }.unwrap_or(0) != 0 {
            unsafe { attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1) }?;
            Some(transform.cast::<IMFMediaEventGenerator>()?)
        } else {
            None
        };

        let mut token = 0u32;
        let mut manager = None;
        unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager) }?;
        let manager = manager.ok_or(CaptureError::NoEncoder)?;
        unsafe { manager.ResetDevice(device, token) }?;
        unsafe {
            transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)
        }?;

        let codec_api = transform.cast::<ICodecAPI>().ok();
        if let Some(api) = &codec_api {
            set_codec_value(
                api,
                &CODECAPI_AVEncCommonRateControlMode,
                eAVEncCommonRateControlMode_CBR.0 as u32,
            );
            set_codec_value(api, &CODECAPI_AVEncCommonMeanBitRate, bitrate);
            set_codec_value(api, &CODECAPI_AVEncMPVGOPSize, fps);
            set_codec_value(api, &CODECAPI_AVEncMPVDefaultBPictureCount, 0);
            set_codec_value(api, &CODECAPI_AVLowLatencyMode, 1);
        }

        let output = unsafe { MFCreateMediaType() }?;
        let profile = match codec {
            VideoCodec::H264 => Some(eAVEncH264VProfile_High.0 as u32),
            VideoCodec::Hevc => Some(eAVEncH265VProfile_Main_420_8.0 as u32),
            VideoCodec::Av1 => None,
        };
        unsafe {
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            output.SetGUID(&MF_MT_SUBTYPE, &subtype(codec))?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            if let Some(profile) = profile {
                output.SetUINT32(&MF_MT_MPEG2_PROFILE, profile)?;
            }
        }
        describe(&output, size, fps)?;
        unsafe { transform.SetOutputType(0, &output, 0) }?;

        let input = unsafe { MFCreateMediaType() }?;
        unsafe {
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        }
        describe(&input, size, fps)?;
        unsafe { transform.SetInputType(0, &input, 0) }?;

        let info = unsafe { transform.GetOutputStreamInfo(0) }?;
        unsafe {
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        }
        let mut encoder = Self {
            transform,
            activate,
            codec_api,
            events,
            _manager: manager,
            pending_input: 0,
            provides_samples: info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0,
            output_size: info.cbSize.max(1 << 20),
            frame_duration: HNS_PER_SECOND / i64::from(fps.max(1)),
            inputs: std::collections::VecDeque::with_capacity(PENDING_PTS),
            format: VideoFormat {
                codec,
                width: size.0,
                height: size.1,
                fps,
                bitrate,
                sequence_header: Vec::new(),
            },
            name,
        };
        encoder.read_sequence_header();
        Ok(encoder)
    }

    pub fn format(&self) -> Option<VideoFormat> {
        (!self.format.sequence_header.is_empty()).then(|| self.format.clone())
    }

    pub fn encode(
        &mut self,
        texture: &ID3D11Texture2D,
        pts: i64,
        force_keyframe: bool,
        sink: &mut impl FnMut(Packet),
    ) -> Result<bool> {
        self.pump(sink)?;
        if self.events.is_some() && self.pending_input == 0 {
            return Ok(false);
        }
        let buffer =
            unsafe { MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false) }?;
        let sample = unsafe { MFCreateSample() }?;
        unsafe {
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            sample.SetSampleDuration(self.frame_duration)?;
        }
        if force_keyframe && let Some(api) = &self.codec_api {
            set_codec_value(api, &CODECAPI_AVEncVideoForceKeyFrame, 1);
        }
        unsafe { self.transform.ProcessInput(0, &sample, 0) }?;
        if self.inputs.len() >= PENDING_PTS {
            self.inputs.pop_front();
        }
        self.inputs.push_back(pts);
        if self.events.is_some() {
            self.pending_input -= 1;
        } else {
            while self.drain_one(sink)? {}
        }
        self.pump(sink)?;
        Ok(true)
    }

    pub fn ready(&self) -> bool {
        self.events.is_none() || self.pending_input > 0
    }

    pub fn settled(&self) -> bool {
        self.inputs.is_empty() && self.ready()
    }

    pub fn poll(&mut self, sink: &mut impl FnMut(Packet)) -> Result<()> {
        self.pump(sink)
    }

    fn pump(&mut self, sink: &mut impl FnMut(Packet)) -> Result<()> {
        while let Some(kind) = self.next_event()? {
            if kind == METransformNeedInput.0 as u32 {
                self.pending_input += 1;
            } else if kind == METransformHaveOutput.0 as u32 {
                self.drain_one(sink)?;
            }
        }
        Ok(())
    }

    fn next_event(&self) -> Result<Option<u32>> {
        let Some(events) = &self.events else {
            return Ok(None);
        };
        match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
            Ok(event) => Ok(Some(unsafe { event.GetType() }?)),
            Err(error) if error.code() == MF_E_NO_EVENTS_AVAILABLE => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn output_sample(&self) -> Result<Option<IMFSample>> {
        if self.provides_samples {
            return Ok(None);
        }
        let sample = unsafe { MFCreateSample() }?;
        let buffer = unsafe { MFCreateMemoryBuffer(self.output_size) }?;
        unsafe { sample.AddBuffer(&buffer) }?;
        Ok(Some(sample))
    }

    fn drain_one(&mut self, sink: &mut impl FnMut(Packet)) -> Result<bool> {
        let mut output = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(self.output_sample()?),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        }];
        let mut status = 0u32;
        let result = unsafe { self.transform.ProcessOutput(0, &mut output, &mut status) };
        let sample = unsafe { ManuallyDrop::take(&mut output[0].pSample) };
        drop(unsafe { ManuallyDrop::take(&mut output[0].pEvents) });
        match result {
            Ok(()) => {}
            Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(false),
            Err(error) if error.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                let changed = unsafe { self.transform.GetOutputAvailableType(0, 0) }?;
                unsafe { self.transform.SetOutputType(0, &changed, 0) }?;
                self.read_sequence_header();
                return Ok(true);
            }
            Err(error) => {
                trace!(AREA, "FAILED take encoded output: {error:?}");
                return Err(error.into());
            }
        }
        let Some(sample) = sample else {
            return Ok(true);
        };
        let packet = self.packet(&sample)?;
        if self.format.sequence_header.is_empty() && packet.keyframe {
            self.read_sequence_header();
            if self.format.sequence_header.is_empty() {
                self.format.sequence_header = parameter_sets(self.format.codec, &packet.data);
            }
        }
        sink(packet);
        Ok(true)
    }

    fn packet(&mut self, sample: &IMFSample) -> Result<Packet> {
        let reported = unsafe { sample.GetSampleTime() }?;
        let pts = self.inputs.pop_front().unwrap_or(reported);
        let duration = unsafe { sample.GetSampleDuration() }.unwrap_or(self.frame_duration);
        let keyframe = unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }.unwrap_or(0) != 0;
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }?;
        let mut data = std::ptr::null_mut();
        let mut length = 0u32;
        unsafe { buffer.Lock(&mut data, None, Some(&mut length)) }?;
        let bytes: Arc<[u8]> =
            Arc::from(unsafe { std::slice::from_raw_parts(data, length as usize) });
        unsafe { buffer.Unlock() }?;
        Ok(Packet {
            track: Track::Video,
            pts,
            duration,
            keyframe,
            data: bytes,
        })
    }

    fn read_sequence_header(&mut self) {
        let Ok(current) = (unsafe { self.transform.GetOutputCurrentType(0) }) else {
            return;
        };
        if let Some(header) = blob(&current, &MF_MT_MPEG_SEQUENCE_HEADER) {
            self.format.sequence_header = header;
        }
    }
}

impl Encoder {
    fn finish(&mut self) {
        let Some(events) = self.events.clone() else {
            return;
        };
        if unsafe { self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0) }.is_err() {
            return;
        }
        let deadline = Instant::now() + DRAIN_TIMEOUT;
        while Instant::now() < deadline {
            match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => {
                    let kind = unsafe { event.GetType() }.unwrap_or(0);
                    if kind == METransformDrainComplete.0 as u32 {
                        return;
                    }
                    if kind == METransformHaveOutput.0 as u32
                        && self.drain_one(&mut |_| {}).is_err()
                    {
                        return;
                    }
                }
                Err(error) if error.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    std::thread::sleep(DRAIN_POLL);
                }
                Err(_) => return,
            }
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        let _ = unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
        };
        self.finish();
        let _ = unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0)
        };
        shut_down(&self.transform, &self.activate);
        let retired = Retired {
            _transform: self.transform.clone(),
            _activate: self.activate.clone(),
            _events: self.events.clone(),
            _manager: self._manager.clone(),
        };
        let _ = std::thread::Builder::new()
            .name("rewind-encoder-release".to_owned())
            .spawn(move || {
                let _runtime = Runtime::enter();
                std::thread::sleep(RELEASE_DELAY);
                drop(retired);
            });
    }
}

fn describe(media_type: &IMFMediaType, size: (u32, u32), fps: u32) -> Result<()> {
    unsafe {
        media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack(size.0, size.1))?;
        media_type.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
        media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    }
    Ok(())
}

pub fn blob(media_type: &IMFMediaType, key: &GUID) -> Option<Vec<u8>> {
    let size = unsafe { media_type.GetBlobSize(key) }.ok()?;
    let mut data = vec![0u8; size as usize];
    unsafe { media_type.GetBlob(key, &mut data, None) }.ok()?;
    (!data.is_empty()).then_some(data)
}
