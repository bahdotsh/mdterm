use crate::config::Config;
use crossterm::style::Color;
use std::collections::HashMap;
use std::fs;

#[derive(Clone)]
pub struct Theme {
    // Main background / foreground
    pub bg: Color,
    pub fg: Color,

    // Frame / chrome
    pub border: Color,
    pub title: Color,
    pub position: Color,
    pub help_hint: Color,
    pub scrollbar_track: Color,
    pub scrollbar_thumb: Color,

    // Headings
    pub h1: Color,
    pub h2: Color,
    pub h3: Color,
    pub h4: Color,
    pub h5: Color,
    pub h6: Color,
    pub heading_separator: Color,

    // Code blocks
    pub code_bg: Color,
    pub code_border: Color,
    pub code_label: Color,
    pub syntect_theme: &'static str,

    // Inline code
    pub inline_code_fg: Color,
    pub inline_code_bg: Color,
    pub inline_code_tick: Color,

    // Blockquote
    pub blockquote_bar: Color,

    // Links
    pub link: Color,
    pub link_url: Color,

    // Lists
    pub bullet: Color,
    pub task_done: Color,
    pub task_pending: Color,

    // Rules
    pub rule: Color,

    // Tables
    pub table_border: Color,
    pub table_header: Color,

    // Search
    pub search_prompt: Color,
    pub search_match_bg: Color,
    pub search_current_bg: Color,
    pub search_current_fg: Color,
    pub search_no_match: Color,

    // Overlays (TOC, link picker, fuzzy search)
    pub overlay_bg: Color,
    pub overlay_border: Color,
    pub overlay_selected_bg: Color,
    pub overlay_selected_fg: Color,
    pub overlay_text: Color,
    pub overlay_muted: Color,

    // Images
    pub image_fg: Color,

    // Slide mode
    pub slide_indicator: Color,

    // Math
    pub math_fg: Color,

    // Line numbers
    pub line_number: Color,

    // JSON
    pub json_key: Color,
    pub json_string: Color,
    pub json_number: Color,
    pub json_bool: Color,
    pub json_null: Color,
    pub json_bracket: Color,
    pub json_path: Color,
    pub json_focus_bg: Color,

    is_dark: bool,
}

fn load_theme_overrides(theme_name: Option<String>) -> HashMap<String, Color> {
    let mut overrides = HashMap::new();

    let name = match theme_name {
        Some(n) => n,
        None => return overrides,
    };

    let theme_path = dirs::config_dir().map(|d| {
        d.join("mdterm")
            .join("themes")
            .join(format!("{}.toml", name))
    });

    let path = match theme_path {
        Some(p) => p,
        None => return overrides,
    };

    // Parse the target theme TOML file into a flat key-value map
    if let Ok(contents) = fs::read_to_string(path)
        && let Ok(theme_map) = toml::from_str::<HashMap<String, String>>(&contents)
    {
        for (key, val) in theme_map {
            let hex = val.trim_start_matches('#');
            if hex.len() == 6 {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                overrides.insert(key, Color::Rgb { r, g, b });
            }
        }
    }

    overrides
}

