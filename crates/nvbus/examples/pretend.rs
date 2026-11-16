#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::mpsc;

    use rewind_nvbus::ansel::{CAMERA, CONTROL, KIND};
    use rewind_nvbus::wire::{self, Value, Writer};
    use rewind_nvbus::{Address, Bus, Host};

    const FILTERS: [(&str, &str); 3] = [
        ("C:\\drv\\NvCamera\\Adjustments.fx", "Brightness / Contrast"),
        ("C:\\drv\\NvCamera\\NvVignette.fx", "Vignette"),
        ("C:\\drv\\NvCamera\\Sharpen.fx", "Sharpen"),
    ];

    struct Applied {
        filter: usize,
        strength: f32,
        depth: bool,
    }

    fn answer(field: u32, body: Writer) -> Vec<u8> {
        Writer::new()
            .message(2, Writer::new().message(field, body))
            .finish()
    }

    fn properties(applied: &Applied) -> Writer {
        let (id, name) = FILTERS[applied.filter];
        let values = Writer::new()
            .float(1, applied.strength)
            .float(2, 0.5)
            .float(3, 0.0)
            .float(4, 1.0)
            .float(5, 0.01);
        let strength = Writer::new()
            .uint(1, 1)
            .text(2, "Intensity")
            .float(3, 0.0)
            .float(4, 100.0)
            .uint(5, 1)
            .message(6, values)
            .uint(10, 0)
            .text(11, "%");
        let checked = Writer::new()
            .flag(1, applied.depth)
            .flag(2, false)
            .flag(3, false)
            .flag(4, true);
        let depth = Writer::new()
            .uint(1, 2)
            .text(2, "Use depth")
            .uint(5, 2)
            .message(7, checked)
            .uint(10, 0);
        Writer::new()
            .text(1, id)
            .text(2, name)
            .message(3, strength)
            .message(3, depth)
    }

    let integer =
        |body: &[u8], field: u32| wire::find(body, field).and_then(Value::uint).unwrap_or(0) as i32;

    let plugins = std::env::temp_dir().join("rewind-nvbus-spy");
    let _host = Host::start(plugins)?;
    let (sender, inbox) = mpsc::channel::<Vec<u8>>();
    let bus = Bus::join(Address::new(CAMERA.0, CAMERA.1), move |message| {
        if message.kind == Some(KIND) {
            let _ = sender.send(message.data);
        }
    })?;
    let control = Address::new(CONTROL.0, CONTROL.1);
    println!("pretend game is on the bus");
    let mut stack: Vec<Applied> = Vec::new();
    while let Ok(data) = inbox.recv() {
        let Some(request) = wire::find(&data, 1).and_then(Value::bytes) else {
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
                Writer::new()
                    .uint(1, 1)
                    .uint(2, u64::from(std::process::id()))
                    .text(3, "C:\\Games\\pretend.exe"),
            ),
            1 => {
                let on = integer(body, 4) == 1;
                answer(11, Writer::new().uint(1, if on { 17 } else { 1 }))
            }
            3 => answer(
                1,
                FILTERS
                    .iter()
                    .fold(Writer::new(), |list, (id, _)| list.text(1, id)),
            ),
            47 => {
                let list = stack.iter().fold(Writer::new(), |list, applied| {
                    list.text(1, FILTERS[applied.filter].0)
                });
                answer(
                    46,
                    if stack.is_empty() {
                        list.text(1, "None")
                    } else {
                        list
                    },
                )
            }
            13 => {
                let wanted = wire::find(body, 1)
                    .and_then(Value::text)
                    .unwrap_or_default();
                let slot = integer(body, 2).max(0) as usize;
                let Some(filter) = FILTERS.iter().position(|(id, _)| *id == wanted) else {
                    continue;
                };
                let applied = Applied {
                    filter,
                    strength: 0.5,
                    depth: false,
                };
                let described = properties(&applied);
                if slot < stack.len() {
                    stack[slot] = applied;
                } else {
                    stack.push(applied);
                }
                answer(
                    17,
                    Writer::new()
                        .uint(1, 1)
                        .uint(2, slot.min(stack.len() - 1) as u64)
                        .message(3, described),
                )
            }
            22 => {
                let slot = integer(body, 1).max(0) as usize;
                let Some(applied) = stack.get(slot) else {
                    continue;
                };
                answer(
                    24,
                    Writer::new()
                        .uint(1, 1)
                        .uint(2, slot as u64)
                        .message(3, properties(applied)),
                )
            }
            14 => {
                let slot = integer(body, 2).max(0) as usize;
                if let Some(applied) = stack.get_mut(slot) {
                    if integer(body, 3) == 1 {
                        applied.strength = wire::find(body, 4)
                            .and_then(Value::float)
                            .unwrap_or(applied.strength);
                    } else {
                        applied.depth = integer(body, 5) == 1;
                    }
                }
                answer(18, Writer::new().uint(1, 1))
            }
            23 => {
                let slot = integer(body, 1).max(0) as usize;
                if let Some(applied) = stack.get_mut(slot) {
                    applied.strength = 0.5;
                    applied.depth = false;
                }
                answer(25, Writer::new().uint(1, slot as u64).uint(2, 1))
            }
            45 => {
                let slot = integer(body, 1).max(0) as usize;
                if slot < stack.len() {
                    stack.remove(slot);
                }
                answer(48, Writer::new().uint(1, 1))
            }
            46 => {
                let order: Vec<usize> = wire::fields(body)
                    .filter_map(|(_, value)| value.uint())
                    .map(|index| index as usize)
                    .collect();
                if order.len() == stack.len() {
                    let mut old: Vec<Option<Applied>> = stack.drain(..).map(Some).collect();
                    stack = order
                        .into_iter()
                        .filter_map(|index| old.get_mut(index).and_then(Option::take))
                        .collect();
                }
                answer(45, Writer::new().uint(1, 1))
            }
            37 => {
                stack.clear();
                answer(38, Writer::new().uint(1, 1))
            }
            _ => continue,
        };
        println!("request {field}, {} filters applied", stack.len());
        let _ = bus.post(&control, KIND, &reply);
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {}
