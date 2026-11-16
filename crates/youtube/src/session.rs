use crate::cdp::{Connection, Page};
use crate::chrome::{Browser, profile_directory};
use crate::failure::Failure;
use crate::login;
use crate::publish::PublishSettings;
use crate::studio::{self, Control, Stage};
use crate::window::SignInWindow;
use std::path::{Path, PathBuf};

pub struct Session {
    browser: Browser,
    connection: Connection,
    page: Page,
}

impl Session {
    pub fn open(data_directory: &Path, account_id: &str, headless: bool) -> Result<Self, Failure> {
        let profile = profile_directory(data_directory, account_id);
        let browser = Browser::launch(&profile, headless)?;
        let mut connection = Connection::connect(&browser.websocket_url)?;
        let page = connection.adopt_first_page()?;
        prepare_working_page(&mut connection, &page);
        Ok(Self {
            browser,
            connection,
            page,
        })
    }

    pub fn profile(&self) -> &PathBuf {
        &self.browser.profile
    }

    pub fn is_signed_in(&mut self) -> Result<bool, Failure> {
        let live = login::is_session_live(&mut self.connection, &self.page);
        let _ = self.page.navigate(&mut self.connection, BLANK);
        live
    }

    fn channel_id(&mut self) -> Result<String, Failure> {
        login::wait_for_channel(&mut self.connection, &self.page)
    }

    fn decorations(&mut self) -> Decorations {
        Decorations {
            title: login::channel_name(&mut self.connection, &self.page),
            handle: login::channel_handle(&mut self.connection, &self.page),
            avatar_url: login::channel_avatar(&mut self.connection, &self.page),
            avatar: login::channel_avatar_bytes(&mut self.connection, &self.page),
        }
    }

    pub fn upload(
        &mut self,
        settings: &PublishSettings,
        source: &Path,
        report: &mut dyn FnMut(Stage, u32) -> Control,
    ) -> Result<String, Failure> {
        studio::upload(&mut self.connection, &self.page, settings, source, report)
    }

    pub fn tab(&self) -> Result<Tab, Failure> {
        let mut connection = Connection::connect(&self.browser.websocket_url)?;
        let page = connection.open(BLANK)?;
        prepare_working_page(&mut connection, &page);
        Ok(Tab { connection, page })
    }

    pub fn close(&mut self) {
        self.connection.close_browser();
        self.browser.wait_for_exit(SHUTDOWN_TIMEOUT);
    }
}

pub struct Tab {
    connection: Connection,
    page: Page,
}

impl Tab {
    pub fn upload(
        &mut self,
        settings: &PublishSettings,
        source: &Path,
        report: &mut dyn FnMut(Stage, u32) -> Control,
    ) -> Result<String, Failure> {
        studio::upload(&mut self.connection, &self.page, settings, source, report)
    }

    pub fn download(
        &mut self,
        video_id: &str,
        destination: &Path,
        report: &mut dyn FnMut(u32) -> Control,
    ) -> Result<(), Failure> {
        crate::download::download(
            &mut self.connection,
            &self.page,
            video_id,
            destination,
            report,
        )
    }

    pub fn read_video(&mut self, video_id: &str) -> Result<crate::manage::Details, Failure> {
        crate::manage::read(&mut self.connection, &self.page, video_id)
    }

    pub fn edit_video(
        &mut self,
        video_id: &str,
        changes: &crate::manage::Changes,
    ) -> Result<(), Failure> {
        crate::manage::apply(&mut self.connection, &self.page, video_id, changes)
    }

    pub fn delete_video(&mut self, video_id: &str) -> Result<(), Failure> {
        crate::manage::delete(&mut self.connection, &self.page, video_id)
    }

    pub fn close(mut self) {
        let _ = self.page.close(&mut self.connection);
    }
}

pub const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

const BLANK: &str = "about:blank";

