use crate::wire::{self, Value, Writer};

pub const SERVERS: [(&str, &str); 4] = [
    ("GfeSDK", "NvNodeSDKAPI"),
    ("NvNodeSDKAPI", "GfeSDK"),
    ("GfeSDKCore", "NvNodeSDKAPI"),
    ("NvNodeSDKAPI", "GfeSDKCore"),
];
pub const SUCCESS: i32 = 0;
pub const GRANTED: u32 = 0;
pub const INSTALLED_VERSION: &str = "3.28.0.417";

const REQUEST: u32 = 1;
const RESPONSE: u32 = 2;
const PREFERRED_LOCALES: [&str; 3] = ["ru-RU", "ru", "en-US"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub id: String,
    pub name: String,
    pub interest: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    Create {
        game: String,
        process: u64,
        path: String,
        scopes: Vec<u32>,
    },
    Destroy,
    Alive,
    Language,
    OpenGroup {
        group: String,
        name: String,
    },
    CloseGroup {
        group: String,
        destroy: bool,
    },
    Count {
        group: String,
    },
    Screenshot {
        group: String,
        highlight: String,
    },
    Video {
        group: String,
        highlight: String,
        start: i32,
        end: i32,
    },
    Summary {
        groups: Vec<String>,
    },
    ToggleOverlay,
    Configure {
        definitions: Vec<Definition>,
    },
    UserConfig,
    Permissions {
        scopes: Vec<u32>,
    },
    Other(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub major: i32,
    pub minor: i32,
    pub call: Call,
}

fn text(body: &[u8], field: u32) -> String {
    wire::find(body, field)
        .and_then(Value::text)
        .unwrap_or_default()
        .to_owned()
}

fn number(body: &[u8], field: u32) -> u64 {
    wire::find(body, field).and_then(Value::uint).unwrap_or(0)
}

fn numbers(body: &[u8], field: u32) -> Vec<u32> {
    wire::fields(body)
        .filter(|(number, _)| *number == field)
        .flat_map(|(_, value)| match value {
            Value::Bytes(packed) => wire::fields_packed(packed),
            other => vec![other.uint().unwrap_or(0)],
        })
        .map(|value| value as u32)
        .collect()
}

fn messages(body: &[u8], field: u32) -> impl Iterator<Item = &[u8]> {
    wire::fields(body)
        .filter(move |(number, _)| *number == field)
        .filter_map(|(_, value)| value.bytes())
}

fn localized(body: &[u8], field: u32) -> String {
    let pairs: Vec<(String, String)> = messages(body, field)
        .map(|pair| (text(pair, 1), text(pair, 2)))
        .collect();
    PREFERRED_LOCALES
        .iter()
        .find_map(|locale| {
            pairs
                .iter()
                .find(|(code, _)| code.eq_ignore_ascii_case(locale))
        })
        .or(pairs.first())
        .map(|(_, name)| name.clone())
        .unwrap_or_default()
}

pub fn decode(message: &[u8]) -> Option<Envelope> {
    let request = wire::find(message, REQUEST)?.bytes()?;
    let (field, body) = wire::fields(request)
        .filter(|(field, _)| *field >= 10)
        .find_map(|(field, value)| value.bytes().map(|body| (field, body)))?;
    let call = match field {
        10 => Call::Language,
        11 => Call::OpenGroup {
            group: text(body, 1),
            name: localized(body, 2),
        },
        12 => Call::CloseGroup {
            group: text(body, 1),
            destroy: number(body, 2) != 0,
        },
        13 => Call::Screenshot {
            group: text(body, 2),
            highlight: text(body, 3),
        },
        14 => Call::Video {
            group: text(body, 2),
            highlight: text(body, 3),
            start: number(body, 4) as i32,
            end: number(body, 5) as i32,
        },
        15 => Call::ToggleOverlay,
        16 => Call::Configure {
            definitions: messages(body, 3)
                .map(|entry| {
                    let id = text(entry, 1);
                    let name = localized(entry, 7);
                    Definition {
                        name: if name.is_empty() { id.clone() } else { name },
                        interest: wire::find(entry, 3).is_none() || number(entry, 3) != 0,
                        id,
                    }
                })
                .collect(),
        },
        17 => Call::Summary {
            groups: messages(body, 2).map(|entry| text(entry, 1)).collect(),
        },
        19 => Call::Create {
            game: text(body, 4),
            process: number(body, 2),
            path: text(body, 7),
            scopes: numbers(body, 6),
        },
        20 => Call::Destroy,
        21 => Call::Alive,
        22 => Call::Count {
            group: text(body, 1),
        },
        23 => Call::UserConfig,
        24 => Call::Permissions {
            scopes: numbers(body, 1),
        },
        other => Call::Other(other),
    };
    Some(Envelope {
        major: number(request, 1) as i32,
        minor: number(request, 2) as i32,
        call,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Done,
    Created {
        major: i32,
        minor: i32,
        scopes: Vec<u32>,
    },
    Language(String),
    Alive,
    Count(i32),
    UserConfig {
        enabled: bool,
        highlights: Vec<(String, bool)>,
    },
}

impl Reply {
    pub fn encode(&self) -> Vec<u8> {
        let response = Writer::new().uint(1, SUCCESS as u64);
        let response = match self {
            Self::Done => response,
            Self::Created {
                major,
                minor,
                scopes,
            } => {
                let created = Writer::new()
                    .text(1, INSTALLED_VERSION)
                    .uint(2, *major as u64)
                    .uint(3, *minor as u64);
                let created = scopes.iter().fold(created, |created, scope| {
                    let granted = Writer::new()
                        .uint(1, u64::from(*scope))
                        .uint(2, u64::from(GRANTED));
                    created.message(4, granted)
                });
                response.message(11, created)
            }
            Self::Language(locale) => response.message(10, Writer::new().text(1, locale)),
            Self::Alive => response.message(12, Writer::new().flag(1, true)),
            Self::Count(count) => {
                response.message(13, Writer::new().uint(1, i64::from(*count) as u64))
            }
            Self::UserConfig {
                enabled,
                highlights,
            } => {
                let config = Writer::new().flag(1, *enabled);
                let config = highlights.iter().fold(config, |config, (id, on)| {
                    config.message(2, Writer::new().text(1, id).flag(2, *on))
                });
                response.message(14, config)
            }
        };
        Writer::new().message(RESPONSE, response).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(field: u32, body: Writer) -> Vec<u8> {
        let request = Writer::new()
            .uint(1, 1)
            .uint(2, 5)
            .uint(5, 1)
            .message(field, body);
        Writer::new().message(REQUEST, request).finish()
    }

    fn pair(locale: &str, name: &str) -> Writer {
        Writer::new().text(1, locale).text(2, name)
    }

    #[test]
    fn a_game_introduces_itself_with_its_name_process_and_wishes() {
        let body = Writer::new()
            .text(1, "client")
            .uint(2, 4242)
            .text(4, "Fortnite")
            .uint(6, 0)
            .uint(6, 1)
            .text(7, "C:\\Games\\Fortnite.exe");
        let envelope = decode(&request(19, body)).unwrap();
        assert_eq!((envelope.major, envelope.minor), (1, 5));
        assert_eq!(
            envelope.call,
            Call::Create {
                game: "Fortnite".to_owned(),
                process: 4242,
                path: "C:\\Games\\Fortnite.exe".to_owned(),
                scopes: vec![0, 1],
            }
        );
    }

    #[test]
    fn a_video_moment_reaches_back_before_now_and_may_run_past_it() {
        let body = Writer::new()
            .text(2, "match")
            .text(3, "kill")
            .uint(4, i64::from(-5000_i32) as u64)
            .uint(5, 2000);
        assert_eq!(
            decode(&request(14, body)).unwrap().call,
            Call::Video {
                group: "match".to_owned(),
                highlight: "kill".to_owned(),
                start: -5000,
                end: 2000,
            }
        );
    }

    #[test]
    fn moment_kinds_are_named_in_russian_when_the_game_offers_it() {
        let named = Writer::new()
            .text(1, "kill")
            .flag(3, false)
            .message(7, pair("en-US", "Kill"))
            .message(7, pair("ru-RU", "Убийство"));
        let plain = Writer::new()
            .text(1, "win")
            .message(7, pair("de-DE", "Sieg"));
        let bare = Writer::new().text(1, "death");
        let body = Writer::new()
            .message(3, named)
            .message(3, plain)
            .message(3, bare);
        assert_eq!(
            decode(&request(16, body)).unwrap().call,
            Call::Configure {
                definitions: vec![
                    Definition {
                        id: "kill".to_owned(),
                        name: "Убийство".to_owned(),
                        interest: false,
                    },
                    Definition {
                        id: "win".to_owned(),
                        name: "Sieg".to_owned(),
                        interest: true,
                    },
                    Definition {
                        id: "death".to_owned(),
                        name: "death".to_owned(),
                        interest: true,
                    },
                ],
            }
        );
    }

    #[test]
    fn the_first_answer_grants_every_scope_the_game_asked_for() {
        let encoded = Reply::Created {
            major: 1,
            minor: 5,
            scopes: vec![0, 2],
        }
        .encode();
        let response = wire::find(&encoded, RESPONSE).unwrap().bytes().unwrap();
        assert_eq!(number(response, 1), 0);
        let created = wire::find(response, 11).unwrap().bytes().unwrap();
        assert_eq!((number(created, 2), number(created, 3)), (1, 5));
        let granted: Vec<(u64, u64)> = messages(created, 4)
            .map(|scope| (number(scope, 1), number(scope, 2)))
            .collect();
        assert_eq!(granted, [(0, 0), (2, 0)]);
    }

    #[test]
    fn the_user_choice_of_moment_kinds_is_reported_back() {
        let encoded = Reply::UserConfig {
            enabled: true,
            highlights: vec![("kill".to_owned(), true), ("death".to_owned(), false)],
        }
        .encode();
        let response = wire::find(&encoded, RESPONSE).unwrap().bytes().unwrap();
        let config = wire::find(response, 14).unwrap().bytes().unwrap();
        assert_eq!(number(config, 1), 1);
        let kinds: Vec<(String, u64)> = messages(config, 2)
            .map(|kind| (text(kind, 1), number(kind, 2)))
            .collect();
        assert_eq!(kinds, [("kill".to_owned(), 1), ("death".to_owned(), 0)]);
    }

    #[test]
    fn anything_that_is_not_a_request_is_ignored() {
        assert_eq!(decode(&Reply::Done.encode()), None);
        assert_eq!(
            decode(&request(25, Writer::new())).unwrap().call,
            Call::Other(25)
        );
    }
}
