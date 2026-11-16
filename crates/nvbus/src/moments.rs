use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::camera::running;
use crate::desk::{Choice, Desk, Moment};
use crate::highlights::{self, SERVERS};
use crate::{Address, Bus, Host, Message};

const TICK: Duration = Duration::from_secs(2);
const RETRY: Duration = Duration::from_secs(10);

enum Event {
    Bus(usize, Message),
    Choice(Choice),
    Stop,
}

pub struct Moments {
    sender: mpsc::Sender<Event>,
    worker: Option<JoinHandle<()>>,
}

struct Link {
    host: Host,
    buses: Vec<Bus>,
}

fn connect(plugins: &Path, sender: &mpsc::Sender<Event>) -> Option<Link> {
    let host = Host::start(plugins.to_path_buf()).ok()?;
    let buses = SERVERS
        .iter()
        .enumerate()
        .filter_map(|(index, (system, module))| {
            let sender = sender.clone();
            Bus::join(Address::new(system, module), move |message| {
                let _ = sender.send(Event::Bus(index, message));
            })
            .ok()
        })
        .collect::<Vec<_>>();
    (buses.len() == SERVERS.len()).then_some(Link { host, buses })
}

struct Worker {
    plugins: PathBuf,
    sender: mpsc::Sender<Event>,
    heard: Box<dyn Fn(Moment) + Send>,
    desk: Desk,
    link: Option<Link>,
    retry_at: Instant,
}

impl Worker {
    fn tell(&self, moments: Vec<Moment>) {
        for moment in moments {
            (self.heard)(moment);
        }
    }

    fn handle(&mut self, index: usize, message: Message) {
        let Some(kind) = message.kind else {
            return;
        };
        let Some(envelope) = highlights::decode(&message.data) else {
            return;
        };
        let (reply, moments) = self.desk.handle(envelope);
        if let Some(bus) = self.link.as_ref().and_then(|link| link.buses.get(index)) {
            let _ = bus.reply(&message, kind, &reply.encode());
        }
        self.tell(moments);
    }

    fn watch(&mut self) {
        if self.link.as_mut().is_some_and(|link| !link.host.alive()) {
            self.link = None;
            let left = self.desk.leave();
            self.tell(left);
        }
        if self.link.is_none() && Instant::now() >= self.retry_at {
            self.link = connect(&self.plugins, &self.sender);
            self.retry_at = Instant::now() + RETRY;
        }
        if self.desk.process != 0 && !running(self.desk.process) {
            let left = self.desk.leave();
            self.tell(left);
        }
    }

    fn run(mut self, events: mpsc::Receiver<Event>) {
        self.watch();
        let mut watched = Instant::now();
        loop {
            match events.recv_timeout(TICK) {
                Ok(Event::Bus(index, message)) => self.handle(index, message),
                Ok(Event::Choice(choice)) => self.desk.choice = choice,
                Ok(Event::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if watched.elapsed() >= TICK {
                self.watch();
                watched = Instant::now();
            }
        }
    }
}

impl Moments {
    pub fn start(
        plugins: PathBuf,
        choice: Choice,
        heard: impl Fn(Moment) + Send + 'static,
    ) -> Self {
        let (sender, events) = mpsc::channel();
        let worker = Worker {
            plugins,
            sender: sender.clone(),
            heard: Box::new(heard),
            desk: Desk::new(choice),
            link: None,
            retry_at: Instant::now(),
        };
        let worker = thread::Builder::new()
            .name("rewind-moments".to_owned())
            .spawn(move || worker.run(events))
            .ok();
        Self { sender, worker }
    }

    pub fn choose(&self, choice: Choice) {
        let _ = self.sender.send(Event::Choice(choice));
    }
}

impl Drop for Moments {
    fn drop(&mut self) {
        let _ = self.sender.send(Event::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
