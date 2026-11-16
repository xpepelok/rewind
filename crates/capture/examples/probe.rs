#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    use windows::Win32::Media::MediaFoundation::{
        MF_MT_FRAME_SIZE, MF_MT_SUBTYPE, MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_ANY_STREAM,
        MF_SOURCE_READERF_ENDOFSTREAM, MFCreateSourceReaderFromURL, MFSTARTUP_FULL, MFStartup,
    };
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    use windows::core::HSTRING;

    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
    unsafe { MFStartup(0x0002_0070, MFSTARTUP_FULL) }?;
    for path in std::env::args().skip(1) {
        let reader = unsafe { MFCreateSourceReaderFromURL(&HSTRING::from(path.as_str()), None) }?;
        unsafe { reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, true) }?;
        let mut streams = Vec::new();
        for index in 0.. {
            let Ok(media_type) = (unsafe { reader.GetNativeMediaType(index, 0) }) else {
                break;
            };
            let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) }?;
            let size = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }.unwrap_or(0);
            streams.push((
                subtype,
                size >> 32,
                size & 0xffff_ffff,
                0u32,
                0i64,
                i64::MAX,
            ));
        }
        loop {
            let mut stream = 0u32;
            let mut flags = 0u32;
            let mut time = 0i64;
            let mut sample = None;
            unsafe {
                reader.ReadSample(
                    MF_SOURCE_READER_ANY_STREAM.0 as u32,
                    0,
                    Some(&mut stream),
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )
            }?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 && sample.is_none() {
                if streams
                    .iter()
                    .enumerate()
                    .all(|(index, _)| index as u32 != stream)
                {
                    break;
                }
                streams[stream as usize].3 |= 0x8000_0000;
                if streams.iter().all(|entry| entry.3 & 0x8000_0000 != 0) {
                    break;
                }
                continue;
            }
            if sample.is_some() {
                let entry = &mut streams[stream as usize];
                entry.3 += 1;
                entry.4 = entry.4.max(time);
                entry.5 = entry.5.min(time);
            }
        }
        println!("{path}");
        for (index, (subtype, width, height, count, last, first)) in streams.iter().enumerate() {
            println!(
                "  stream {index}: {subtype:?} {width}x{height} samples={} first={:.3}s last={:.3}s",
                count & 0x7fff_ffff,
                *first as f64 / 1e7,
                *last as f64 / 1e7
            );
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {}
