use std::collections::{BTreeMap, BTreeSet};

use crate::highlights::{Call, Definition, Envelope, Reply};

pub const LOCALE: &str = "ru-RU";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Moment {
    Joined {
        game: String,
        process: u32,
        path: String,
    },
    Left,
    Configured(Vec<Definition>),
    Video {
        group: String,
        highlight: String,
        start: i32,
        end: i32,
    },
    Screenshot {
        group: String,
        highlight: String,
    },
    Summary,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub enabled: bool,
    pub muted: BTreeSet<String>,
}

#[derive(Debug, Default)]
pub struct Desk {
    pub choice: Choice,
    pub process: u32,
    definitions: Vec<Definition>,
    counts: BTreeMap<String, i32>,
}

impl Desk {
    pub fn new(choice: Choice) -> Self {
        Self {
            choice,
            ..Self::default()
        }
    }

    fn wanted(&self, highlight: &str) -> bool {
        self.choice.enabled && !self.choice.muted.contains(highlight)
    }

    fn counted(&mut self, group: &str) {
        *self.counts.entry(group.to_owned()).or_default() += 1;
    }

    pub fn leave(&mut self) -> Vec<Moment> {
        if self.process == 0 {
            return Vec::new();
        }
        self.process = 0;
        self.definitions.clear();
        self.counts.clear();
        vec![Moment::Left]
    }

    pub fn handle(&mut self, envelope: Envelope) -> (Reply, Vec<Moment>) {
        match envelope.call {
            Call::Create {
                game,
                process,
                path,
                scopes,
            } => {
                self.process = process as u32;
                self.counts.clear();
                let reply = Reply::Created {
                    major: envelope.major,
                    minor: envelope.minor,
                    scopes,
                };
                let joined = Moment::Joined {
                    game,
                    process: self.process,
                    path,
                };
                (reply, vec![joined])
            }
            Call::Destroy => (Reply::Done, self.leave()),
            Call::Alive => (Reply::Alive, Vec::new()),
            Call::Language => (Reply::Language(LOCALE.to_owned()), Vec::new()),
            Call::OpenGroup { group, .. } => {
                self.counts.entry(group).or_default();
                (Reply::Done, Vec::new())
            }
            Call::CloseGroup { group, .. } => {
                self.counts.remove(&group);
                (Reply::Done, Vec::new())
            }
            Call::Count { group } => {
                let count = self.counts.get(&group).copied().unwrap_or(0);
                (Reply::Count(count), Vec::new())
            }
            Call::Configure { definitions } => {
                self.definitions.clone_from(&definitions);
                (Reply::Done, vec![Moment::Configured(definitions)])
            }
            Call::UserConfig => {
                let highlights = self
                    .definitions
                    .iter()
                    .map(|kind| (kind.id.clone(), !self.choice.muted.contains(&kind.id)))
                    .collect();
                let reply = Reply::UserConfig {
                    enabled: self.choice.enabled,
                    highlights,
                };
                (reply, Vec::new())
            }
            Call::Video {
                group,
                highlight,
                start,
                end,
            } => {
                if !self.wanted(&highlight) {
                    return (Reply::Done, Vec::new());
                }
                self.counted(&group);
                let moment = Moment::Video {
                    group,
                    highlight,
                    start,
                    end,
                };
                (Reply::Done, vec![moment])
            }
            Call::Screenshot { group, highlight } => {
                if !self.wanted(&highlight) {
                    return (Reply::Done, Vec::new());
                }
                self.counted(&group);
                (Reply::Done, vec![Moment::Screenshot { group, highlight }])
            }
            Call::Summary { .. } | Call::ToggleOverlay => (Reply::Done, vec![Moment::Summary]),
            Call::Permissions { .. } | Call::Other(_) => (Reply::Done, Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(call: Call) -> Envelope {
        Envelope {
            major: 1,
            minor: 5,
            call,
        }
    }

    fn video(highlight: &str) -> Call {
        Call::Video {
            group: "match".to_owned(),
            highlight: highlight.to_owned(),
            start: -5000,
            end: 1000,
        }
    }

    fn joined() -> Desk {
        let mut desk = Desk {
            choice: Choice {
                enabled: true,
                muted: BTreeSet::new(),
            },
            ..Desk::default()
        };
        desk.handle(call(Call::Create {
            game: "Game".to_owned(),
            process: 77,
            path: "C:\\game.exe".to_owned(),
            scopes: vec![0, 1],
        }));
        desk
    }

    #[test]
    fn a_game_gets_the_version_it_speaks_and_every_scope_it_asked_for() {
        let mut desk = Desk::default();
        let (reply, moments) = desk.handle(call(Call::Create {
            game: "Game".to_owned(),
            process: 77,
            path: "C:\\game.exe".to_owned(),
            scopes: vec![0, 1, 2],
        }));
        assert_eq!(
            reply,
            Reply::Created {
                major: 1,
                minor: 5,
                scopes: vec![0, 1, 2]
            }
        );
        assert_eq!(
            moments,
            [Moment::Joined {
                game: "Game".to_owned(),
                process: 77,
                path: "C:\\game.exe".to_owned()
            }]
        );
    }

    #[test]
    fn moments_are_counted_per_group_and_forgotten_with_it() {
        let mut desk = joined();
        desk.handle(call(Call::OpenGroup {
            group: "match".to_owned(),
            name: String::new(),
        }));
        assert_eq!(desk.handle(call(video("kill"))).1.len(), 1);
        desk.handle(call(Call::Screenshot {
            group: "match".to_owned(),
            highlight: "win".to_owned(),
        }));
        let count = |desk: &mut Desk| {
            desk.handle(call(Call::Count {
                group: "match".to_owned(),
            }))
            .0
        };
        assert_eq!(count(&mut desk), Reply::Count(2));
        desk.handle(call(Call::CloseGroup {
            group: "match".to_owned(),
            destroy: false,
        }));
        assert_eq!(count(&mut desk), Reply::Count(0));
    }

    #[test]
    fn a_kind_the_user_switched_off_is_acknowledged_but_not_saved() {
        let mut desk = joined();
        desk.choice.muted.insert("death".to_owned());
        desk.handle(call(Call::Configure {
            definitions: vec![
                Definition {
                    id: "kill".to_owned(),
                    name: "Kill".to_owned(),
                    interest: true,
                },
                Definition {
                    id: "death".to_owned(),
                    name: "Death".to_owned(),
                    interest: true,
                },
            ],
        }));
        assert_eq!(desk.handle(call(video("death"))), (Reply::Done, Vec::new()));
        assert_eq!(
            desk.handle(call(Call::UserConfig)).0,
            Reply::UserConfig {
                enabled: true,
                highlights: vec![("kill".to_owned(), true), ("death".to_owned(), false)],
            }
        );
        desk.choice.enabled = false;
        assert!(desk.handle(call(video("kill"))).1.is_empty());
    }

    #[test]
    fn a_game_that_leaves_is_reported_once() {
        let mut desk = joined();
        assert_eq!(desk.handle(call(Call::Destroy)).1, [Moment::Left]);
        assert!(desk.leave().is_empty());
    }
}
