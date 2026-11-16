#![cfg(windows)]

use std::sync::mpsc;
use std::time::Duration;

use rewind_nvbus::ansel::{Request, Response};
use rewind_nvbus::wire::Writer;
use rewind_nvbus::{Address, Bus, Host};

const WAIT: Duration = Duration::from_secs(10);
const ANSEL_KIND: u32 = 77;

fn member(module: &str) -> (Bus, mpsc::Receiver<rewind_nvbus::Message>) {
    let (sender, receiver) = mpsc::channel();
    let bus = Bus::join(Address::new("Rewind", module), move |message| {
        let _ = sender.send(message);
    })
    .unwrap();
    (bus, receiver)
}

#[test]
#[ignore = "needs the NVIDIA app's message bus files on this machine"]
fn two_members_talk_over_a_bus_that_carries_nothing_but_the_broadcaster() {
    let plugins = tempfile::tempdir().unwrap();
    let mut host = Host::start(plugins.path().join("plugins")).unwrap();
    assert!(host.alive());

    let (listener, heard) = member("LiveListener");
    let (speaker, _) = member("LiveSpeaker");
    let joined = heard.recv_timeout(WAIT).unwrap();
    assert!(joined.peers.contains(listener.address()));

    let reply = Writer::new()
        .message(2, Writer::new().message(19, Writer::new().flag(1, true)))
        .finish();
    let mut delivered = None;
    for _ in 0..20 {
        speaker
            .post(listener.address(), ANSEL_KIND, &reply)
            .unwrap();
        let arrived = std::iter::from_fn(|| heard.recv_timeout(Duration::from_millis(500)).ok())
            .find(|message| message.kind == Some(ANSEL_KIND));
        if arrived.is_some() {
            delivered = arrived;
            break;
        }
    }
    let delivered = delivered.expect("the message never reached the listener");
    assert_eq!(&delivered.source, speaker.address());
    assert_eq!(
        Response::decode(&delivered.data),
        Some(Response::Available(true))
    );
    assert_eq!(Response::decode(&Request::Filters.encode()), None);

    drop(speaker);
    drop(listener);
    if host.owned() {
        drop(host);
        assert!(!bus_pipe_exists());
    }
}

fn bus_pipe_exists() -> bool {
    std::thread::sleep(Duration::from_millis(500));
    std::fs::read_dir(r"\\.\pipe\").is_ok_and(|pipes| {
        pipes
            .flatten()
            .any(|pipe| pipe.file_name().to_str() == Some("NvMessageBusBroadcast"))
    })
}

mod pretend {
    use std::sync::{Arc, mpsc};
    use std::thread;

    use rewind_nvbus::ansel::{CAMERA, CONTROL, KIND};
    use rewind_nvbus::wire::{self, Writer};
    use rewind_nvbus::{Address, Bus};

    pub const FILTER: &str = "C:\\drv\\NvCamera\\NvVignette.fx";

    fn answer(field: u32, body: Writer) -> Vec<u8> {
        Writer::new()
            .message(2, Writer::new().message(field, body))
            .finish()
    }

    fn properties(strength: f32) -> Writer {
        let values = Writer::new()
            .float(1, strength)
            .float(2, 0.5)
            .float(3, 0.0)
            .float(4, 1.0)
            .float(5, 0.01);
        let control = Writer::new()
            .uint(1, 1)
            .text(2, "Intensity")
            .float(3, 0.0)
            .float(4, 100.0)
            .uint(5, 1)
            .message(6, values)
            .uint(10, 0);
        Writer::new()
            .text(1, FILTER)
            .text(2, "Vignette")
            .message(3, control)
    }

