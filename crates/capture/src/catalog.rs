use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const CATALOG: &str =
    r"NVIDIA Corporation\NVIDIA app\NvBackend\ApplicationOntology\data\fingerprint.db";
const ENTRY: &str = "<Fingerprint ";
const NAME_OPEN: &str = "<DisplayName>";
const NAME_CLOSE: &str = "</DisplayName>";
const IMAGE_OPEN: &str = "<Image>";
const IMAGE_CLOSE: &str = "</Image>";
const STORE_APPS: &str = "\\windowsapps\\";
const RUNTIMES: [&str; 8] = [
    "java.exe",
    "javaw.exe",
    "python.exe",
    "pythonw.exe",
    "node.exe",
    "dotnet.exe",
    "unitycrashhandler64.exe",
    "unitycrashhandler32.exe",
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct Game {
    image: String,
    name: String,
}

fn catalog_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join(CATALOG))
}

fn unescape(text: &str) -> String {
    text.replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        rest = &rest[start + open.len()..];
        let Some(end) = rest.find(close) else {
            break;
        };
        found.push(&rest[..end]);
        rest = &rest[end + close.len()..];
    }
    found
}

fn normalize(path: &str) -> String {
    path.trim().replace('/', "\\").to_lowercase()
}

fn parse(xml: &str) -> Vec<Game> {
    let mut games = Vec::new();
    for entry in xml.split(ENTRY).skip(1) {
        let Some(name) = between(entry, NAME_OPEN, NAME_CLOSE)
            .first()
            .map(|name| unescape(name))
        else {
            continue;
        };
        for image in between(entry, IMAGE_OPEN, IMAGE_CLOSE) {
            let image = normalize(&unescape(image));
            if !image.is_empty() {
                games.push(Game {
                    image,
                    name: name.clone(),
                });
            }
        }
    }
    games.sort_by_key(|game| std::cmp::Reverse(game.image.len()));
    games
}

fn games() -> &'static [Game] {
    static GAMES: OnceLock<Vec<Game>> = OnceLock::new();
    GAMES.get_or_init(|| {
        catalog_path()
            .and_then(|path| std::fs::read(path).ok())
            .map(|bytes| parse(&String::from_utf8_lossy(&bytes)))
            .unwrap_or_default()
    })
}

fn lookup(games: &[Game], executable: &Path) -> Option<String> {
    let full = normalize(&executable.to_string_lossy());
    games
        .iter()
        .find(|game| {
            let shared_runtime = !game.image.contains('\\')
                && RUNTIMES.contains(&game.image.as_str())
                && !full.contains(STORE_APPS);
            !shared_runtime
                && full.ends_with(&game.image)
                && (full.len() == game.image.len()
                    || full.as_bytes()[full.len() - game.image.len() - 1] == b'\\')
        })
        .map(|game| game.name.clone())
}

pub fn display_name(executable: &Path) -> Option<String> {
    lookup(games(), executable)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r"<FingerprintDB>
  <Fingerprint name='cs2'>
    <DisplayName>Counter-Strike 2</DisplayName>
    <Version name='steam'><Image>game\bin\win64\cs2.exe</Image></Version>
  </Fingerprint>
  <Fingerprint name='pubg'>
    <DisplayName>PLAYERUNKNOWN&apos;S BATTLEGROUNDS</DisplayName>
    <Version name='steam'><Image>TslGame\Binaries\Win64\TslGame.exe</Image></Version>
  </Fingerprint>
</FingerprintDB>";

    #[test]
    fn games_are_found_by_the_tail_of_their_executable_path() {
        let games = parse(SAMPLE);
        let cs2 = Path::new(
            r"D:\Steam\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe",
        );
        assert_eq!(lookup(&games, cs2).as_deref(), Some("Counter-Strike 2"));
        let pubg = Path::new(r"C:\Games\PUBG\TslGame\Binaries\Win64\TSLGAME.EXE");
        assert_eq!(
            lookup(&games, pubg).as_deref(),
            Some("PLAYERUNKNOWN'S BATTLEGROUNDS")
        );
        assert_eq!(lookup(&games, Path::new(r"C:\x\notcs2.exe")), None);
        let launcher = parse(
            "<Fingerprint name='mc'><DisplayName>Minecraft</DisplayName><Image>javaw.exe</Image></Fingerprint>",
        );
        let java = Path::new(r"C:\Program Files\Java\jdk-23\bin\javaw.exe");
        assert_eq!(lookup(&launcher, java), None);
        let store = Path::new(r"C:\Program Files\WindowsApps\Minecraft\javaw.exe");
        assert_eq!(lookup(&launcher, store).as_deref(), Some("Minecraft"));
    }
}