impl Theme {
    pub fn dark() -> Self {
        let config = Config::load();
        let overrides = load_theme_overrides(config.theme_dark);

        macro_rules! resolve_color {
            ($key:expr, $r:expr, $g:expr, $b:expr) => {
                overrides.get($key).copied().unwrap_or(Color::Rgb {
                    r: $r,
                    g: $g,
                    b: $b,
                })
            };
        }

        Self {
            is_dark: true,
            bg: resolve_color!("bg", 30, 30, 46),
            fg: resolve_color!("fg", 205, 214, 244),

            border: resolve_color!("border", 68, 71, 90),
            title: resolve_color!("title", 147, 153, 178),
            position: resolve_color!("position", 108, 112, 134),
            help_hint: resolve_color!("help_hint", 88, 91, 112),
            scrollbar_track: resolve_color!("scrollbar_track", 49, 50, 68),
            scrollbar_thumb: resolve_color!("scrollbar_thumb", 127, 132, 156),

            h1: resolve_color!("h1", 205, 214, 244),
            h2: resolve_color!("h2", 137, 180, 250),
            h3: resolve_color!("h3", 203, 166, 247),
            h4: resolve_color!("h4", 166, 227, 161),
            h5: resolve_color!("h5", 249, 226, 175),
            h6: resolve_color!("h6", 127, 132, 156),
            heading_separator: resolve_color!("heading_separator", 49, 50, 68),

            code_bg: resolve_color!("code_bg", 30, 32, 42),
            code_border: resolve_color!("code_border", 68, 71, 90),
            code_label: resolve_color!("code_label", 108, 112, 134),
            syntect_theme: "base16-ocean.dark",

            inline_code_fg: resolve_color!("inline_code_fg", 242, 205, 147),
            inline_code_bg: resolve_color!("inline_code_bg", 40, 42, 54),
            inline_code_tick: resolve_color!("inline_code_tick", 68, 71, 90),

            blockquote_bar: resolve_color!("blockquote_bar", 116, 143, 196),

            link: resolve_color!("link", 137, 180, 250),
            link_url: resolve_color!("link_url", 108, 112, 134),

            bullet: resolve_color!("bullet", 127, 132, 156),
            task_done: resolve_color!("task_done", 166, 227, 161),
            task_pending: resolve_color!("task_pending", 108, 112, 134),

            rule: resolve_color!("rule", 68, 71, 90),

            table_border: resolve_color!("table_border", 68, 71, 90),
            table_header: resolve_color!("table_header", 137, 180, 250),

            search_prompt: resolve_color!("search_prompt", 249, 226, 175),
            search_match_bg: resolve_color!("search_match_bg", 100, 80, 0),
            search_current_bg: resolve_color!("search_current_bg", 249, 226, 175),
            search_current_fg: resolve_color!("search_current_fg", 24, 24, 37),
            search_no_match: resolve_color!("search_no_match", 243, 139, 168),

            overlay_bg: resolve_color!("overlay_bg", 36, 39, 58),
            overlay_border: resolve_color!("overlay_border", 91, 96, 120),
            overlay_selected_bg: resolve_color!("overlay_selected_bg", 68, 71, 90),
            overlay_selected_fg: resolve_color!("overlay_selected_fg", 205, 214, 244),
            overlay_text: resolve_color!("overlay_text", 186, 194, 222),
            overlay_muted: resolve_color!("overlay_muted", 108, 112, 134),

            image_fg: resolve_color!("image_fg", 166, 227, 161),

            slide_indicator: resolve_color!("slide_indicator", 249, 226, 175),

            math_fg: resolve_color!("math_fg", 242, 205, 147),

            line_number: resolve_color!("line_number", 68, 71, 90),

            json_key: resolve_color!("json_key", 137, 180, 250),
            json_string: resolve_color!("json_string", 166, 227, 161),
            json_number: resolve_color!("json_number", 250, 179, 135),
            json_bool: resolve_color!("json_bool", 249, 226, 175),
            json_null: resolve_color!("json_null", 108, 112, 134),
            json_bracket: resolve_color!("json_bracket", 127, 132, 156),
            json_path: resolve_color!("json_path", 203, 166, 247),
            json_focus_bg: resolve_color!("json_focus_bg", 40, 42, 54),
        }
    }

