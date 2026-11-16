use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::Error;
use crate::install::{self, BROADCASTER, SETTINGS};

const PIPES: &str = r"\\.\pipe\";
const BUS_PIPE: &str = "NvMessageBusBroadcast";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const START_TIMEOUT: Duration = Duration::from_secs(10);
const START_POLL: Duration = Duration::from_millis(100);

pub struct Host {
    child: Option<Child>,
}

pub fn bus_is_up() -> bool {
    fs::read_dir(PIPES).is_ok_and(|pipes| {
        pipes
            .flatten()
            .any(|pipe| pipe.file_name().to_str() == Some(BUS_PIPE))
    })
}

fn stale(source: &Path, copy: &Path) -> bool {
    let (Ok(source), Ok(copy)) = (fs::metadata(source), fs::metadata(copy)) else {
        return true;
    };
    source.len() != copy.len() || source.modified().ok() != copy.modified().ok()
}

fn refresh(source: &Path, copy: &Path) -> Result<(), Error> {
    if stale(source, copy) {
        fs::copy(source, copy)?;
        let modified = fs::metadata(source)?.modified()?;
        fs::File::options()
            .write(true)
            .open(copy)?
            .set_modified(modified)?;
    }
    Ok(())
}

pub fn prepare(source: &Path, plugins: &Path) -> Result<(), Error> {
    fs::create_dir_all(plugins)?;
    for name in [BROADCASTER, SETTINGS] {
        refresh(&source.join(name), &plugins.join(name))?;
    }
    for entry in fs::read_dir(plugins)?.flatten() {
        let name = entry.file_name();
        if name.to_str() != Some(BROADCASTER) && name.to_str() != Some(SETTINGS) {
            return Err(Error::Foreign);
        }
    }
    Ok(())
}

impl Host {
    pub fn start(plugins: PathBuf) -> Result<Self, Error> {
        if bus_is_up() {
            return Ok(Self { child: None });
        }
        let source = install::bus_directory()?;
        let container = install::container()?;
        prepare(&source, &plugins)?;
        let mut child = Command::new(container)
            .arg("-d")
            .arg(&plugins)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        let started = Instant::now();
        while !bus_is_up() {
            if child.try_wait()?.is_some() || started.elapsed() > START_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Missing("канал сообщений не запустился"));
            }
            thread::sleep(START_POLL);
        }
        Ok(Self { child: Some(child) })
    }

    pub fn owned(&self) -> bool {
        self.child.is_some()
    }

    pub fn alive(&mut self) -> bool {
        match &mut self.child {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => bus_is_up(),
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(BROADCASTER), b"broadcaster").unwrap();
        fs::write(directory.path().join(SETTINGS), b"{}").unwrap();
        fs::write(directory.path().join("_nvspcaps64.dll"), b"capture").unwrap();
        directory
    }

    #[test]
    fn only_the_broadcaster_and_its_settings_reach_the_plugin_folder() {
        let source = source();
        let plugins = tempfile::tempdir().unwrap();
        prepare(source.path(), plugins.path()).unwrap();
        let mut names: Vec<_> = fs::read_dir(plugins.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, [BROADCASTER, SETTINGS]);
    }

    #[test]
    fn a_foreign_plugin_in_the_folder_stops_the_start() {
        let source = source();
        let plugins = tempfile::tempdir().unwrap();
        fs::write(plugins.path().join("_nvspcaps64.dll"), b"capture").unwrap();
        assert!(prepare(source.path(), plugins.path()).is_err());
    }

    #[test]
    fn an_updated_broadcaster_replaces_the_old_copy() {
        let source = source();
        let plugins = tempfile::tempdir().unwrap();
        prepare(source.path(), plugins.path()).unwrap();
        fs::write(source.path().join(BROADCASTER), b"newer broadcaster").unwrap();
        prepare(source.path(), plugins.path()).unwrap();
        assert_eq!(
            fs::read(plugins.path().join(BROADCASTER)).unwrap(),
            b"newer broadcaster"
        );
    }
}