pub const UNNEEDED: [&str; 19] = [
    "*://i.ytimg.com/*",
    "*://i9.ytimg.com/*",
    "*://yt3.ggpht.com/*",
    "*://yt3.googleusercontent.com/*",
    "*://www.gstatic.com/youtube/img/*",
    "*://fonts.gstatic.com/*",
    "*://fonts.googleapis.com/*",
    "*://*.googlevideo.com/videoplayback*",
    "*://*.c.youtube.com/videoplayback*",
    "*://realtimesupport.youtube.com/*",
    "*://casespartner-pa.youtube.com/*",
    "*://ssl.gstatic.com/support/realtime/*",
    "*://play.google.com/log*",
    "*://www.youtube.com/api/stats/*",
    "*://www.youtube.com/ptracking*",
    "*://*/youtubei/v1/log_event*",
    "*://www.google-analytics.com/*",
    "*://www.googletagmanager.com/*",
    "*://*.doubleclick.net/*",
];

fn prepare_working_page(connection: &mut Connection, page: &Page) {
    present_as_a_visible_browser(connection, page);
    let _ = page.block(connection, &UNNEEDED);
}

fn present_as_a_visible_browser(connection: &mut Connection, page: &Page) {
    let Ok(agent) = connection.user_agent() else {
        return;
    };
    if !crate::cdp::is_headless_agent(&agent) {
        return;
    }
    let _ = page.set_user_agent(connection, &crate::cdp::visible_user_agent(&agent));
}

#[derive(Default)]
struct Decorations {
    title: String,
    handle: String,
    avatar_url: String,
    avatar: Option<Vec<u8>>,
}

fn launch_sign_in(
    data_directory: &Path,
    account_id: &str,
    window: &SignInWindow,
) -> Result<Browser, Failure> {
    let profile = profile_directory(data_directory, account_id);
    let (bounds, dpi) = window.launch_bounds();
    Browser::launch_app(&profile, login::SIGN_IN_URL, bounds, dpi)
}

fn shut_down(mut browser: Browser) {
    if let Ok(mut connection) = Connection::connect(&browser.websocket_url) {
        connection.close_browser();
    }
    browser.wait_for_exit(SHUTDOWN_TIMEOUT);
}

fn given_up(browser: Browser, window: &SignInWindow, failure: Failure) -> Failure {
    let failure = if window.is_cancelled() {
        Failure::Cancelled
    } else {
        failure
    };
    shut_down(browser);
    failure
}

pub fn sign_in_within(
    data_directory: &Path,
    account_id: &str,
    now: i64,
    window: &SignInWindow,
    still_open: &mut dyn FnMut() -> bool,
) -> Result<(crate::Account, Option<Vec<u8>>), Failure> {
    let mut browser = launch_sign_in(data_directory, account_id, window)?;
    let keeper = window.keep(browser.pid());
    let port = browser.port;
    let websocket_url = browser.websocket_url.clone();

    let watched = {
        let mut open = || browser.is_running() && still_open() && !window.is_cancelled();
        if login::any_tab_signed_in(port) {
            Ok(())
        } else {
            login::watch_for_sign_in(port, &mut open).map(drop)
        }
    };
    if let Err(failure) = watched {
        drop(keeper);
        return Err(given_up(browser, window, failure));
    }
    window.hide();

    let watched_id = {
        let mut open = || browser.is_running() && still_open() && !window.is_cancelled();
        login::watch_for_channel_id(port, &mut open)
    };
    if window.is_cancelled() {
        drop(keeper);
        return Err(given_up(browser, window, Failure::Cancelled));
    }

    let mut session = match Connection::connect(&websocket_url) {
        Ok(mut connection) => {
            let page = connection.adopt_first_page()?;
            Some(Session {
                browser,
                connection,
                page,
            })
        }

        Err(error) if watched_id.is_none() => {
            drop(keeper);
            return Err(given_up(browser, window, error));
        }
        Err(_) => {
            shut_down(browser);
            None
        }
    };

    let outcome = match (watched_id, session.as_mut()) {
        (Some(id), Some(session)) => Ok((id, session.decorations())),
        (Some(id), None) => Ok((id, Decorations::default())),
        (None, Some(session)) => session.channel_id().map(|id| {
            let shown = session.decorations();
            (id, shown)
        }),
        (None, None) => Err(Failure::NoChannel),
    };

    if let Some(session) = session.as_mut() {
        session.close();
    }
    drop(session);
    drop(keeper);

    let (id, shown) = outcome?;

    Ok((
        crate::Account {
            id,
            title: shown.title,
            handle: shown.handle,
            avatar_url: shown.avatar_url,
            avatar_file: None,
            added_at: now,
            refreshed_at: now,
            needs_reauth: false,
        },
        shown.avatar,
    ))
}

