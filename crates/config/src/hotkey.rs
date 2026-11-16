use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const VK_BACK: u16 = 0x08;
const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0D;
const VK_PAUSE: u16 = 0x13;
const VK_SPACE: u16 = 0x20;
const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_END: u16 = 0x23;
const VK_HOME: u16 = 0x24;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_SNAPSHOT: u16 = 0x2C;
const VK_SCROLL: u16 = 0x91;
const VK_INSERT: u16 = 0x2D;
const VK_DELETE: u16 = 0x2E;
const VK_NUMPAD0: u16 = 0x60;
const VK_F1: u16 = 0x70;
const VK_0: u16 = 0x30;
const VK_F24: u16 = 0x87;
const VK_OEM_1: u16 = 0xBA;
const VK_OEM_PLUS: u16 = 0xBB;
const VK_OEM_COMMA: u16 = 0xBC;
const VK_OEM_MINUS: u16 = 0xBD;
const VK_OEM_PERIOD: u16 = 0xBE;
const VK_OEM_2: u16 = 0xBF;
const VK_OEM_3: u16 = 0xC0;
const VK_OEM_4: u16 = 0xDB;
const VK_OEM_5: u16 = 0xDC;
const VK_OEM_6: u16 = 0xDD;
const VK_OEM_7: u16 = 0xDE;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
    };
    pub const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };
    pub const CTRL: Self = Self {
        ctrl: true,
        ..Self::NONE
    };
    pub const ALT_SHIFT: Self = Self {
        alt: true,
        shift: true,
        ..Self::NONE
    };
    pub const CTRL_ALT: Self = Self {
        ctrl: true,
        alt: true,
        ..Self::NONE
    };

    pub const fn is_empty(self) -> bool {
        !(self.ctrl || self.alt || self.shift || self.win)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key: u16,
}

impl Hotkey {
    pub const fn new(modifiers: Modifiers, key: u16) -> Self {
        Self { modifiers, key }
    }

    pub fn label(&self) -> String {
        let parts = [
            (self.modifiers.ctrl, "Ctrl"),
            (self.modifiers.alt, "Alt"),
            (self.modifiers.shift, "Shift"),
            (self.modifiers.win, "Win"),
        ];
        let mut label = String::with_capacity(24);
        for (_, name) in parts.iter().filter(|(on, _)| *on) {
            label.push_str(name);
            label.push('+');
        }
        label.push_str(&key_name(self.key));
        label
    }
}

pub fn key_name(key: u16) -> String {
    match key {
        VK_F1..=VK_F24 => format!("F{}", key - VK_F1 + 1),
        0x30..=0x39 | 0x41..=0x5A => char::from(key as u8).to_string(),
        VK_NUMPAD0..=0x69 => format!("Num {}", key - VK_NUMPAD0),
        _ => named_key(key).map_or_else(|| format!("0x{key:02X}"), str::to_owned),
    }
}

