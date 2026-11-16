use std::ops::Range;

const SUFFIX_KEY: &str = "\"LocalConnectionSuffix\"";
pub const DRIVER_FILE: &str = "файл настроек драйвера";
pub const APP_FILE: &str = "настройки приложения с адресом драйвера";
pub const MINIMAL: &str = "минимальные настройки";
pub const DRIVER_PATH: &str = "путь к файлу настроек драйвера";

fn suffix_value(config: &str) -> Option<Range<usize>> {
    let after_key = config.find(SUFFIX_KEY)? + SUFFIX_KEY.len();
    let colon = after_key + config[after_key..].find(':')?;
    let open = colon + config[colon..].find('"')? + 1;
    let close = open + config[open..].find('"')?;
    Some(open..close)
}

pub fn suffix(config: &str) -> Option<&str> {
    suffix_value(config).map(|range| &config[range])
}

pub fn with_suffix(config: &str, suffix: &str) -> Option<String> {
    let range = suffix_value(config)?;
    Some(format!(
        "{}{suffix}{}",
        &config[..range.start],
        &config[range.end..]
    ))
}

pub fn candidates(driver: &str, application: Option<&str>) -> Vec<(&'static str, String)> {
    let driver = driver.trim_start_matches('\u{feff}').trim();
    let mut list = vec![(DRIVER_FILE, driver.to_owned())];
    if let Some(suffix) = suffix(driver).filter(|suffix| !suffix.is_empty()) {
        if let Some(adapted) = application.and_then(|config| with_suffix(config, suffix)) {
            list.push((APP_FILE, adapted));
        }
        list.push((
            MINIMAL,
            format!("{{\"LocalConnectionSuffix\":\"{suffix}\",\"MessageBusPort\":1}}"),
        ));
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRIVER: &str = "\u{feff}{\n\t\"LocalConnectionSuffix\": \"NVIDIA.Display.Driver\",\n    \"MessageBusPort\": 1\n}\n";
    const APPLICATION: &str = "{\n    \"LogLevel\": 4,\n    \"LocalConnectionSuffix\": \"\",\n    \"MessageBusPort\": 1\n}";

    #[test]
    fn the_driver_bus_is_named_by_its_connection_suffix() {
        assert_eq!(suffix(DRIVER), Some("NVIDIA.Display.Driver"));
        assert_eq!(suffix(APPLICATION), Some(""));
        assert_eq!(suffix("{\"MessageBusPort\": 1}"), None);
    }

    #[test]
    fn only_the_suffix_changes_when_the_application_settings_are_pointed_at_the_driver() {
        let adapted = with_suffix(APPLICATION, "NVIDIA.Display.Driver").unwrap();
        assert_eq!(
            adapted,
            APPLICATION.replace(
                "\"LocalConnectionSuffix\": \"\"",
                "\"LocalConnectionSuffix\": \"NVIDIA.Display.Driver\""
            )
        );
    }

    #[test]
    fn the_driver_file_is_tried_first_then_adapted_then_minimal_settings() {
        let list = candidates(DRIVER, Some(APPLICATION));
        let labels: Vec<_> = list.iter().map(|(label, _)| *label).collect();
        assert_eq!(labels, [DRIVER_FILE, APP_FILE, MINIMAL]);
        assert!(list[0].1.starts_with('{'));
        assert!(
            list.iter()
                .all(|(_, config)| suffix(config) == Some("NVIDIA.Display.Driver"))
        );
    }

    #[test]
    fn without_a_suffix_there_is_nothing_to_adapt() {
        let list = candidates("{\"MessageBusPort\": 1}", Some(APPLICATION));
        assert_eq!(list.len(), 1);
    }
}