const DECORATION_ATTEMPTS: usize = 15;
const DECORATION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

pub struct ChannelDecorations {
    pub title: String,
    pub handle: String,
    pub avatar_url: String,
    pub avatar: Option<Vec<u8>>,
}

pub fn refresh_decorations(
    data_directory: &Path,
    account_id: &str,
) -> Result<ChannelDecorations, Failure> {
    let profile = profile_directory(data_directory, account_id);
    let mut browser = Browser::launch_at(&profile, true, login::STUDIO_URL)?;
    let websocket_url = browser.websocket_url.clone();

    let mut connection = match Connection::connect(&websocket_url) {
        Ok(connection) => connection,
        Err(error) => {
            browser.wait_for_exit(SHUTDOWN_TIMEOUT);
            return Err(error);
        }
    };
    let page = match connection.open(login::STUDIO_URL) {
        Ok(page) => page,
        Err(error) => {
            connection.close_browser();
            browser.wait_for_exit(SHUTDOWN_TIMEOUT);
            return Err(error);
        }
    };

    present_as_a_visible_browser(&mut connection, &page);
    let _ = page.navigate(&mut connection, login::STUDIO_URL);

    let mut session = Session {
        browser,
        connection,
        page,
    };

    let mut shown = Decorations::default();
    for _ in 0..DECORATION_ATTEMPTS {
        std::thread::sleep(DECORATION_INTERVAL);
        shown = session.decorations();
        if !shown.title.trim().is_empty() {
            break;
        }
    }
    session.close();

    Ok(ChannelDecorations {
        title: shown.title,
        handle: shown.handle,
        avatar_url: shown.avatar_url,
        avatar: shown.avatar,
    })
}

