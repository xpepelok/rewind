use std::path::Path;
use std::time::Duration;

use windows::Win32::Media::MediaFoundation::{
    IMFSourceReader, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_PD_DURATION,
    MF_SOURCE_READER_MEDIASOURCE, MFCreateSourceReaderFromURL, MFMediaType_Video,
    MFVideoFormat_H264, MFVideoFormat_HEVC,
};
use windows::core::HSTRING;

use crate::error::{MediaError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaInfo {
    pub duration: Duration,
    pub width: u32,
    pub height: u32,
    pub codec: Codec,
}

pub(crate) fn open_reader(path: &Path) -> Result<IMFSourceReader> {
    Ok(unsafe { MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None) }?)
}

pub(crate) fn duration(reader: &IMFSourceReader) -> Result<Duration> {
    let value = unsafe {
        reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
    }?;
    let hns = u64::try_from(&value).unwrap_or(0);
    Ok(Duration::from_nanos(hns.saturating_mul(100)))
}

pub(crate) fn video_stream(reader: &IMFSourceReader) -> Option<u32> {
    (0..)
        .map_while(|index| {
            unsafe { reader.GetNativeMediaType(index, 0) }
                .ok()
                .map(|media_type| (index, media_type))
        })
        .find(|(_, media_type)| {
            unsafe { media_type.GetGUID(&MF_MT_MAJOR_TYPE) }
                .is_ok_and(|major| major == MFMediaType_Video)
        })
        .map(|(index, _)| index)
}

pub fn probe(path: &Path) -> Result<MediaInfo> {
    let reader = open_reader(path)?;
    let stream = video_stream(&reader).ok_or(MediaError::NoVideo)?;
    let native = unsafe { reader.GetNativeMediaType(stream, 0) }?;
    let size = unsafe { native.GetUINT64(&MF_MT_FRAME_SIZE) }.unwrap_or(0);
    let subtype = unsafe { native.GetGUID(&MF_MT_SUBTYPE) }?;
    let codec = if subtype == MFVideoFormat_H264 {
        Codec::H264
    } else if subtype == MFVideoFormat_HEVC {
        Codec::Hevc
    } else {
        Codec::Other
    };
    Ok(MediaInfo {
        duration: duration(&reader)?,
        width: (size >> 32) as u32,
        height: (size & 0xffff_ffff) as u32,
        codec,
    })
}
