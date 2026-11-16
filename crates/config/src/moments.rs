use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::filters::game_key;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MomentKind {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameMoments {
    pub enabled: bool,
    pub name: String,
    pub kinds: Vec<MomentKind>,
}

impl Default for GameMoments {
    fn default() -> Self {
        Self {
            enabled: true,
            name: String::new(),
            kinds: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Highlights {
    pub enabled: bool,
    pub games: BTreeMap<String, GameMoments>,
}

impl Default for Highlights {
    fn default() -> Self {
        Self {
            enabled: true,
            games: BTreeMap::new(),
        }
    }
}

impl Highlights {
    pub fn game(&self, path: &str) -> Option<&GameMoments> {
        self.games.get(&game_key(path))
    }

    pub fn meet(&mut self, path: &str, name: &str) -> bool {
        let key = game_key(path);
        if key.is_empty() {
            return false;
        }
        let known = self.games.contains_key(&key);
        let game = self.games.entry(key).or_default();
        let renamed = !name.is_empty() && game.name != name;
        if renamed {
            name.clone_into(&mut game.name);
        }
        renamed || !known
    }

    pub fn configure(&mut self, path: &str, kinds: &[(String, String, bool)]) -> bool {
        let key = game_key(path);
        if key.is_empty() {
            return false;
        }
        let game = self.games.entry(key).or_default();
        let known = std::mem::take(&mut game.kinds);
        game.kinds = kinds
            .iter()
            .map(|(id, name, interest)| MomentKind {
                id: id.clone(),
                name: name.clone(),
                enabled: known
                    .iter()
                    .find(|kind| kind.id == *id)
                    .map_or(*interest, |kind| kind.enabled),
            })
            .collect();
        game.kinds != known
    }

    pub fn allows(&self, path: &str) -> bool {
        self.enabled && self.game(path).is_none_or(|game| game.enabled)
    }

    pub fn toggle_game(&mut self, path: &str) {
        if let Some(game) = self.games.get_mut(&game_key(path)) {
            game.enabled ^= true;
        }
    }

    pub fn toggle(&mut self, path: &str, id: &str) {
        if let Some(game) = self.games.get_mut(&game_key(path)) {
            for kind in &mut game.kinds {
                if kind.id == id {
                    kind.enabled ^= true;
                }
            }
        }
    }

    pub fn muted(&self, path: &str) -> BTreeSet<String> {
        self.game(path)
            .map(|game| {
                game.kinds
                    .iter()
                    .filter(|kind| !kind.enabled)
                    .map(|kind| kind.id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds() -> Vec<(String, String, bool)> {
        vec![
            ("kill".to_owned(), "Убийство".to_owned(), true),
            ("death".to_owned(), "Смерть".to_owned(), false),
        ]
    }

    #[test]
    fn a_game_starts_with_the_kinds_it_marks_as_interesting() {
        let mut highlights = Highlights::default();
        highlights.meet("D:\\Games\\Shooter.exe", "Shooter");
        highlights.configure("D:\\Games\\Shooter.exe", &kinds());
        let game = highlights.game("shooter.exe").unwrap();
        assert_eq!(game.name, "Shooter");
        assert_eq!(
            highlights.muted("shooter.exe"),
            BTreeSet::from(["death".to_owned()])
        );
    }

    #[test]
    fn the_users_choice_survives_the_game_describing_itself_again() {
        let mut highlights = Highlights::default();
        highlights.configure("game.exe", &kinds());
        highlights.toggle("game.exe", "death");
        highlights.toggle("game.exe", "kill");
        let mut again = kinds();
        again.push(("win".to_owned(), "Победа".to_owned(), true));
        highlights.configure("game.exe", &again);
        assert_eq!(
            highlights.muted("game.exe"),
            BTreeSet::from(["kill".to_owned()])
        );
        assert_eq!(highlights.game("game.exe").unwrap().kinds.len(), 3);
    }

    #[test]
    fn a_game_can_be_switched_off_without_losing_its_kinds() {
        let mut highlights = Highlights::default();
        assert!(highlights.allows("new.exe"));
        highlights.configure("game.exe", &kinds());
        highlights.toggle_game("game.exe");
        assert!(!highlights.allows("GAME.exe"));
        assert_eq!(highlights.game("game.exe").unwrap().kinds.len(), 2);
        highlights.toggle_game("game.exe");
        highlights.enabled = false;
        assert!(!highlights.allows("game.exe"));
    }

    #[test]
    fn an_unknown_game_mutes_nothing_and_a_nameless_one_is_not_stored() {
        let mut highlights = Highlights::default();
        assert!(highlights.muted("other.exe").is_empty());
        highlights.meet("", "Nameless");
        highlights.configure("", &kinds());
        assert!(highlights.games.is_empty());
        let loaded: Highlights = serde_json::from_str("{}").unwrap();
        assert!(loaded.enabled);
    }

    #[test]
    fn a_game_seen_again_with_the_same_kinds_changes_nothing() {
        let mut highlights = Highlights::default();
        assert!(highlights.meet("game.exe", "Game"));
        assert!(highlights.configure("game.exe", &kinds()));
        assert!(!highlights.meet("game.exe", "Game"));
        assert!(!highlights.configure("game.exe", &kinds()));
        assert!(highlights.meet("game.exe", "Game 2"));
    }
}
