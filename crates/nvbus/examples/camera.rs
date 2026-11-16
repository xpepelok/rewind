#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use rewind_nvbus::ansel::{CAMERA, CONTROL, Incoming, KIND, Request, Response};
    use rewind_nvbus::{Address, Bus, Host};

    const SETTLE: Duration = Duration::from_secs(4);
    const REPLY_WAIT: Duration = Duration::from_secs(3);
    const LIST_WAIT: Duration = Duration::from_secs(8);
    const PREVIEW: usize = 40;

    let plugins = std::env::temp_dir().join("rewind-nvbus-spy");
    let host = Host::start(plugins)?;
    println!("bus up, started here: {}", host.owned());

    let camera = Address::new(CAMERA.0, CAMERA.1);
    let (sender, inbox) = mpsc::channel();
    let bus = Bus::join(Address::new(CONTROL.0, CONTROL.1), move |message| {
        let _ = sender.send(message);
    })?;
    println!(
        "joined as {}/{}",
        bus.address().system,
        bus.address().module
    );

    let listen = |wait: Duration| {
        let until = Instant::now() + wait;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            let Ok(message) = inbox.recv_timeout(left) else {
                break;
            };
            if message.source == camera {
                println!("camera is on the bus, game pid {}", message.process);
            }
            if message.kind.is_none() {
                continue;
            }
            let preview: String = message
                .data
                .iter()
                .take(PREVIEW)
                .map(|byte| format!("{byte:02x}"))
                .collect();
            if let Some(incoming) = Incoming::decode(&message.data) {
                println!("  camera asks {incoming:?}");
                if let Some(reply) = incoming.reply() {
                    let sent = bus.post(&message.source, KIND, &reply);
                    println!("  answered: {sent:?}");
                }
            } else if let Some(response) = Response::decode(&message.data) {
                println!("  camera replies {response:?}");
            } else {
                println!(
                    "  unknown from {}/{} len {} {preview}",
                    message.source.system,
                    message.source.module,
                    message.data.len()
                );
            }
        }
    };

    let session = |on: bool| Request::Enable {
        on,
        keep_filters: true,
        pause: false,
    };
    listen(SETTLE);
    for (request, wait) in [
        (Request::UiReady, REPLY_WAIT),
        (Request::Features, REPLY_WAIT),
        (Request::Filters, LIST_WAIT),
        (session(true), LIST_WAIT),
        (Request::Filters, LIST_WAIT),
        (Request::Stack, REPLY_WAIT),
        (session(false), LIST_WAIT),
    ] {
        println!("ask {request:?}");
        if let Err(error) = bus.post(&camera, KIND, &request.encode()) {
            println!("  not sent: {error}");
        }
        listen(wait);
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {}