pub fn forget_profile(data_directory: &Path, account_id: &str) -> std::io::Result<()> {
    let profile = profile_directory(data_directory, account_id);
    match std::fs::remove_dir_all(&profile) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

pub fn is_supported() -> bool {
    crate::chrome::find().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgetting_a_profile_that_was_never_created_is_not_an_error() {
        let directory = std::env::temp_dir().join(format!(
            "rewind-youtube-forget-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        assert!(forget_profile(&directory, "UCmissing").is_ok());

        let profile = profile_directory(&directory, "UCreal");
        std::fs::create_dir_all(profile.join("Default")).expect("create");
        std::fs::write(profile.join("Default").join("Cookies"), b"session").expect("write");
        assert!(profile.exists());

        forget_profile(&directory, "UCreal").expect("forget");
        assert!(!profile.exists(), "the cookies have to actually go");
        assert!(
            forget_profile(&directory, "UCreal").is_ok(),
            "and again is fine"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn removing_one_account_leaves_the_others_signed_in() {
        let directory = std::env::temp_dir().join(format!(
            "rewind-youtube-profiles-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&directory);

        for account in ["UCfirst", "UCsecond"] {
            std::fs::create_dir_all(profile_directory(&directory, account)).expect("create");
        }
        forget_profile(&directory, "UCfirst").expect("forget");

        assert!(!profile_directory(&directory, "UCfirst").exists());
        assert!(profile_directory(&directory, "UCsecond").exists());

        let _ = std::fs::remove_dir_all(&directory);
    }

    fn footprint(profile: &Path) -> (usize, f64, f64) {
        let script = format!(
            "Get-CimInstance Win32_Process -Filter \"Name='chrome.exe'\" | Where-Object {{ $_.CommandLine -like '*{}*' }} | ForEach-Object {{ $kind = if ($_.CommandLine -match '--utility-sub-type=([a-zA-Z_.]+)') {{ $Matches[1] }} elseif ($_.CommandLine -match '--type=([a-z-]+)') {{ $Matches[1] }} else {{ 'browser' }}; \"$($_.WorkingSetSize) $($_.PrivatePageCount) $kind\" }}",
            profile.display()
        );
        let listing = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&listing.stdout);
        let mut count = 0;
        let (mut working, mut private) = (0.0, 0.0);
        for line in text.lines() {
            let mut numbers = line
                .split_whitespace()
                .map(|value| value.parse::<f64>().unwrap_or(0.0));
            working += numbers.next().unwrap_or(0.0);
            private += numbers.next().unwrap_or(0.0);
            count += 1;
        }
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let private = parts
                .nth(1)
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0);
            println!(
                "    {:>5.0} MB  {}",
                private / 1_048_576.0,
                parts.next().unwrap_or("?")
            );
        }
        (count, working / 1_048_576.0, private / 1_048_576.0)
    }

    fn measure(label: &str, profile: &Path) {
        std::thread::sleep(std::time::Duration::from_secs(8));
        let (count, working, private) = footprint(profile);
        println!(
            "{label}: {count} processes, working set {working:.0} MB, private {private:.0} MB"
        );
    }

    fn upload_dialog(tab: &mut Tab) {
        tab.page
            .navigate(&mut tab.connection, crate::studio::UPLOAD_URL)
            .unwrap();
        tab.page
            .wait_until(
                &mut tab.connection,
                &format!(
                    "(() => !!{})()",
                    crate::cdp::query(crate::selectors::studio::SELECT_FILES)
                ),
                std::time::Duration::from_secs(60),
                "upload dialog",
            )
            .unwrap();
    }

    fn heap(tab: &mut Tab, label: &str) {
        let metrics = tab
            .page
            .call(
                &mut tab.connection,
                "Performance.getMetrics",
                serde_json::json!({}),
            )
            .unwrap();
        let pick = |name: &str| {
            metrics["metrics"]
                .as_array()
                .and_then(|all| all.iter().find(|metric| metric["name"] == name))
                .and_then(|metric| metric["value"].as_f64())
                .unwrap_or(0.0)
        };
        println!(
            "{label}: js used {:.0} MB, js total {:.0} MB, nodes {:.0}, documents {:.0}, frames {:.0}, listeners {:.0}",
            pick("JSHeapUsedSize") / 1_048_576.0,
            pick("JSHeapTotalSize") / 1_048_576.0,
            pick("Nodes"),
            pick("Documents"),
            pick("Frames"),
            pick("JSEventListeners"),
        );
    }

    #[test]
    #[ignore = "inspects what the upload dialog of a real signed-in profile keeps in memory; chooses no file; set REWIND_YOUTUBE_DATA and REWIND_ACCOUNT"]
    fn the_upload_dialog_is_dissected() {
        let directory = PathBuf::from(std::env::var("REWIND_YOUTUBE_DATA").unwrap());
        let account = std::env::var("REWIND_ACCOUNT").unwrap();
        let profile = profile_directory(&directory, &account);
        let mut session = Session::open(&directory, &account, true).unwrap();
        let mut tab = session.tab().unwrap();
        tab.page
            .call(
                &mut tab.connection,
                "Performance.enable",
                serde_json::json!({}),
            )
            .unwrap();
        upload_dialog(&mut tab);
        measure("upload dialog", &profile);
        heap(&mut tab, "loaded");
        let listing = tab
            .page
            .eval(
                &mut tab.connection,
                "(() => { const rows = {}; for (const entry of performance.getEntriesByType('resource')) { let host = ''; try { const url = new URL(entry.name); host = url.host + url.pathname.split('/').slice(0, 3).join('/'); } catch (error) {} const key = entry.initiatorType + ' ' + host; const row = rows[key] || (rows[key] = [0, 0]); row[0] += 1; row[1] += entry.decodedBodySize || 0; } return Object.entries(rows).sort((a, b) => b[1][1] - a[1][1]).slice(0, 40).map(([key, row]) => (row[1] / 1048576).toFixed(2) + ' MB x' + row[0] + ' ' + key).join(String.fromCharCode(10)); })()",
            )
            .unwrap();
        println!("{}", listing.as_str().unwrap_or("?"));
        let frames = tab
            .page
            .eval(
                &mut tab.connection,
                "(() => [...document.querySelectorAll('iframe')].map(frame => frame.src.slice(0, 90)).join(String.fromCharCode(10)))()",
            )
            .unwrap();
        println!(
            "frames:
{}",
            frames.as_str().unwrap_or("?")
        );
        tab.page.collect_garbage(&mut tab.connection).unwrap();
        measure("after a collection", &profile);
        heap(&mut tab, "collected");
        println!("trimmed {} processes", crate::chrome::trim_now());
        measure("after a trim", &profile);
        std::thread::sleep(std::time::Duration::from_secs(30));
        measure("half a minute after the trim", &profile);
        let wait = std::time::Duration::from_secs(30);
        tab.connection
            .call(
                None,
                "Tracing.start",
                serde_json::json!({ "traceConfig": {
                    "includedCategories": ["disabled-by-default-memory-infra"],
                    "excludedCategories": ["*"],
                    "memoryDumpConfig": { "triggers": [] },
                } }),
                wait,
            )
            .unwrap();
        tab.connection
            .call(
                None,
                "Tracing.requestMemoryDump",
                serde_json::json!({ "levelOfDetail": "detailed" }),
                wait,
            )
            .unwrap();
        tab.connection
            .call(None, "Tracing.end", serde_json::json!({}), wait)
            .unwrap();
        let mut biggest: (f64, Vec<(f64, String)>) = (0.0, Vec::new());
        while let Ok(chunk) = tab
            .connection
            .wait_for_event("Tracing.dataCollected", std::time::Duration::from_secs(8))
        {
            for event in chunk["params"]["value"].as_array().into_iter().flatten() {
                let Some(allocators) = event["args"]["dumps"]["allocators"].as_object() else {
                    continue;
                };
                let size = |name: &str| {
                    allocators
                        .get(name)
                        .and_then(|dump| dump["attrs"]["size"]["value"].as_str())
                        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
                        .unwrap_or(0) as f64
                        / 1_048_576.0
                };
                let rows: Vec<(f64, String)> = allocators
                    .keys()
                    .filter(|name| name.matches('/').count() <= 1)
                    .map(|name| (size(name), name.clone()))
                    .filter(|(megabytes, _)| *megabytes >= 2.0)
                    .collect();
                let total: f64 = allocators
                    .keys()
                    .filter(|name| !name.contains('/'))
                    .map(|name| size(name))
                    .sum();
                if total > biggest.0 {
                    biggest = (total, rows);
                }
            }
        }
        biggest.1.sort_by(|left, right| right.0.total_cmp(&left.0));
        println!("largest process by allocators: {:.0} MB", biggest.0);
        for (megabytes, name) in &biggest.1 {
            println!("    {megabytes:>6.1} MB  {name}");
        }
        assert!(
            tab.page
                .eval_bool(
                    &mut tab.connection,
                    &format!(
                        "(() => !!{})()",
                        crate::cdp::query(crate::selectors::studio::SELECT_FILES)
                    ),
                )
                .unwrap()
        );
        tab.close();
        session.close();
    }

    #[test]
    #[ignore = "measures the browser of a real signed-in profile; opens the upload dialog without choosing a file; set REWIND_YOUTUBE_DATA, REWIND_ACCOUNT and REWIND_VIDEO"]
    fn the_browser_footprint_is_measured() {
        let directory = PathBuf::from(std::env::var("REWIND_YOUTUBE_DATA").unwrap());
        let account = std::env::var("REWIND_ACCOUNT").unwrap();
        let video = std::env::var("REWIND_VIDEO").unwrap();
        let profile = profile_directory(&directory, &account);
        let mut session = Session::open(&directory, &account, true).unwrap();
        measure("launched", &profile);
        assert!(session.is_signed_in().unwrap());
        measure("idle after the sign-in check", &profile);

        let mut first = session.tab().unwrap();
        first.read_video(&video).unwrap();
        measure("one tab on the edit page", &profile);

        upload_dialog(&mut first);
        measure("one tab on the upload dialog", &profile);

        let mut more = vec![session.tab().unwrap(), session.tab().unwrap()];
        for tab in &mut more {
            upload_dialog(tab);
        }
        measure("three tabs on the upload dialog", &profile);
        first.page.collect_garbage(&mut first.connection).unwrap();
        measure("three tabs after a collection", &profile);
        println!("trimmed {} processes", crate::chrome::trim_now());
        measure("three tabs after a trim", &profile);

        for tab in more {
            tab.close();
        }
        first.close();
        measure("all tabs closed", &profile);
        session.close();
        let deadline = std::time::Instant::now() + SHUTDOWN_TIMEOUT;
        while footprint(&profile).0 > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let (left, _, _) = footprint(&profile);
        println!("left after close: {left} processes");
        assert_eq!(left, 0);
    }
}
