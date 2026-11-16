#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use rewind_nvbus::ansel::Response;
    use rewind_nvbus::{Address, Bus, Host};

    const NAMES: [(&str, &str); 6] = [
        ("Rewind", "Spy"),
        ("Ansel", "NvCameraControl"),
        ("NvCamera", "NvCameraControl"),
        ("CrimsonNative", "NvCameraControl"),
        ("CrimsonNative", "ShareServer"),
        ("Ansel", "ShareServer"),
    ];
    const PREVIEW: usize = 48;

    let plugins = std::env::temp_dir().join("rewind-nvbus-spy");
    let mut host = Host::start(plugins)?;
    println!("bus up, started here: {}", host.owned());
    let known = Arc::new(Mutex::new(BTreeSet::new()));
    let mut members = Vec::new();
    for (system, module) in NAMES {
        let known = known.clone();
        let me = format!("{system}/{module}");
        members.push(Bus::join(Address::new(system, module), move |message| {
            let mut known = known.lock().unwrap();
            for peer in &message.peers {
                let name = format!("{}/{}", peer.system, peer.module);
                if known.insert(name.clone()) {
                    println!("peer {name}");
                }
            }
            if let Some(kind) = message.kind {
                let preview: String = message
                    .data
                    .iter()
                    .take(PREVIEW)
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                println!(
                    "to {me} from {}/{} pid {} kind {kind} len {} {:?} {preview}",
                    message.source.system,
                    message.source.module,
                    message.process,
                    message.data.len(),
                    Response::decode(&message.data),
                );
            }
        })?);
    }
    loop {
        std::thread::sleep(Duration::from_secs(5));
        if !host.alive() {
            println!("bus went away");
            return Ok(());
        }
    }
}

#[cfg(not(windows))]
fn main() {}