fn named_key(key: u16) -> Option<&'static str> {
    Some(match key {
        VK_BACK => "Backspace",
        VK_TAB => "Tab",
        VK_RETURN => "Enter",
        VK_PAUSE => "Pause",
        VK_SPACE => "Space",
        VK_PRIOR => "PgUp",
        VK_NEXT => "PgDn",
        VK_END => "End",
        VK_HOME => "Home",
        VK_LEFT => "Left",
        VK_UP => "Up",
        VK_RIGHT => "Right",
        VK_DOWN => "Down",
        VK_SNAPSHOT => "PrtSc",
        VK_SCROLL => "ScrLk",
        VK_INSERT => "Insert",
        VK_DELETE => "Delete",
        VK_OEM_1 => ";",
        VK_OEM_PLUS => "=",
        VK_OEM_COMMA => ",",
        VK_OEM_MINUS => "-",
        VK_OEM_PERIOD => ".",
        VK_OEM_2 => "/",
        VK_OEM_3 => "`",
        VK_OEM_4 => "[",
        VK_OEM_5 => "\\",
        VK_OEM_6 => "]",
        VK_OEM_7 => "'",
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Command {
    ToggleOverlay,
    ToggleMic,
    PushToTalk,
    ToggleStats,
    ToggleRecording,
    ToggleReplay,
    SaveReplay,
    Screenshot,
    PublishScreenshot,
    Mark,
    CopyLastClip,
    UploadReplay,
    PhotoMode,
    Filters,
    StyleOff,
    Style1,
    Style2,
    Style3,
}

impl Command {
    pub const ALL: [Self; 18] = [
        Self::ToggleOverlay,
        Self::ToggleMic,
        Self::PushToTalk,
        Self::ToggleStats,
        Self::ToggleRecording,
        Self::ToggleReplay,
        Self::SaveReplay,
        Self::Screenshot,
        Self::PublishScreenshot,
        Self::Mark,
        Self::CopyLastClip,
        Self::UploadReplay,
        Self::PhotoMode,
        Self::Filters,
        Self::StyleOff,
        Self::Style1,
        Self::Style2,
        Self::Style3,
    ];

    pub const fn default_hotkey(self) -> Hotkey {
        match self {
            Self::ToggleOverlay => Hotkey::new(Modifiers::ALT, 0x5A),
            Self::ToggleMic => Hotkey::new(Modifiers::CTRL_ALT, 0x4D),
            Self::PushToTalk => Hotkey::new(Modifiers::NONE, VK_OEM_3),
            Self::ToggleStats => Hotkey::new(Modifiers::ALT, 0x52),
            Self::ToggleRecording => Hotkey::new(Modifiers::CTRL, VK_F1 + 11),
            Self::ToggleReplay => Hotkey::new(Modifiers::ALT_SHIFT, VK_F1 + 9),
            Self::SaveReplay => Hotkey::new(Modifiers::ALT, VK_F1 + 9),
            Self::Screenshot => Hotkey::new(Modifiers::NONE, VK_SNAPSHOT),
            Self::PublishScreenshot => Hotkey::new(Modifiers::ALT_SHIFT, VK_F1),
            Self::Mark => Hotkey::new(Modifiers::ALT, VK_F1 + 7),
            Self::CopyLastClip => Hotkey::new(Modifiers::ALT, VK_F1 + 10),
            Self::UploadReplay => Hotkey::new(Modifiers::ALT, VK_F1 + 5),
            Self::PhotoMode => Hotkey::new(Modifiers::ALT, VK_F1 + 1),
            Self::Filters => Hotkey::new(Modifiers::ALT, VK_F1 + 2),
            Self::StyleOff => Hotkey::new(Modifiers::CTRL_ALT, VK_0),
            Self::Style1 => Hotkey::new(Modifiers::CTRL_ALT, VK_0 + 1),
            Self::Style2 => Hotkey::new(Modifiers::CTRL_ALT, VK_0 + 2),
            Self::Style3 => Hotkey::new(Modifiers::CTRL_ALT, VK_0 + 3),
        }
    }

    pub const fn is_global(self) -> bool {
        !matches!(self, Self::PushToTalk)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bindings(BTreeMap<Command, Option<Hotkey>>);

impl Bindings {
    pub fn get(&self, command: Command) -> Option<Hotkey> {
        self.0
            .get(&command)
            .copied()
            .unwrap_or(Some(command.default_hotkey()))
    }

    pub fn set(&mut self, command: Command, hotkey: Option<Hotkey>) {
        if let Some(hotkey) = hotkey {
            for other in Command::ALL {
                if other != command && self.get(other) == Some(hotkey) {
                    self.0.insert(other, None);
                }
            }
        }
        if hotkey == Some(command.default_hotkey()) {
            self.0.remove(&command);
        } else {
            self.0.insert(command, hotkey);
        }
    }

    pub fn reset(&mut self) {
        self.0.clear();
    }

    pub fn global(&self) -> Vec<(Command, Hotkey)> {
        Command::ALL
            .into_iter()
            .filter(|command| command.is_global())
            .filter_map(|command| self.get(command).map(|hotkey| (command, hotkey)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_match_shadowplay_style() {
        assert_eq!(
            Command::ToggleReplay.default_hotkey().label(),
            "Alt+Shift+F10"
        );
        assert_eq!(Command::ToggleMic.default_hotkey().label(), "Ctrl+Alt+M");
        assert_eq!(Command::PushToTalk.default_hotkey().label(), "`");
    }

    #[test]
    fn cleared_binding_stays_cleared() {
        let mut bindings = Bindings::default();
        bindings.set(Command::Screenshot, None);
        assert_eq!(bindings.get(Command::Screenshot), None);
        let json = serde_json::to_string(&bindings).unwrap();
        let restored: Bindings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.get(Command::Screenshot), None);
    }

    #[test]
    fn rebinding_steals_duplicate() {
        let mut bindings = Bindings::default();
        let hotkey = Command::SaveReplay.default_hotkey();
        bindings.set(Command::Screenshot, Some(hotkey));
        assert_eq!(bindings.get(Command::Screenshot), Some(hotkey));
        assert_eq!(bindings.get(Command::SaveReplay), None);
    }

    #[test]
    fn push_to_talk_is_not_global() {
        let global = Bindings::default().global();
        assert!(
            global
                .iter()
                .all(|(command, _)| *command != Command::PushToTalk)
        );
    }
}
