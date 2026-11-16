use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};

use crate::ansel::{CAMERA, CONTROL, Incoming, KIND, Request, Response};
use crate::session::{Command, Machine, State};
use crate::{Address, Bus, Host, Message};

const TICK: Duration = Duration::from_secs(2);
const RETRY: Duration = Duration::from_secs(10);
const BATCH: usize = 256;

enum Event {
    Bus(Message),
    Command(Command),
    Stop,
}

pub struct Camera {
    sender: mpsc::Sender<Event>,
    shared: Arc<Mutex<State>>,
    worker: Option<JoinHandle<()>>,
}

struct Link {
    host: Host,
    bus: Bus,
}

fn connect(plugins: &Path, sender: &mpsc::Sender<Event>) -> Option<Link> {
    let host = Host::start(plugins.to_path_buf()).ok()?;
    let sender = sender.clone();
    let bus = Bus::join(Address::new(CONTROL.0, CONTROL.1), move |message| {
        let _ = sender.send(Event::Bus(message));
    })
    .ok()?;
    Some(Link { host, bus })
}

pub(crate) fn running(process: u32) -> bool {
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, process) }) else {
        return false;
    };
    let alive = unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
    let _ = unsafe { CloseHandle(handle) };
    alive
}

fn settle(batch: impl IntoIterator<Item = Event>) -> Vec<Event> {
    let mut settled: Vec<Event> = Vec::new();
    for event in batch {
        let repeated = matches!(
            (&event, settled.last()),
            (Event::Command(command), Some(Event::Command(earlier))) if command.replaces(earlier)
        );
        if repeated {
            settled.pop();
        }
        settled.push(event);
    }
    settled
}

struct Worker {
    plugins: PathBuf,
    sender: mpsc::Sender<Event>,
    shared: Arc<Mutex<State>>,
    changed: Box<dyn Fn() + Send>,
    machine: Machine,
    link: Option<Link>,
    camera: Address,
    retry_at: Instant,
}

impl Worker {
    fn post(&self, requests: Vec<Request>) {
        let Some(link) = &self.link else {
            return;
        };
        for request in requests {
            let _ = link.bus.post(&self.camera, KIND, &request.encode());
        }
    }

    fn heard(&mut self, message: Message) -> Vec<Request> {
        let mut requests = Vec::new();
        let from_camera = message.source == self.camera;
        if !message.peers.is_empty() {
            let present = from_camera || message.peers.contains(&self.camera);
            requests.extend(self.machine.seen(present, 0));
        }
        if !from_camera {
            return requests;
        }
        requests.extend(self.machine.seen(true, message.process));
        if message.kind.is_none() {
            return requests;
        }
        if let Some(incoming) = Incoming::decode(&message.data) {
            if let (Some(reply), Some(link)) = (incoming.reply(), &self.link) {
                let _ = link.bus.post(&self.camera, KIND, &reply);
            }
            requests.extend(self.machine.incoming(&incoming));
        } else if let Some(response) = Response::decode(&message.data) {
            requests.extend(self.machine.response(response));
        }
        requests
    }

    fn watch(&mut self) {
        let lost = self.link.as_mut().is_some_and(|link| !link.host.alive());
        if lost {
            self.link = None;
            self.machine.seen(false, 0);
        }
        if self.link.is_none() && Instant::now() >= self.retry_at {
            self.link = connect(&self.plugins, &self.sender);
            self.retry_at = Instant::now() + RETRY;
        }
        let state = self.machine.state();
        if state.present && state.process != 0 && !running(state.process) {
            self.machine.seen(false, 0);
        }
    }

    fn publish(&self) {
        let Ok(mut shared) = self.shared.lock() else {
            return;
        };
        if *shared == *self.machine.state() {
            return;
        }
        shared.clone_from(self.machine.state());
        drop(shared);
        (self.changed)();
    }

    fn run(mut self, events: mpsc::Receiver<Event>) {
        self.watch();
        let mut watched = Instant::now();
        let mut serving = true;
        while serving {
            let first = match events.recv_timeout(TICK) {
                Ok(event) => Some(event),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let batch = first
                .into_iter()
                .chain(events.try_iter().take(BATCH))
                .collect::<Vec<_>>();
            for event in settle(batch) {
                let requests = match event {
                    Event::Bus(message) => self.heard(message),
                    Event::Command(command) => self.machine.command(command),
                    Event::Stop => {
                        serving = false;
                        break;
                    }
                };
                self.post(requests);
            }
            if watched.elapsed() >= TICK {
                self.watch();
                watched = Instant::now();
            }
            self.publish();
        }
        if self.machine.state().mode != crate::session::Mode::Off {
            let closing = self.machine.command(Command::Close);
            self.post(closing);
        }
    }
}

impl Camera {
    pub fn start(plugins: PathBuf, changed: impl Fn() + Send + 'static) -> Self {
        let (sender, events) = mpsc::channel();
        let shared = Arc::new(Mutex::new(State::default()));
        let worker = Worker {
            plugins,
            sender: sender.clone(),
            shared: shared.clone(),
            changed: Box::new(changed),
            machine: Machine::default(),
            link: None,
            camera: Address::new(CAMERA.0, CAMERA.1),
            retry_at: Instant::now(),
        };
        let worker = thread::Builder::new()
            .name("rewind-camera".to_owned())
            .spawn(move || worker.run(events))
            .ok();
        Self {
            sender,
            shared,
            worker,
        }
    }

    pub fn state(&self) -> State {
        self.shared
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default()
    }

    pub fn send(&self, command: Command) {
        let _ = self.sender.send(Event::Command(command));
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let _ = self.sender.send(Event::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(events: Vec<Event>) -> Vec<Command> {
        events
            .into_iter()
            .filter_map(|event| match event {
                Event::Command(command) => Some(command),
                Event::Bus(_) | Event::Stop => None,
            })
            .collect()
    }

    #[test]
    fn a_burst_from_one_slider_keeps_only_its_last_value() {
        let set = |control: i32, value: f32| {
            Event::Command(Command::Set {
                layer: 0,
                control,
                component: 0,
                value,
            })
        };
        let settled = settle([
            Event::Command(Command::Fov(60.0)),
            Event::Command(Command::Fov(61.0)),
            Event::Command(Command::Fov(62.0)),
            set(1, 0.1),
            set(1, 0.2),
            set(2, 0.5),
            Event::Command(Command::Fov(63.0)),
        ]);
        assert_eq!(
            commands(settled),
            [
                Command::Fov(62.0),
                Command::Set {
                    layer: 0,
                    control: 1,
                    component: 0,
                    value: 0.2
                },
                Command::Set {
                    layer: 0,
                    control: 2,
                    component: 0,
                    value: 0.5
                },
                Command::Fov(63.0),
            ]
        );
    }

    #[test]
    fn commands_of_other_kinds_are_never_dropped() {
        let settled = settle([
            Event::Command(Command::Grid(true)),
            Event::Command(Command::Grid(false)),
            Event::Command(Command::Remove(0)),
            Event::Command(Command::Remove(0)),
        ]);
        assert_eq!(settled.len(), 4);
    }
}
