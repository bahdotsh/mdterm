use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Deserialize)]
pub struct Config {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub line_numbers: bool,
    #[serde(default)]
    pub width: usize,
}

fn default_theme() -> String {
    "dark".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            line_numbers: false,
            width: 0,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        if let Some(path) = config_path()
            && let Ok(contents) = fs::read_to_string(&path)
            && let Ok(config) = toml::from_str(&contents)
        {
            return config;
        }
        Config::default()
    }
}

fn config_path() -> Option<PathBuf> {
    // Check XDG-style ~/.config first (takes precedence on all platforms),
    // then fall back to the platform config dir (e.g. ~/Library/Preferences on macOS).
    let xdg = dirs::home_dir().map(|h| h.join(".config").join("mdterm").join("config.toml"));
    if let Some(ref p) = xdg {
        if p.exists() {
            return xdg;
        }
    }
    dirs::config_dir().map(|d| d.join("mdterm").join("config.toml"))
}
