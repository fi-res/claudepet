//! Preferences that survive restarts, stored as JSON in the app's config folder.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Ru,
    En,
}

impl Lang {
    /// Russian for Russian-language systems, English otherwise.
    pub fn from_system() -> Self {
        match sys_locale::get_locale() {
            Some(locale) if locale.to_lowercase().starts_with("ru") => Lang::Ru,
            _ => Lang::En,
        }
    }

    /// What the model is told to answer in.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Ru => "Russian",
            Lang::En => "English",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: Lang,
    pub claude_path: Option<PathBuf>,
    pub listen: bool,
    pub watch: bool,
    pub llm: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Lang::from_system(),
            claude_path: None,
            listen: true,
            watch: true,
            llm: true,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        let result = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default()));
        if let Err(e) = result {
            eprintln!("settings: cannot save {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_missing_fields() {
        let dir = std::env::temp_dir().join(format!("clawd-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        let settings = Settings {
            language: Lang::En,
            claude_path: Some("/opt/claude".into()),
            listen: false,
            watch: true,
            llm: false,
        };
        settings.save(&path);
        assert_eq!(Settings::load(&path), settings);
        fs::write(&path, r#"{"language":"ru"}"#).unwrap();
        let partial = Settings::load(&path);
        assert_eq!(partial.language, Lang::Ru);
        assert!(partial.listen && partial.llm && partial.claude_path.is_none());
        fs::write(&path, "garbage").unwrap();
        assert!(Settings::load(&path).listen);
        let _ = fs::remove_dir_all(dir);
    }
}
