use std::sync::Arc;

pub const HNS_PER_SECOND: i64 = 10_000_000;

const H264_SPS: u8 = 7;
const H264_PPS: u8 = 8;
const HEVC_VPS: u8 = 32;
const HEVC_SPS: u8 = 33;
const HEVC_PPS: u8 = 34;
const START_CODE: [u8; 4] = [0, 0, 0, 1];
const OBU_SEQUENCE_HEADER: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

impl VideoCodec {
    fn is_parameter_set(self, header: u8) -> bool {
        match self {
            Self::H264 => matches!(header & 0x1F, H264_SPS | H264_PPS),
            Self::Hevc => matches!((header >> 1) & 0x3F, HEVC_VPS | HEVC_SPS | HEVC_PPS),
            Self::Av1 => false,
        }
    }

    pub const fn fallback(self) -> Option<Self> {
        match self {
            Self::Av1 => Some(Self::Hevc),
            Self::Hevc => Some(Self::H264),
            Self::H264 => None,
        }
    }
}

fn leb128(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    for (index, byte) in bytes.iter().take(8).enumerate() {
        value |= usize::from(byte & 0x7F) << (index * 7);
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
    }
    None
}

fn sequence_header_obu(stream: &[u8]) -> Vec<u8> {
    let mut at = 0;
    while let Some(header) = stream.get(at) {
        let kind = (header >> 3) & 0x0F;
        let extension = usize::from(header & 0x04 != 0);
        if header & 0x02 == 0 {
            return if kind == OBU_SEQUENCE_HEADER {
                stream[at..].to_vec()
            } else {
                Vec::new()
            };
        }
        let cursor = at + 1 + extension;
        let Some((size, read)) = stream.get(cursor..).and_then(leb128) else {
            return Vec::new();
        };
        let end = cursor + read + size;
        if end > stream.len() {
            return Vec::new();
        }
        if kind == OBU_SEQUENCE_HEADER {
            return stream[at..end].to_vec();
        }
        at = end;
    }
    Vec::new()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoFormat {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub sequence_header: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u32,
    pub bytes_per_second: u32,
    pub user_data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Video,
    Audio(usize),
}

#[derive(Clone, Debug)]
pub struct Packet {
    pub track: Track,
    pub pts: i64,
    pub duration: i64,
    pub keyframe: bool,
    pub data: Arc<[u8]>,
}

pub fn parameter_sets(codec: VideoCodec, stream: &[u8]) -> Vec<u8> {
    if codec == VideoCodec::Av1 {
        return sequence_header_obu(stream);
    }
    let mut header = Vec::with_capacity(128);
    for unit in nal_units(stream) {
        if unit
            .first()
            .is_some_and(|byte| codec.is_parameter_set(*byte))
        {
            header.extend_from_slice(&START_CODE);
            header.extend_from_slice(unit);
        }
    }
    header
}

fn nal_units(stream: &[u8]) -> impl Iterator<Item = &[u8]> {
    let starts: Vec<usize> = stream
        .windows(3)
        .enumerate()
        .filter(|(_, window)| *window == [0, 0, 1])
        .map(|(index, _)| index + 3)
        .collect();
    let ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .map(|next| {
            let end = next - 3;
            if end > 0 && stream[end - 1] == 0 {
                end - 1
            } else {
                end
            }
        })
        .chain(std::iter::once(stream.len()))
        .collect();
    starts
        .into_iter()
        .zip(ends)
        .filter(|(start, end)| start < end)
        .map(move |(start, end)| &stream[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_av1_sequence_header_after_a_temporal_delimiter() {
        let stream = [0x12, 0x00, 0x0A, 0x03, 7, 8, 9, 0x32, 0x02, 1, 2];
        assert_eq!(
            parameter_sets(VideoCodec::Av1, &stream),
            vec![0x0A, 0x03, 7, 8, 9]
        );
        assert!(parameter_sets(VideoCodec::Av1, &[0x32, 0x02, 1, 2]).is_empty());
        assert!(parameter_sets(VideoCodec::Av1, &[0x0A, 0x09, 1]).is_empty());
        assert_eq!(VideoCodec::Av1.fallback(), Some(VideoCodec::Hevc));
    }

    #[test]
    fn extracts_h264_sps_and_pps_only() {
        let stream = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3, 0, 0, 1, 0x65, 9, 9, 9,
        ];
        let header = parameter_sets(VideoCodec::H264, &stream);
        assert_eq!(header, vec![0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3]);
    }

    #[test]
    fn extracts_hevc_vps_sps_pps() {
        let stream = [
            0, 0, 0, 1, 0x40, 1, 0, 0, 0, 1, 0x42, 1, 0, 0, 0, 1, 0x44, 1, 0, 0, 0, 1, 0x26, 1,
        ];
        let header = parameter_sets(VideoCodec::Hevc, &stream);
        assert_eq!(
            header,
            vec![
                0, 0, 0, 1, 0x40, 1, 0, 0, 0, 1, 0x42, 1, 0, 0, 0, 1, 0x44, 1
            ]
        );
    }

    #[test]
    fn stream_without_parameter_sets_gives_empty_header() {
        assert!(parameter_sets(VideoCodec::H264, &[0, 0, 1, 0x41, 7]).is_empty());
    }
}