    pub fn light() -> Self {
        let config = Config::load();
        let overrides = load_theme_overrides(config.theme_light);

        macro_rules! resolve_color {
            ($key:expr, $r:expr, $g:expr, $b:expr) => {
                overrides.get($key).copied().unwrap_or(Color::Rgb {
                    r: $r,
                    g: $g,
                    b: $b,
                })
            };
        }

        Self {
            is_dark: false,

            bg: resolve_color!("bg", 239, 241, 245),
            fg: resolve_color!("fg", 76, 79, 105),

            border: resolve_color!("border", 172, 176, 190),
            title: resolve_color!("title", 92, 95, 119),
            position: resolve_color!("position", 108, 111, 133),
            help_hint: resolve_color!("help_hint", 140, 143, 161),
            scrollbar_track: resolve_color!("scrollbar_track", 204, 208, 218),
            scrollbar_thumb: resolve_color!("scrollbar_thumb", 140, 143, 161),

            h1: resolve_color!("h1", 32, 32, 42),
            h2: resolve_color!("h2", 30, 102, 245),
            h3: resolve_color!("h3", 136, 57, 239),
            h4: resolve_color!("h4", 64, 160, 43),
            h5: resolve_color!("h5", 223, 142, 29),
            h6: resolve_color!("h6", 108, 111, 133),
            heading_separator: resolve_color!("heading_separator", 204, 208, 218),

            code_bg: resolve_color!("code_bg", 239, 241, 245),
            code_border: resolve_color!("code_border", 188, 192, 204),
            code_label: resolve_color!("code_label", 124, 127, 147),
            syntect_theme: "InspiredGitHub",

            inline_code_fg: resolve_color!("inline_code_fg", 179, 82, 2),
            inline_code_bg: resolve_color!("inline_code_bg", 230, 233, 239),
            inline_code_tick: resolve_color!("inline_code_tick", 172, 176, 190),

            blockquote_bar: resolve_color!("blockquote_bar", 30, 102, 245),

            link: resolve_color!("link", 30, 102, 245),
            link_url: resolve_color!("link_url", 140, 143, 161),

            bullet: resolve_color!("bullet", 108, 111, 133),
            task_done: resolve_color!("task_done", 64, 160, 43),
            task_pending: resolve_color!("task_pending", 140, 143, 161),

            rule: resolve_color!("rule", 188, 192, 204),

            table_border: resolve_color!("table_border", 188, 192, 204),
            table_header: resolve_color!("table_header", 30, 102, 245),

            search_prompt: resolve_color!("search_prompt", 223, 142, 29),
            search_match_bg: resolve_color!("search_match_bg", 255, 235, 160),
            search_current_bg: resolve_color!("search_current_bg", 253, 205, 54),
            search_current_fg: resolve_color!("search_current_fg", 32, 32, 42),
            search_no_match: resolve_color!("search_no_match", 210, 15, 57),

            overlay_bg: resolve_color!("overlay_bg", 230, 233, 239),
            overlay_border: resolve_color!("overlay_border", 172, 176, 190),
            overlay_selected_bg: resolve_color!("overlay_selected_bg", 188, 192, 204),
            overlay_selected_fg: resolve_color!("overlay_selected_fg", 76, 79, 105),
            overlay_text: resolve_color!("overlay_text", 76, 79, 105),
            overlay_muted: resolve_color!("overlay_muted", 140, 143, 161),

            image_fg: resolve_color!("image_fg", 64, 160, 43),

            slide_indicator: resolve_color!("slide_indicator", 223, 142, 29),

            math_fg: resolve_color!("math_fg", 179, 82, 2),

            line_number: resolve_color!("line_number", 172, 176, 190),

            json_key: resolve_color!("json_key", 30, 102, 245),
            json_string: resolve_color!("json_string", 64, 160, 43),
            json_number: resolve_color!("json_number", 254, 100, 11),
            json_bool: resolve_color!("json_bool", 223, 142, 29),
            json_null: resolve_color!("json_null", 140, 143, 161),
            json_bracket: resolve_color!("json_bracket", 108, 111, 133),
            json_path: resolve_color!("json_path", 136, 57, 239),
            json_focus_bg: resolve_color!("json_focus_bg", 220, 224, 232),
        }
    }

    pub fn toggle(&self) -> Self {
        if self.is_dark {
            Self::light()
        } else {
            Self::dark()
        }
    }
}
