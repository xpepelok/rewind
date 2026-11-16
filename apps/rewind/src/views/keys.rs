use gpui::Keystroke;

use rewind_config::{Hotkey, Modifiers};

const VK_F1: u16 = 0x70;

pub fn virtual_key(key: &str) -> Option<u16> {
    let single = {
        let mut characters = key.chars();
        match (characters.next(), characters.next()) {
            (Some(character), None) => Some(character),
            _ => None,
        }
    };
    if let Some(character) = single {
        return match character {
            'a'..='z' => Some(character.to_ascii_uppercase() as u16),
            '0'..='9' => Some(character as u16),
            '`' => Some(0xC0),
            '-' => Some(0xBD),
            '=' => Some(0xBB),
            '[' => Some(0xDB),
            ']' => Some(0xDD),
            '\\' => Some(0xDC),
            ';' => Some(0xBA),
            '\'' => Some(0xDE),
            ',' => Some(0xBC),
            '.' => Some(0xBE),
            '/' => Some(0xBF),
            _ => None,
        };
    }
    if let Some(number) = key
        .strip_prefix('f')
        .and_then(|rest| rest.parse::<u16>().ok())
        && (1..=24).contains(&number)
    {
        return Some(VK_F1 + number - 1);
    }
    Some(match key {
        "space" => 0x20,
        "tab" => 0x09,
        "enter" => 0x0D,
        "pause" => 0x13,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        _ => return None,
    })
}

pub enum Captured {
    Cancel,
    Clear,
    Bind(Hotkey),
    Ignore,
}

pub fn capture(keystroke: &Keystroke) -> Captured {
    let modifiers = Modifiers {
        ctrl: keystroke.modifiers.control,
        alt: keystroke.modifiers.alt,
        shift: keystroke.modifiers.shift,
        win: keystroke.modifiers.platform,
    };
    match keystroke.key.as_str() {
        "escape" if modifiers.is_empty() => Captured::Cancel,
        "backspace" | "delete" if modifiers.is_empty() => Captured::Clear,
        key => virtual_key(key).map_or(Captured::Ignore, |code| {
            Captured::Bind(Hotkey::new(modifiers, code))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::virtual_key;

    #[test]
    fn maps_letters_digits_and_function_keys() {
        assert_eq!(virtual_key("z"), Some(0x5A));
        assert_eq!(virtual_key("7"), Some(0x37));
        assert_eq!(virtual_key("f10"), Some(0x79));
        assert_eq!(virtual_key("f12"), Some(0x7B));
        assert_eq!(virtual_key("`"), Some(0xC0));
        assert_eq!(virtual_key("shift"), None);
        assert_eq!(virtual_key("f25"), None);
    }
}
