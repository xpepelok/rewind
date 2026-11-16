use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use rewind_capture::ReplayHealth;

pub const INTERVAL: Duration = Duration::from_secs(60);
pub const RETRY_DELAY: Duration = Duration::from_secs(8);
pub const RETRY_LIMIT: u32 = 5;
const LOG_FILE: &str = "replay.log";
const LOG_ROTATED_SUFFIX: &str = ".old";
const LOG_LIMIT: u64 = 1024 * 1024;
const TURNED_OFF: &str = "повтор выключился сам";
const NOT_RESPONDING: &str = "захват экрана не отвечает";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Nothing,
    CheckSoon,
    Recovered,
    Enable { attempt: u32, reason: String },
    Restart { attempt: u32, reason: String },
    GiveUp(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayWatch {
    attempts: u32,
    repairing: bool,
    reported: bool,
}

fn reason(health: &ReplayHealth) -> String {
    match health {
        ReplayHealth::Off => TURNED_OFF.to_owned(),
        ReplayHealth::Stalled(Some(reason)) => reason.clone(),
        _ => NOT_RESPONDING.to_owned(),
    }
}

impl ReplayWatch {
    pub fn begin_cycle(&mut self) {
        self.attempts = 0;
    }

    pub fn step(&mut self, wanted: bool, health: &ReplayHealth) -> Action {
        if !wanted {
            *self = Self::default();
            return Action::Nothing;
        }
        match health {
            ReplayHealth::Running | ReplayHealth::Waiting => {
                let recovered = self.repairing;
                *self = Self::default();
                if recovered {
                    Action::Recovered
                } else {
                    Action::Nothing
                }
            }
            ReplayHealth::Starting => Action::CheckSoon,
            ReplayHealth::Off | ReplayHealth::Stalled(_) if self.attempts >= RETRY_LIMIT => {
                if self.reported {
                    Action::Nothing
                } else {
                    self.reported = true;
                    Action::GiveUp(reason(health))
                }
            }
            ReplayHealth::Off => {
                self.attempts += 1;
                self.repairing = true;
                Action::Enable {
                    attempt: self.attempts,
                    reason: reason(health),
                }
            }
            ReplayHealth::Stalled(_) => {
                self.attempts += 1;
                self.repairing = true;
                Action::Restart {
                    attempt: self.attempts,
                    reason: reason(health),
                }
            }
        }
    }
}

pub fn log(directory: &Path, line: &str) {
    log_to(directory, LOG_FILE, line);
}

pub fn log_to(directory: &Path, name: &str, line: &str) {
    if fs::create_dir_all(directory).is_err() {
        return;
    }
    let file = directory.join(name);
    if fs::metadata(&file).is_ok_and(|meta| meta.len() > LOG_LIMIT) {
        fs::rename(&file, directory.join(format!("{name}{LOG_ROTATED_SUFFIX}"))).ok();
    }
    if let Ok(mut out) = OpenOptions::new().create(true).append(true).open(&file) {
        writeln!(out, "{} {line}", timestamp()).ok();
    }
}

#[cfg(windows)]
fn timestamp() -> String {
    let now = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
    )
}

#[cfg(not(windows))]
fn timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stalled() -> ReplayHealth {
        ReplayHealth::Stalled(Some("no hardware video encoder".to_owned()))
    }

    #[test]
    fn a_healthy_replay_needs_nothing() {
        let mut watch = ReplayWatch::default();
        watch.begin_cycle();
        assert_eq!(watch.step(true, &ReplayHealth::Running), Action::Nothing);
        assert_eq!(watch.step(true, &ReplayHealth::Waiting), Action::Nothing);
    }

    #[test]
    fn replay_the_user_turned_off_is_left_alone() {
        let mut watch = ReplayWatch::default();
        watch.begin_cycle();
        assert_eq!(watch.step(false, &ReplayHealth::Off), Action::Nothing);
    }

    #[test]
    fn replay_that_turned_itself_off_is_turned_back_on() {
        let mut watch = ReplayWatch::default();
        watch.begin_cycle();
        assert_eq!(
            watch.step(true, &ReplayHealth::Off),
            Action::Enable {
                attempt: 1,
                reason: TURNED_OFF.to_owned()
            }
        );
        assert_eq!(watch.step(true, &ReplayHealth::Starting), Action::CheckSoon);
        assert_eq!(watch.step(true, &ReplayHealth::Running), Action::Recovered);
        assert_eq!(watch.step(true, &ReplayHealth::Running), Action::Nothing);
    }

    #[test]
    fn five_failed_restarts_raise_one_alert_per_outage() {
        let mut watch = ReplayWatch::default();
        watch.begin_cycle();
        for attempt in 1..=RETRY_LIMIT {
            assert_eq!(
                watch.step(true, &stalled()),
                Action::Restart {
                    attempt,
                    reason: "no hardware video encoder".to_owned()
                }
            );
        }
        assert_eq!(
            watch.step(true, &stalled()),
            Action::GiveUp("no hardware video encoder".to_owned())
        );
        assert_eq!(watch.step(true, &stalled()), Action::Nothing);

        watch.begin_cycle();
        assert!(matches!(
            watch.step(true, &stalled()),
            Action::Restart { attempt: 1, .. }
        ));
        for _ in 1..RETRY_LIMIT {
            watch.step(true, &stalled());
        }
        assert_eq!(watch.step(true, &stalled()), Action::Nothing);

        assert_eq!(watch.step(true, &ReplayHealth::Running), Action::Recovered);
        watch.begin_cycle();
        for _ in 0..RETRY_LIMIT {
            watch.step(true, &stalled());
        }
        assert!(matches!(watch.step(true, &stalled()), Action::GiveUp(_)));
    }

    #[test]
    fn a_stall_without_a_known_cause_still_says_why() {
        let mut watch = ReplayWatch::default();
        watch.begin_cycle();
        assert_eq!(
            watch.step(true, &ReplayHealth::Stalled(None)),
            Action::Restart {
                attempt: 1,
                reason: NOT_RESPONDING.to_owned()
            }
        );
    }

    #[test]
    fn log_lines_are_appended_with_a_timestamp() {
        let directory = tempfile::tempdir().unwrap();
        log(directory.path(), "первая");
        log(directory.path(), "вторая");
        let text = fs::read_to_string(directory.path().join(LOG_FILE)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" первая"));
        assert!(lines[1].ends_with(" вторая"));
        assert!(lines[0].starts_with("20"));
    }
}
