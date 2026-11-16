use std::path::Path;

use rewind_config::QuickShare;
use rewind_media::{AUDIO_BITS_PER_SECOND, Edit, Span, export, probe};

use crate::editor_files::{SharePlan, share_file, share_plan};

pub mod text {
    pub const PREPARING: &str = "Готовлю клип…";
    pub const FAILED: &str = "Не удалось подготовить клип";
    pub const HINT: &str =
        "Обрежет выбранный отрезок, сожмёт его под лимит и положит файл в буфер обмена.";

    pub const READY: &str = "Клип в буфере обмена — вставьте его в мессенджер (Ctrl+V)";

    pub fn size(megabytes: f64, over: bool) -> String {
        if over {
            format!("{megabytes:.1} МБ — чуть больше лимита, обрежьте клип ещё немного")
        } else {
            format!("{megabytes:.1} МБ")
        }
    }

    pub fn too_long(app: &str, seconds: u32) -> String {
        format!("Слишком длинный клип для {app}: обрежьте его примерно до {seconds} с")
    }
}

pub enum ShareError {
    TooLong(u32),
    Failed(String),
}

pub struct SharedClip {
    pub bytes: u64,
    pub over: bool,
}

pub fn share_clip(
    source: &Path,
    edit: Option<Edit>,
    target: QuickShare,
    report: &dyn Fn(f32),
) -> Result<SharedClip, ShareError> {
    let _runtime = rewind_media::Runtime::enter();
    let info = probe(source).map_err(|error| ShareError::Failed(error.to_string()))?;
    let mut edit = edit.unwrap_or_else(|| Edit::new(Span::whole(info.duration)));
    let source_height = {
        let mut whole = edit.clone();
        whole.height = None;
        whole.output_size(info.width, info.height).1.1
    };
    let limit = target.limit_bytes();
    let file_size = std::fs::metadata(source).map_or(0, |meta| meta.len());
    match share_plan(
        limit,
        file_size,
        info.duration,
        edit.span.length(),
        source_height,
        edit.needs_render(),
        AUDIO_BITS_PER_SECOND,
    ) {
        SharePlan::Copy if !edit.needs_render() && edit.span == Span::whole(info.duration) => {
            crate::platform::clipboard::copy_file(source).map_err(ShareError::Failed)?;
            return Ok(SharedClip {
                bytes: file_size,
                over: file_size > limit,
            });
        }
        SharePlan::Copy => {}
        SharePlan::Encode { bitrate, height } => {
            edit.bitrate = Some(bitrate);
            if let Some(height) = height.filter(|height| *height < source_height) {
                edit.height = Some(height);
            }
        }
        SharePlan::TooLong { max_seconds } => return Err(ShareError::TooLong(max_seconds)),
    }
    let destination = share_file(source);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| ShareError::Failed(error.to_string()))?;
    }
    std::fs::remove_file(&destination).ok();
    export(source, &edit, &destination, report)
        .map_err(|error| ShareError::Failed(error.to_string()))?;
    crate::platform::clipboard::copy_file(&destination).map_err(ShareError::Failed)?;
    let bytes = std::fs::metadata(&destination).map_or(0, |meta| meta.len());
    Ok(SharedClip {
        bytes,
        over: bytes > limit,
    })
}

pub fn outcome_toast(
    target: QuickShare,
    outcome: &Result<SharedClip, ShareError>,
) -> (String, Option<String>, bool) {
    match outcome {
        Ok(clip) => (
            text::READY.to_owned(),
            Some(text::size(clip.bytes as f64 / 1_000_000.0, clip.over)),
            false,
        ),
        Err(ShareError::TooLong(seconds)) => (text::too_long(target.app(), *seconds), None, true),
        Err(ShareError::Failed(reason)) => (text::FAILED.to_owned(), Some(reason.clone()), true),
    }
}
