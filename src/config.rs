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
    #[serde(default)]
    pub hide: HideConfig,
    #[serde(default)]
    pub picker: PickerConfig,
    /// Fallback folder (relative to the current file's own directory) checked
    /// for a bare-filename wikilink embed (`![[photo.png]]`) that isn't found
    /// directly next to the file — mirrors Obsidian's own default attachment
    /// folder. Plain CommonMark images never use this fallback.
    #[serde(default = "default_attachments_dir")]
    pub attachments_dir: String,
}

/// Which files the file picker will offer.
#[derive(Deserialize, Default, Clone, Debug)]
pub struct PickerConfig {
    /// Path component names to skip, matched against each file and directory
    /// name, e.g. `["node_modules", "attachments"]`.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Descend into dot-directories and offer dot-files. Off by default, so
    /// `.obsidian`, `.git` and `.trash` are skipped — the same default as `fd`.
    #[serde(default)]
    pub hidden: bool,
    /// Cap on how many results the list shows at once. `0` means the only
    /// limit is terminal height. This is a display cap: every file is still
    /// matched against the query, so anything can be found by typing.
    #[serde(default)]
    pub max_results: usize,
}

impl PickerConfig {
    /// `name` is a single path component, not a whole path.
    pub fn skips(&self, name: &str) -> bool {
        if !self.hidden && name.starts_with('.') {
            return true;
        }
        self.ignore
            .iter()
            .any(|ignored| ignored.trim().eq_ignore_ascii_case(name))
    }
}

/// Content that is parsed but deliberately never rendered.
#[derive(Deserialize, Default, Clone)]
pub struct HideConfig {
    #[serde(default)]
    pub images: bool,
    /// A leading YAML metadata block delimited by `---`.
    #[serde(default)]
    pub frontmatter: bool,
    /// Fenced code block languages to omit entirely, e.g. `dataviewjs`.
    #[serde(default)]
    pub code_languages: Vec<String>,
    /// Images nested inside a link's label (e.g. an icon prefixing a link,
    /// `[Label ![icon](path)](url)`) render as a block that breaks the
    /// link's line. Off by default — existing vaults that intentionally
    /// embed a real image inside a link (e.g. a clickable thumbnail) keep
    /// today's behavior unless they opt in.
    #[serde(default)]
    pub images_in_links: bool,
}

impl HideConfig {
    /// `info` is the raw fence info string, which may carry attributes after the
    /// language: a fence opened with `js title="x"` arrives here in full, so only
    /// the leading token is compared.
    pub fn hides_language(&self, info: &str) -> bool {
        let Some(lang) = info.split_whitespace().next() else {
            return false;
        };
        self.code_languages
            .iter()
            .any(|hidden| hidden.trim().eq_ignore_ascii_case(lang))
    }
}

fn default_theme() -> String {
    "dark".to_string()
}

fn default_attachments_dir() -> String {
    "attachments".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            line_numbers: false,
            width: 0,
            hide: HideConfig::default(),
            picker: PickerConfig::default(),
            attachments_dir: default_attachments_dir(),
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

/// `mdterm` is still accepted so a config written before the rename keeps working.
const APP_DIRS: [&str; 2] = ["mdviewer", "mdterm"];

fn config_path() -> Option<PathBuf> {
    let mut paths = Vec::new();
    for base in config_bases() {
        for app in APP_DIRS {
            paths.push(base.join(app).join("config.toml"));
        }
    }

    paths
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .or_else(|| paths.first().cloned())
}

/// Directories searched for `<app>/config.toml`, in precedence order.
///
/// `dirs::config_dir()` alone is not enough: on macOS it resolves to
/// `~/Library/Application Support`, so the `~/.config/mdviewer/config.toml` the
/// README documents would never be read.
fn config_bases() -> Vec<PathBuf> {
    let mut bases = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        bases.push(PathBuf::from(xdg));
    }
    if let Some(home) = dirs::home_dir() {
        bases.push(home.join(".config"));
    }
    if let Some(platform) = dirs::config_dir() {
        bases.push(platform);
    }
    bases
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hide(langs: &[&str]) -> HideConfig {
        HideConfig {
            images: false,
            frontmatter: false,
            code_languages: langs.iter().map(|s| s.to_string()).collect(),
            images_in_links: false,
        }
    }

    #[test]
    fn matches_plain_language() {
        assert!(hide(&["dataviewjs"]).hides_language("dataviewjs"));
    }

    #[test]
    fn ignores_attributes_after_the_language() {
        assert!(hide(&["dataviewjs"]).hides_language("dataviewjs foo=bar"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(hide(&["DataviewJS"]).hides_language("dataviewjs"));
    }

    #[test]
    fn does_not_match_other_languages() {
        assert!(!hide(&["dataviewjs"]).hides_language("rust"));
    }

    #[test]
    fn does_not_match_a_language_prefix() {
        assert!(!hide(&["data"]).hides_language("dataviewjs"));
    }

    #[test]
    fn unlabelled_blocks_are_never_hidden() {
        assert!(!hide(&["dataviewjs"]).hides_language(""));
        assert!(!hide(&["dataviewjs"]).hides_language("   "));
    }

    #[test]
    fn empty_config_hides_nothing() {
        assert!(!HideConfig::default().hides_language("dataviewjs"));
    }

    fn picker(ignore: &[&str], hidden: bool) -> PickerConfig {
        PickerConfig {
            ignore: ignore.iter().map(|s| s.to_string()).collect(),
            hidden,
            max_results: 0,
        }
    }

    #[test]
    fn dot_names_are_skipped_by_default() {
        let cfg = PickerConfig::default();
        assert!(cfg.skips(".obsidian"));
        assert!(cfg.skips(".git"));
        assert!(!cfg.skips("notes"));
    }

    #[test]
    fn hidden_true_stops_skipping_dot_names() {
        assert!(!picker(&[], true).skips(".obsidian"));
    }

    #[test]
    fn ignore_list_matches_names_case_insensitively() {
        let cfg = picker(&["Attachments"], false);
        assert!(cfg.skips("attachments"));
        assert!(cfg.skips("ATTACHMENTS"));
        assert!(!cfg.skips("attachments-old"));
    }

    #[test]
    fn ignore_list_matches_file_names_too() {
        assert!(picker(&["scratch.md"], false).skips("scratch.md"));
    }

    #[test]
    fn default_picker_skips_nothing_visible() {
        let cfg = PickerConfig::default();
        assert!(!cfg.skips("notes"));
        assert!(!cfg.skips("README.md"));
    }

    #[test]
    fn default_attachments_dir_is_attachments() {
        assert_eq!(Config::default().attachments_dir, "attachments");
    }

    #[test]
    fn attachments_dir_deserializes_from_toml() {
        let cfg: Config = toml::from_str("attachments_dir = \"assets\"").unwrap();
        assert_eq!(cfg.attachments_dir, "assets");
    }
}
