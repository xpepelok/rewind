#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::mpsc;
    use std::time::Duration;

    use rewind_nvbus::highlights::SERVERS;
    use rewind_nvbus::wire::{self, Value, Writer};
    use rewind_nvbus::{Address, Bus, Host};

    const KIND: u32 = 0;
    const WAIT: Duration = Duration::from_secs(4);

    fn request(field: u32, body: Writer) -> Vec<u8> {
        let request = Writer::new()
            .uint(1, 1)
            .uint(2, 5)
            .uint(5, 1)
            .message(field, body);
        Writer::new().message(1, request).finish()
    }

    fn kind(id: &str, name: &str, interest: bool) -> Writer {
        Writer::new()
            .text(1, id)
            .flag(3, interest)
            .message(7, Writer::new().text(1, "ru-RU").text(2, name))
    }

    let plugins = std::env::temp_dir().join("rewind-nvbus-spy");
    let _host = Host::start(plugins)?;
    let (sender, inbox) = mpsc::channel::<Vec<u8>>();
    let bus = Bus::join(Address::new("GfeSDKCore", "PretendGame"), move |message| {
        if message.kind.is_some() {
            let _ = sender.send(message.data);
        }
    })?;
    std::thread::sleep(Duration::from_secs(2));
    let server = Address::new(SERVERS[0].0, SERVERS[0].1);
    let calls = [
        (
            "create",
            request(
                19,
                Writer::new()
                    .text(1, "pretend")
                    .uint(2, u64::from(std::process::id()))
                    .text(4, "Pretend Shooter")
                    .uint(6, 0)
                    .uint(6, 1)
                    .text(7, "C:\\Games\\pretend-shooter.exe"),
            ),
        ),
        (
            "configure",
            request(
                16,
                Writer::new()
                    .message(3, kind("kill", "Убийство", true))
                    .message(3, kind("death", "Смерть", false)),
            ),
        ),
        ("user config", request(23, Writer::new())),
        ("open group", request(11, Writer::new().text(1, "match"))),
        (
            "video",
            request(
                14,
                Writer::new()
                    .text(2, "match")
                    .text(3, "kill")
                    .uint(4, i64::from(-6000_i32) as u64)
                    .uint(5, 1000),
            ),
        ),
        ("count", request(22, Writer::new().text(1, "match"))),
    ];
    for (name, data) in calls {
        bus.post(&server, KIND, &data)?;
        match inbox.recv_timeout(WAIT) {
            Ok(reply) => {
                let response = wire::find(&reply, 2)
                    .and_then(Value::bytes)
                    .unwrap_or_default();
                let fields: Vec<u32> = wire::fields(response).map(|(field, _)| field).collect();
                let code = wire::find(response, 1).and_then(Value::uint);
                println!(
                    "{name}: retval {code:?}, fields {fields:?}, {} bytes",
                    reply.len()
                );
            }
            Err(_) => println!("{name}: no reply"),
        }
    }
    std::thread::sleep(Duration::from_secs(8));
    let summary = request(17, Writer::new().message(2, Writer::new().text(1, "match")));
    bus.post(&server, KIND, &summary)?;
    println!("summary asked: {}", inbox.recv_timeout(WAIT).is_ok());
    std::thread::sleep(Duration::from_secs(2));
    Ok(())
}

#[cfg(not(windows))]
fn main() {}