    pub fn camera() -> (
        Arc<Bus>,
        thread::JoinHandle<()>,
        mpsc::Sender<Option<Vec<u8>>>,
    ) {
        let (sender, inbox) = mpsc::channel::<Option<Vec<u8>>>();
        let heard = sender.clone();
        let bus = Bus::join(Address::new(CAMERA.0, CAMERA.1), move |message| {
            if message.kind == Some(KIND) {
                let _ = heard.send(Some(message.data));
            }
        })
        .unwrap();
        let bus = Arc::new(bus);
        let poster = bus.clone();
        let control = Address::new(CONTROL.0, CONTROL.1);
        let worker = thread::spawn(move || {
            let mut applied = false;
            let mut strength = 0.5_f32;
            while let Ok(Some(data)) = inbox.recv() {
                let Some(request) = wire::find(&data, 1).and_then(wire::Value::bytes) else {
                    continue;
                };
                let Some((field, body)) = wire::fields(request).next() else {
                    continue;
                };
                let body = body.bytes().unwrap_or_default();
                let reply = match field {
                    28 => answer(29, Writer::new()),
                    38 => answer(
                        39,
                        Writer::new().flag(1, false).flag(2, true).flag(3, false),
                    ),
                    33 => answer(
                        34,
                        Writer::new().uint(1, 1).uint(2, 0).text(3, "C:\\game.exe"),
                    ),
                    1 => {
                        let on = wire::find(body, 4).and_then(wire::Value::uint) == Some(1);
                        answer(11, Writer::new().uint(1, if on { 17 } else { 1 }))
                    }
                    3 => answer(1, Writer::new().text(1, FILTER).text(2, "Vignette")),
                    47 => answer(
                        46,
                        Writer::new().text(1, if applied { FILTER } else { "None" }),
                    ),
                    13 => {
                        applied = true;
                        answer(
                            17,
                            Writer::new()
                                .uint(1, 1)
                                .uint(2, 0)
                                .message(3, properties(strength)),
                        )
                    }
                    22 => answer(
                        24,
                        Writer::new()
                            .uint(1, 1)
                            .uint(2, 0)
                            .message(3, properties(strength)),
                    ),
                    14 => {
                        strength = wire::find(body, 4)
                            .and_then(wire::Value::float)
                            .unwrap_or(strength);
                        answer(18, Writer::new().uint(1, 1))
                    }
                    _ => continue,
                };
                let _ = poster.post(&control, KIND, &reply);
            }
            drop(poster);
        });
        (bus, worker, sender)
    }
}

#[test]
#[ignore = "needs the NVIDIA app's message bus files on this machine"]
fn the_camera_session_adds_a_filter_and_moves_its_slider_against_a_pretend_game() {
    use rewind_nvbus::Camera;
    use rewind_nvbus::session::{Command, Mode};

    fn eventually(camera: &Camera, done: impl Fn(&rewind_nvbus::session::State) -> bool) -> bool {
        for _ in 0..100 {
            if done(&camera.state()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    let plugins = tempfile::tempdir().unwrap();
    let camera = Camera::start(plugins.path().join("plugins"), || {});
    std::thread::sleep(Duration::from_secs(2));
    let (game, worker, stop) = pretend::camera();

    assert!(eventually(&camera, |state| state.present && state.features.is_some()));
    assert_eq!(camera.state().path, "C:\\game.exe");

    camera.send(Command::Open(Mode::Filters));
    assert!(eventually(&camera, |state| {
        state.mode == Mode::Filters && state.filters.len() == 1
    }));
    assert!(camera.state().layers.is_empty());

    camera.send(Command::Add(pretend::FILTER.to_owned()));
    assert!(eventually(&camera, |state| {
        state.layers.len() == 1 && state.layers[0].controls.len() == 1
    }));
    assert_eq!(camera.state().layers[0].name, "Vignette");

    camera.send(Command::Set {
        layer: 0,
        control: 1,
        component: 0,
        value: 0.9,
    });
    assert!(eventually(&camera, |state| {
        state.layers[0].controls[0].components[0].current == 0.9
    }));
    assert_eq!(camera.state().style()[0].filter, "NvVignette.fx");

    camera.send(Command::Close);
    assert!(eventually(&camera, |state| state.mode == Mode::Off));

    let _ = stop.send(None);
    let _ = worker.join();
    drop(game);
    drop(camera);
}
