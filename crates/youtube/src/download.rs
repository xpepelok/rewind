use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::cdp::{Connection, Page};
use crate::failure::Failure;
use crate::studio::{Control, edit_url};

const LINK_TIMEOUT: Duration = Duration::from_secs(60);
const START_TIMEOUT: Duration = Duration::from_secs(60);
const STALL_TIMEOUT: Duration = Duration::from_secs(90);
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(500);
const TICK: Duration = Duration::from_millis(250);
const BEGIN: &str = "Browser.downloadWillBegin";
const PROGRESS: &str = "Browser.downloadProgress";
const LINK_SCRIPT: &str = r#"(() => {
  const found = document.documentElement.outerHTML.match(/"downloadUrl":"([^"]+)"/);
  return found ? JSON.parse('"' + found[1] + '"') : '';
})()"#;

pub fn download(
    connection: &mut Connection,
    page: &Page,
    video_id: &str,
    destination: &Path,
    report: &mut dyn FnMut(u32) -> Control,
) -> Result<(), Failure> {
    let directory = destination
        .parent()
        .ok_or_else(|| Failure::Protocol(destination.display().to_string()))?;
    std::fs::create_dir_all(directory).map_err(|error| Failure::Protocol(error.to_string()))?;
    page.navigate(connection, &edit_url(video_id))?;
    let link = owner_link(connection, page, report)?;
    connection.discard_events(BEGIN);
    connection.discard_events(PROGRESS);
    connection.call(
        None,
        "Browser.setDownloadBehavior",
        json!({
            "behavior": "allowAndName",
            "downloadPath": directory.display().to_string(),
            "eventsEnabled": true,
        }),
        CALL_TIMEOUT,
    )?;
    page.block(connection, &[])?;
    page.navigate(connection, &format!("https://www.youtube.com{link}"))
        .ok();
    let guid = started(connection, page, report)?;
    let partial = directory.join(&guid);
    let mut stalled = Instant::now() + STALL_TIMEOUT;
    let mut done = 0;
    loop {
        if report(done) == Control::Cancel {
            connection
                .call(
                    None,
                    "Browser.cancelDownload",
                    json!({ "guid": guid }),
                    CALL_TIMEOUT,
                )
                .ok();
            std::fs::remove_file(&partial).ok();
            return Err(Failure::Cancelled);
        }
        if Instant::now() >= stalled {
            std::fs::remove_file(&partial).ok();
            return Err(Failure::Timeout(PROGRESS.to_string()));
        }
        let Some(event) = next_event(connection, PROGRESS)? else {
            continue;
        };
        let params = &event["params"];
        if params["guid"].as_str() != Some(guid.as_str()) {
            continue;
        }
        stalled = Instant::now() + STALL_TIMEOUT;
        match params["state"].as_str().unwrap_or_default() {
            "completed" => {
                std::fs::rename(&partial, destination)
                    .map_err(|error| Failure::Protocol(error.to_string()))?;
                return Ok(());
            }
            "canceled" => {
                std::fs::remove_file(&partial).ok();
                return Err(Failure::Rejected("download".to_string()));
            }
            _ => done = percent(params),
        }
    }
}

fn next_event(connection: &mut Connection, method: &str) -> Result<Option<Value>, Failure> {
    match connection.wait_for_event(method, TICK) {
        Ok(event) => Ok(Some(event)),
        Err(Failure::Timeout(_)) => Ok(None),
        Err(failure) => Err(failure),
    }
}

fn owner_link(
    connection: &mut Connection,
    page: &Page,
    report: &mut dyn FnMut(u32) -> Control,
) -> Result<String, Failure> {
    let deadline = Instant::now() + LINK_TIMEOUT;
    while Instant::now() < deadline {
        std::thread::sleep(POLL);
        if report(0) == Control::Cancel {
            return Err(Failure::Cancelled);
        }
        if page
            .url(connection)
            .is_ok_and(|url| url.contains("accounts.google.com"))
        {
            return Err(Failure::SignedOut);
        }
        let link = page
            .eval_string(connection, LINK_SCRIPT)
            .unwrap_or_default();
        if link.starts_with("/download_my_video") {
            return Ok(link);
        }
    }
    Err(Failure::Timeout("downloadUrl".to_string()))
}

fn started(
    connection: &mut Connection,
    page: &Page,
    report: &mut dyn FnMut(u32) -> Control,
) -> Result<String, Failure> {
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if report(0) == Control::Cancel {
            return Err(Failure::Cancelled);
        }
        let Some(event) = next_event(connection, BEGIN)? else {
            continue;
        };
        let params = &event["params"];
        if params["frameId"].as_str() == Some(page.target_id.as_str())
            && let Some(guid) = params["guid"].as_str()
        {
            return Ok(guid.to_string());
        }
    }
    Err(Failure::Timeout(BEGIN.to_string()))
}

fn percent(params: &Value) -> u32 {
    let received = params["receivedBytes"].as_f64().unwrap_or(0.0);
    let total = params["totalBytes"].as_f64().unwrap_or(0.0);
    if total > 0.0 {
        ((received / total) * 100.0).clamp(0.0, 100.0) as u32
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_a_clamped_percentage() {
        assert_eq!(
            percent(&json!({"receivedBytes": 50, "totalBytes": 200})),
            25
        );
        assert_eq!(percent(&json!({"receivedBytes": 50, "totalBytes": 0})), 0);
        assert_eq!(
            percent(&json!({"receivedBytes": 300, "totalBytes": 200})),
            100
        );
    }
}
