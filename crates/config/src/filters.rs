use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const STYLE_SLOTS: usize = 3;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Amount(pub f32);

impl PartialEq for Amount {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Amount {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterValue {
    pub control: i32,
    pub amounts: Vec<Amount>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterLayer {
    pub filter: String,
    pub values: Vec<FilterValue>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameFilters {
    pub active: Option<usize>,
    pub styles: [Vec<FilterLayer>; STYLE_SLOTS],
}

impl GameFilters {
    pub fn current(&self) -> Option<&[FilterLayer]> {
        self.active
            .and_then(|slot| self.styles.get(slot))
            .map(Vec::as_slice)
    }

    pub fn is_blank(&self) -> bool {
        self.active.is_none() && self.styles.iter().all(Vec::is_empty)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filters {
    pub enabled: bool,
    pub games: BTreeMap<String, GameFilters>,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            enabled: true,
            games: BTreeMap::new(),
        }
    }
}

pub fn game_key(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .next()
        .unwrap_or(path)
        .to_lowercase()
}

impl Filters {
    pub fn game(&self, path: &str) -> Option<&GameFilters> {
        self.games.get(&game_key(path))
    }

    pub fn update(&mut self, path: &str, apply: impl FnOnce(&mut GameFilters)) {
        let key = game_key(path);
        if key.is_empty() {
            return;
        }
        let entry = self.games.entry(key.clone()).or_default();
        apply(entry);
        if entry.is_blank() {
            self.games.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(filter: &str, amount: f32) -> FilterLayer {
        FilterLayer {
            filter: filter.to_owned(),
            values: vec![FilterValue {
                control: 1,
                amounts: vec![Amount(amount)],
            }],
        }
    }

    #[test]
    fn a_game_is_remembered_by_its_program_name_whatever_the_folder() {
        assert_eq!(
            game_key("D:\\Games\\The Witcher 3\\bin\\Witcher3.EXE"),
            "witcher3.exe"
        );
        assert_eq!(game_key("witcher3.exe"), "witcher3.exe");
        let mut filters = Filters::default();
        filters.update("D:\\Games\\witcher3.exe", |game| {
            game.styles[0] = vec![layer("Sharpen.fx", 0.4)];
            game.active = Some(0);
        });
        let found = filters.game("C:\\Other\\WITCHER3.exe").unwrap();
        assert_eq!(found.current(), Some(&[layer("Sharpen.fx", 0.4)][..]));
    }

    #[test]
    fn a_game_left_without_styles_is_forgotten() {
        let mut filters = Filters::default();
        filters.update("game.exe", |game| game.active = Some(1));
        assert!(filters.game("game.exe").is_some());
        filters.update("game.exe", |game| game.active = None);
        assert!(filters.games.is_empty());
        filters.update("", |game| game.active = Some(0));
        assert!(filters.games.is_empty());
    }

    #[test]
    fn switching_filters_off_keeps_the_saved_styles() {
        let mut game = GameFilters {
            active: Some(2),
            ..GameFilters::default()
        };
        game.styles[2] = vec![layer("NvVignette.fx", 0.7)];
        game.active = None;
        assert_eq!(game.current(), None);
        assert!(!game.is_blank());
    }

    #[test]
    fn styles_survive_saving_and_loading() {
        let mut filters = Filters::default();
        filters.update("game.exe", |game| {
            game.styles[1] = vec![layer("Color.fx", -0.25)];
            game.active = Some(1);
        });
        let saved = serde_json::to_string(&filters).unwrap();
        let loaded: Filters = serde_json::from_str(&saved).unwrap();
        assert_eq!(loaded, filters);
        let empty: Filters = serde_json::from_str("{}").unwrap();
        assert!(empty.enabled);
    }
}
