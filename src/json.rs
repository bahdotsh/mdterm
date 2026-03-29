use std::collections::HashSet;

use crossterm::style::Color;
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::style::{CodeBlockContent, DocumentInfo, Line, LineMeta, Style, StyledSpan};
use crate::theme::Theme;

/// Maximum key width used for value alignment (prevents excessive padding)
const MAX_ALIGN_WIDTH: usize = 24;

pub fn render(
    input: &str,
    width: usize,
    theme: &Theme,
) -> Result<(Vec<Line>, DocumentInfo), String> {
    let value: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            let mut lines = Vec::new();
            render_parse_error(&mut lines, input, &e, theme, width);
            return Ok((
                lines,
                DocumentInfo {
                    code_blocks: Vec::<CodeBlockContent>::new(),
                },
            ));
        }
    };
    let mut renderer = JsonRenderer {
        theme,
        lines: Vec::new(),
        width,
    };
    renderer.render_root(&value);
    Ok((
        renderer.lines,
        DocumentInfo {
            code_blocks: Vec::<CodeBlockContent>::new(),
        },
    ))
}

/// Render a styled JSON parse error with source context and caret pointer
fn render_parse_error(
    lines: &mut Vec<Line>,
    input: &str,
    err: &serde_json::Error,
    theme: &Theme,
    width: usize,
) {
    let error_color = Color::Rgb {
        r: 243,
        g: 139,
        b: 168,
    };

    // Heading
    lines.push(Line {
        spans: vec![StyledSpan {
            text: "  Invalid JSON".to_string(),
            style: Style {
                fg: Some(error_color),
                bold: true,
                ..Default::default()
            },
        }],
        meta: LineMeta::Heading {
            level: 1,
            text: "Invalid JSON".to_string(),
        },
    });
    let sep_w = width.min(60);
    lines.push(Line {
        spans: vec![StyledSpan {
            text: "\u{2500}".repeat(sep_w),
            style: style_fg(theme.heading_separator),
        }],
        meta: LineMeta::None,
    });
    lines.push(Line::empty());

    // Error message
    lines.push(Line {
        spans: vec![
            StyledSpan {
                text: "  Error: ".to_string(),
                style: Style {
                    fg: Some(error_color),
                    bold: true,
                    ..Default::default()
                },
            },
            StyledSpan {
                text: format!("{}", err),
                style: style_fg(theme.fg),
            },
        ],
        meta: LineMeta::None,
    });
    lines.push(Line::empty());

    // Source context around the error position
    let err_line = err.line();
    let err_col = err.column();
    let source_lines: Vec<&str> = input.lines().collect();

    let start = err_line.saturating_sub(3);
    let end = (err_line + 2).min(source_lines.len());

    for i in start..end {
        let line_num = i + 1;
        let is_err = line_num == err_line;
        let content = source_lines.get(i).unwrap_or(&"");
        let num_str = format!("  {:>4} \u{2502} ", line_num);

        lines.push(Line {
            spans: vec![
                StyledSpan {
                    text: num_str,
                    style: Style {
                        fg: Some(if is_err {
                            error_color
                        } else {
                            theme.line_number
                        }),
                        dim: !is_err,
                        ..Default::default()
                    },
                },
                StyledSpan {
                    text: content.to_string(),
                    style: Style {
                        fg: Some(if is_err { theme.fg } else { theme.json_null }),
                        ..Default::default()
                    },
                },
            ],
            meta: LineMeta::None,
        });

        if is_err {
            let pointer_pad = 9 + err_col.saturating_sub(1);
            lines.push(Line {
                spans: vec![StyledSpan {
                    text: format!("{}^", " ".repeat(pointer_pad)),
                    style: Style {
                        fg: Some(error_color),
                        bold: true,
                        ..Default::default()
                    },
                }],
                meta: LineMeta::None,
            });
        }
    }

    lines.push(Line::empty());
}

struct JsonRenderer<'a> {
    theme: &'a Theme,
    lines: Vec<Line>,
    width: usize,
}

impl<'a> JsonRenderer<'a> {
    // ── entry point ───────────────────────────────────────────────

    fn render_root(&mut self, value: &Value) {
        match value {
            Value::Object(map) => {
                // Separate simple (primitive/empty) keys from section (object/array) keys
                let mut simple: Vec<(&String, &Value)> = Vec::new();
                let mut sections: Vec<(&String, &Value)> = Vec::new();

                for (key, val) in map {
                    if is_primitive_or_empty(val) {
                        simple.push((key, val));
                    } else {
                        sections.push((key, val));
                    }
                }

                // Render simple values as a compact aligned group
                if !simple.is_empty() {
                    let align = simple
                        .iter()
                        .map(|(k, _)| UnicodeWidthStr::width(k.as_str()))
                        .max()
                        .unwrap_or(0)
                        .min(MAX_ALIGN_WIDTH);

                    for (key, val) in &simple {
                        self.emit_kv(key, val, 1, align);
                    }

                    if !sections.is_empty() {
                        self.emit_blank();
                    }
                }

                // Render sections with H1 headings (for TOC navigation)
                for (i, (key, val)) in sections.iter().enumerate() {
                    let annotation = match val {
                        Value::Object(m) => format!("({} keys)", m.len()),
                        Value::Array(a) => format!("({} items)", a.len()),
                        _ => String::new(),
                    };
                    self.emit_heading_with_annotation(1, key, &annotation);
                    self.emit_blank();

                    self.render_value(val, 1);

                    if i < sections.len() - 1 {
                        self.emit_blank();
                    }
                }
            }
            Value::Array(arr) => {
                let annotation = if arr.is_empty() {
                    String::new()
                } else {
                    format!("({} items)", arr.len())
                };
                self.emit_heading_with_annotation(1, "root", &annotation);
                self.emit_blank();
                self.render_array(arr, 1);
            }
            _ => {
                self.emit_heading(1, "value");
                self.emit_blank();
                self.emit_indented_value(value, 1);
            }
        }
    }

    // ── recursive renderers ───────────────────────────────────────

    fn render_value(&mut self, value: &Value, depth: usize) {
        match value {
            Value::Object(map) => self.render_object(map, depth),
            Value::Array(arr) => self.render_array(arr, depth),
            _ => self.emit_indented_value(value, depth),
        }
    }

    fn render_object(&mut self, map: &serde_json::Map<String, Value>, depth: usize) {
        if map.is_empty() {
            let indent = indent_str(depth);
            self.push_line(
                vec![
                    StyledSpan {
                        text: format!("{}{}", indent, "{}"),
                        style: style_fg(self.theme.json_bracket),
                    },
                    StyledSpan {
                        text: " empty".to_string(),
                        style: Style {
                            fg: Some(self.theme.json_null),
                            dim: true,
                            ..Default::default()
                        },
                    },
                ],
                LineMeta::None,
            );
            return;
        }

        // Group simple keys (primitives/empty) before section keys (objects/arrays)
        let mut simple: Vec<(&String, &Value)> = Vec::new();
        let mut sections: Vec<(&String, &Value)> = Vec::new();

        for (key, val) in map {
            if is_primitive_or_empty(val) {
                simple.push((key, val));
            } else {
                sections.push((key, val));
            }
        }

        // Render simple values first, aligned
        if !simple.is_empty() {
            let align_width = simple
                .iter()
                .map(|(k, _)| UnicodeWidthStr::width(k.as_str()))
                .max()
                .unwrap_or(0)
                .min(MAX_ALIGN_WIDTH);

            for (key, val) in &simple {
                self.emit_kv(key, val, depth, align_width);
            }
        }

        // Render sections with labels and blank line separators
        for (i, (key, val)) in sections.iter().enumerate() {
            // Blank line before each section
            if i > 0 || !simple.is_empty() {
                self.emit_blank();
            }

            match val {
                Value::Object(inner) => {
                    let annotation = format!("({} keys)", inner.len());
                    self.emit_section_label(key, depth, &annotation);
                    self.render_object(inner, depth + 1);
                }
                Value::Array(arr) => {
                    let annotation = format!("({} items)", arr.len());
                    self.emit_section_label(key, depth, &annotation);
                    self.render_array(arr, depth + 1);
                }
                _ => {}
            }
        }
    }

    fn render_array(&mut self, arr: &[Value], depth: usize) {
        if arr.is_empty() {
            let indent = indent_str(depth);
            self.push_line(
                vec![
                    StyledSpan {
                        text: format!("{}[]", indent),
                        style: style_fg(self.theme.json_bracket),
                    },
                    StyledSpan {
                        text: " empty".to_string(),
                        style: Style {
                            fg: Some(self.theme.json_null),
                            dim: true,
                            ..Default::default()
                        },
                    },
                ],
                LineMeta::None,
            );
            return;
        }

        // Homogeneous object arrays → table
        if should_render_as_table(arr) {
            self.render_table(arr, depth);
            return;
        }

        let all_primitive = arr.iter().all(is_primitive_or_empty);

        if all_primitive {
            // Clean bullet list for primitive arrays
            for item in arr {
                self.emit_bullet(item, depth);
            }
        } else {
            // Mixed/complex array with index labels
            let mut prev_complex = false;
            for (i, item) in arr.iter().enumerate() {
                let is_complex = matches!(item, Value::Object(m) if !m.is_empty())
                    || matches!(item, Value::Array(a) if !a.is_empty());

                if i > 0 && (is_complex || prev_complex) {
                    self.emit_blank();
                }

                match item {
                    Value::Object(map) if !map.is_empty() => {
                        self.emit_index_label(i, depth);
                        self.render_object(map, depth + 1);
                        prev_complex = true;
                    }
                    Value::Array(inner) if !inner.is_empty() => {
                        let label = format!("({} items)", inner.len());
                        self.emit_index_label_with_annotation(i, &label, depth);
                        self.render_array(inner, depth + 1);
                        prev_complex = true;
                    }
                    _ => {
                        self.emit_indexed_value(i, item, depth);
                        prev_complex = false;
                    }
                }
            }
        }
    }

    // ── table rendering ───────────────────────────────────────────

    fn render_table(&mut self, arr: &[Value], depth: usize) {
        let objects: Vec<&serde_json::Map<String, Value>> =
            arr.iter().filter_map(|v| v.as_object()).collect();
        if objects.is_empty() {
            return;
        }

        // Collect all keys preserving first-seen order
        let mut headers: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for obj in &objects {
            for key in obj.keys() {
                if seen.insert(key.clone()) {
                    headers.push(key.clone());
                }
            }
        }

        // Build cell text matrix
        let rows: Vec<Vec<String>> = objects
            .iter()
            .map(|obj| {
                headers
                    .iter()
                    .map(|h| match obj.get(h) {
                        Some(v) => value_to_short_string(v),
                        None => String::new(),
                    })
                    .collect()
            })
            .collect();

        // Compute column widths
        let indent = indent_str(depth);
        let indent_w = UnicodeWidthStr::width(indent.as_str());
        let available = self.width.saturating_sub(indent_w);

        let mut col_widths: Vec<usize> = headers
            .iter()
            .enumerate()
            .map(|(ci, h)| {
                let header_w = UnicodeWidthStr::width(h.as_str());
                let max_cell = rows
                    .iter()
                    .map(|r| UnicodeWidthStr::width(r[ci].as_str()))
                    .max()
                    .unwrap_or(0);
                header_w.max(max_cell).max(3)
            })
            .collect();

        // Shrink columns if total exceeds available width
        let separators = if headers.len() > 1 {
            (headers.len() - 1) * 3
        } else {
            0
        };
        let border_chars = 4; // "│ " prefix + " │" suffix
        let total_need: usize = col_widths.iter().sum::<usize>() + separators + border_chars;
        if total_need > available && available > border_chars + separators + headers.len() {
            let usable = available - border_chars - separators;
            let current_total: usize = col_widths.iter().sum();
            for w in &mut col_widths {
                *w = (*w * usable / current_total).max(3);
            }
        }

        let bc = self.theme.table_border;
        let hc = self.theme.table_header;

        // Top border
        let top: String = col_widths
            .iter()
            .map(|w| "\u{2500}".repeat(*w))
            .collect::<Vec<_>>()
            .join("\u{2500}\u{252c}\u{2500}");
        self.push_line(
            vec![StyledSpan {
                text: format!("{}\u{250c}\u{2500}{}\u{2500}\u{2510}", indent, top),
                style: style_fg(bc),
            }],
            LineMeta::None,
        );

        // Header row
        let mut hdr = vec![StyledSpan {
            text: format!("{}\u{2502} ", indent),
            style: style_fg(bc),
        }];
        for (ci, h) in headers.iter().enumerate() {
            hdr.push(StyledSpan {
                text: pad_or_truncate(h, col_widths[ci]),
                style: Style {
                    fg: Some(hc),
                    bold: true,
                    ..Default::default()
                },
            });
            if ci < headers.len() - 1 {
                hdr.push(StyledSpan {
                    text: " \u{2502} ".to_string(),
                    style: style_fg(bc),
                });
            }
        }
        hdr.push(StyledSpan {
            text: " \u{2502}".to_string(),
            style: style_fg(bc),
        });
        self.push_line(hdr, LineMeta::None);

        // Header separator
        let sep: String = col_widths
            .iter()
            .map(|w| "\u{2500}".repeat(*w))
            .collect::<Vec<_>>()
            .join("\u{2500}\u{253c}\u{2500}");
        self.push_line(
            vec![StyledSpan {
                text: format!("{}\u{251c}\u{2500}{}\u{2500}\u{2524}", indent, sep),
                style: style_fg(bc),
            }],
            LineMeta::None,
        );

        // Data rows
        for row in &rows {
            let mut spans = vec![StyledSpan {
                text: format!("{}\u{2502} ", indent),
                style: style_fg(bc),
            }];
            for (ci, cell) in row.iter().enumerate() {
                let fg = cell_color(cell, self.theme);
                spans.push(StyledSpan {
                    text: pad_or_truncate(cell, col_widths[ci]),
                    style: style_fg(fg),
                });
                if ci < row.len() - 1 {
                    spans.push(StyledSpan {
                        text: " \u{2502} ".to_string(),
                        style: style_fg(bc),
                    });
                }
            }
            spans.push(StyledSpan {
                text: " \u{2502}".to_string(),
                style: style_fg(bc),
            });
            self.push_line(spans, LineMeta::None);
        }

        // Bottom border
        let bot: String = col_widths
            .iter()
            .map(|w| "\u{2500}".repeat(*w))
            .collect::<Vec<_>>()
            .join("\u{2500}\u{2534}\u{2500}");
        self.push_line(
            vec![StyledSpan {
                text: format!("{}\u{2514}\u{2500}{}\u{2500}\u{2518}", indent, bot),
                style: style_fg(bc),
            }],
            LineMeta::None,
        );
    }

    // ── line emission helpers ─────────────────────────────────────

    fn emit_heading(&mut self, level: u8, text: &str) {
        self.emit_heading_with_annotation(level, text, "");
    }

    fn emit_heading_with_annotation(&mut self, level: u8, text: &str, annotation: &str) {
        let color = match level {
            1 => self.theme.h1,
            2 => self.theme.h2,
            3 => self.theme.h3,
            4 => self.theme.h4,
            5 => self.theme.h5,
            _ => self.theme.h6,
        };
        let prefix = match level {
            1 => "# ",
            2 => "## ",
            3 => "### ",
            4 => "#### ",
            5 => "##### ",
            _ => "###### ",
        };

        let mut spans = vec![
            StyledSpan {
                text: prefix.to_string(),
                style: Style {
                    fg: Some(self.theme.json_bracket),
                    dim: true,
                    ..Default::default()
                },
            },
            StyledSpan {
                text: text.to_string(),
                style: Style {
                    fg: Some(color),
                    bold: true,
                    ..Default::default()
                },
            },
        ];
        if !annotation.is_empty() {
            spans.push(StyledSpan {
                text: format!(" {}", annotation),
                style: Style {
                    fg: Some(self.theme.json_null),
                    dim: true,
                    ..Default::default()
                },
            });
        }

        self.push_line(
            spans,
            LineMeta::Heading {
                level,
                text: text.to_string(),
            },
        );

        if level <= 2 {
            let sep_w = self.width.min(60);
            self.push_line(
                vec![StyledSpan {
                    text: "\u{2500}".repeat(sep_w),
                    style: style_fg(self.theme.heading_separator),
                }],
                LineMeta::None,
            );
        }
    }

    /// Section label for nested objects/arrays (bold key with optional annotation).
    /// Registers as a heading for TOC navigation when depth is shallow enough.
    fn emit_section_label(&mut self, key: &str, depth: usize, annotation: &str) {
        let indent = indent_str(depth);
        let heading_level = if depth < 6 {
            Some((depth + 1) as u8)
        } else {
            None
        };

        let color = match heading_level {
            Some(2) => self.theme.h2,
            Some(3) => self.theme.h3,
            Some(4) => self.theme.h4,
            Some(5) => self.theme.h5,
            _ => self.theme.h6,
        };

        let meta = match heading_level {
            Some(level) => LineMeta::Heading {
                level,
                text: key.to_string(),
            },
            None => LineMeta::None,
        };

        let mut spans = vec![StyledSpan {
            text: format!("{}{}:", indent, key),
            style: Style {
                fg: Some(color),
                bold: true,
                ..Default::default()
            },
        }];
        if !annotation.is_empty() {
            spans.push(StyledSpan {
                text: format!(" {}", annotation),
                style: Style {
                    fg: Some(self.theme.json_null),
                    dim: true,
                    ..Default::default()
                },
            });
        }

        self.push_line(spans, meta);
    }

    fn emit_blank(&mut self) {
        self.push_line(vec![], LineMeta::None);
    }

    /// Key: value on a single indented line with alignment padding
    fn emit_kv(&mut self, key: &str, value: &Value, depth: usize, align_width: usize) {
        let indent = indent_str(depth);
        let key_w = UnicodeWidthStr::width(key);
        let padding = align_width.saturating_sub(key_w);

        let mut spans = vec![StyledSpan {
            text: format!("{}{}:{} ", indent, key, " ".repeat(padding)),
            style: Style {
                fg: Some(self.theme.json_key),
                bold: true,
                ..Default::default()
            },
        }];

        match value {
            Value::Object(m) if m.is_empty() => {
                spans.push(StyledSpan {
                    text: "{}".to_string(),
                    style: style_fg(self.theme.json_bracket),
                });
                spans.push(StyledSpan {
                    text: " empty".to_string(),
                    style: Style {
                        fg: Some(self.theme.json_null),
                        dim: true,
                        ..Default::default()
                    },
                });
            }
            Value::Array(a) if a.is_empty() => {
                spans.push(StyledSpan {
                    text: "[]".to_string(),
                    style: style_fg(self.theme.json_bracket),
                });
                spans.push(StyledSpan {
                    text: " empty".to_string(),
                    style: Style {
                        fg: Some(self.theme.json_null),
                        dim: true,
                        ..Default::default()
                    },
                });
            }
            _ => {
                spans.push(self.value_span(value));
            }
        }

        self.push_line(spans, LineMeta::None);
    }

    /// Indented value on its own line (for root primitives)
    fn emit_indented_value(&mut self, value: &Value, depth: usize) {
        let indent = indent_str(depth);
        let val = self.value_span(value);
        self.push_line(
            vec![
                StyledSpan {
                    text: indent,
                    style: Style::default(),
                },
                val,
            ],
            LineMeta::None,
        );
    }

    /// Bullet item for primitive arrays: "  \u{2022} value"
    fn emit_bullet(&mut self, value: &Value, depth: usize) {
        let indent = indent_str(depth);
        let val = self.value_span(value);
        self.push_line(
            vec![
                StyledSpan {
                    text: format!("{}\u{2022} ", indent),
                    style: Style {
                        fg: Some(self.theme.json_bracket),
                        dim: true,
                        ..Default::default()
                    },
                },
                val,
            ],
            LineMeta::None,
        );
    }

    /// Index label for complex array items: "  [N]"
    fn emit_index_label(&mut self, index: usize, depth: usize) {
        let indent = indent_str(depth);
        self.push_line(
            vec![StyledSpan {
                text: format!("{}[{}]", indent, index),
                style: style_fg(self.theme.json_bracket),
            }],
            LineMeta::None,
        );
    }

    /// Index label with annotation: "  [N] (M items)"
    fn emit_index_label_with_annotation(&mut self, index: usize, annotation: &str, depth: usize) {
        let indent = indent_str(depth);
        self.push_line(
            vec![
                StyledSpan {
                    text: format!("{}[{}] ", indent, index),
                    style: style_fg(self.theme.json_bracket),
                },
                StyledSpan {
                    text: annotation.to_string(),
                    style: Style {
                        fg: Some(self.theme.json_null),
                        dim: true,
                        ..Default::default()
                    },
                },
            ],
            LineMeta::None,
        );
    }

    /// Indexed primitive value in a mixed array: "  [N] value"
    fn emit_indexed_value(&mut self, index: usize, value: &Value, depth: usize) {
        let indent = indent_str(depth);
        let val = self.value_span(value);
        self.push_line(
            vec![
                StyledSpan {
                    text: format!("{}[{}] ", indent, index),
                    style: style_fg(self.theme.json_bracket),
                },
                val,
            ],
            LineMeta::None,
        );
    }

    // ── span helpers ──────────────────────────────────────────────

    fn value_span(&self, value: &Value) -> StyledSpan {
        match value {
            Value::String(s) => {
                let display = format!("\"{}\"", s);
                if s.starts_with("http://") || s.starts_with("https://") {
                    StyledSpan {
                        text: display,
                        style: Style {
                            fg: Some(self.theme.json_string),
                            underline: true,
                            link_url: Some(s.clone()),
                            ..Default::default()
                        },
                    }
                } else {
                    StyledSpan {
                        text: display,
                        style: style_fg(self.theme.json_string),
                    }
                }
            }
            Value::Number(n) => StyledSpan {
                text: n.to_string(),
                style: style_fg(self.theme.json_number),
            },
            Value::Bool(b) => StyledSpan {
                text: b.to_string(),
                style: style_fg(self.theme.json_bool),
            },
            Value::Null => StyledSpan {
                text: "null".to_string(),
                style: Style {
                    fg: Some(self.theme.json_null),
                    dim: true,
                    ..Default::default()
                },
            },
            Value::Object(m) if m.is_empty() => StyledSpan {
                text: "{}".to_string(),
                style: style_fg(self.theme.json_bracket),
            },
            Value::Array(a) if a.is_empty() => StyledSpan {
                text: "[]".to_string(),
                style: style_fg(self.theme.json_bracket),
            },
            Value::Object(_) => StyledSpan {
                text: "{\u{2026}}".to_string(),
                style: style_fg(self.theme.json_bracket),
            },
            Value::Array(_) => StyledSpan {
                text: "[\u{2026}]".to_string(),
                style: style_fg(self.theme.json_bracket),
            },
        }
    }

    fn push_line(&mut self, spans: Vec<StyledSpan>, meta: LineMeta) {
        self.lines.push(Line { spans, meta });
    }
}

// ── Interactive JSON explorer ──────────────────────────────────────

/// Navigable node in the interactive JSON view.
pub struct NavItem {
    pub line_index: usize,
    pub path: String,
}

/// State for the interactive JSON explorer.
pub struct JsonViewState {
    pub expanded: HashSet<String>,
    pub cursor: usize,
    pub navigable: Vec<NavItem>,
    /// Path of the cursor before a rebuild, used to restore position.
    pub cursor_path_save: Option<String>,
    /// When true, show a tree diagram instead of the card explorer.
    pub diagram_mode: bool,
}

impl JsonViewState {
    pub fn new() -> Self {
        Self {
            expanded: HashSet::new(),
            cursor: 0,
            navigable: Vec::new(),
            cursor_path_save: None,
            diagram_mode: false,
        }
    }

    /// Toggle expand/collapse for the node under the cursor.
    pub fn toggle_current(&mut self) {
        if let Some(nav) = self.navigable.get(self.cursor) {
            let path = nav.path.clone();
            self.cursor_path_save = Some(path.clone());
            if !self.expanded.remove(&path) {
                self.expanded.insert(path);
            }
        }
    }

    pub fn cursor_line(&self) -> Option<usize> {
        self.navigable.get(self.cursor).map(|n| n.line_index)
    }

    pub fn cursor_path(&self) -> Option<&str> {
        self.navigable.get(self.cursor).map(|n| n.path.as_str())
    }

    pub fn move_cursor(&mut self, delta: i32) {
        if self.navigable.is_empty() {
            return;
        }
        let new = (self.cursor as i32 + delta).clamp(0, self.navigable.len() as i32 - 1);
        self.cursor = new as usize;
    }

    /// After a rebuild, restore cursor to the same path (or clamp).
    pub fn restore_cursor(&mut self) {
        if let Some(ref saved) = self.cursor_path_save.take()
            && let Some(idx) = self.navigable.iter().position(|n| n.path == *saved)
        {
            self.cursor = idx;
            return;
        }
        if self.cursor >= self.navigable.len() {
            self.cursor = self.navigable.len().saturating_sub(1);
        }
    }

    /// Expand every expandable node in the entire document.
    pub fn expand_all(&mut self, json_src: &str) {
        if let Some(nav) = self.navigable.get(self.cursor) {
            self.cursor_path_save = Some(nav.path.clone());
        }
        if let Ok(root) = serde_json::from_str::<Value>(json_src) {
            let mut paths = Vec::new();
            collect_all_children(&root, "", &mut paths);
            for p in paths {
                self.expanded.insert(p);
            }
        }
    }

    /// Collapse every expanded node in the document.
    pub fn collapse_all(&mut self) {
        if let Some(nav) = self.navigable.get(self.cursor) {
            self.cursor_path_save = Some(nav.path.clone());
        }
        self.expanded.clear();
    }

    /// Format the current cursor path as a breadcrumb string (e.g., "data > users > [0]").
    pub fn breadcrumb(&self) -> Option<String> {
        let path = self.cursor_path()?;
        if path.is_empty() {
            return Some("root".to_string());
        }
        Some(format_breadcrumb(path))
    }
}

/// Render JSON interactively with expand/collapse bordered cards.
pub fn render_interactive(
    input: &str,
    width: usize,
    theme: &Theme,
    expanded: &HashSet<String>,
) -> Result<(Vec<Line>, DocumentInfo, Vec<NavItem>), String> {
    let value: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            let mut lines = Vec::new();
            render_parse_error(&mut lines, input, &e, theme, width);
            return Ok((
                lines,
                DocumentInfo {
                    code_blocks: Vec::new(),
                },
                Vec::new(),
            ));
        }
    };
    let mut r = CardRenderer {
        theme,
        lines: Vec::new(),
        width,
        expanded,
        navigable: Vec::new(),
        card_starts: Vec::new(),
        nesting: 0,
    };
    r.render_root(&value);
    Ok((
        r.lines,
        DocumentInfo {
            code_blocks: Vec::new(),
        },
        r.navigable,
    ))
}

struct CardRenderer<'a> {
    theme: &'a Theme,
    lines: Vec<Line>,
    width: usize,
    expanded: &'a HashSet<String>,
    navigable: Vec<NavItem>,
    card_starts: Vec<usize>,
    nesting: usize,
}

impl<'a> CardRenderer<'a> {
    // ── card borders ──────────────────────────────────────────

    fn card_width(&self, nesting: usize) -> usize {
        let base = self.width.saturating_sub(6);
        base.saturating_sub(nesting * 7).max(16)
    }

    fn open_card(&mut self) {
        let w = self.card_width(self.nesting);
        let bc = self.theme.json_bracket;
        // Top border at current nesting (before incrementing)
        self.push_line_raw(
            vec![StyledSpan {
                text: format!("  \u{256d}{}\u{256e}", "\u{2500}".repeat(w - 2)),
                style: style_fg(bc),
            }],
            LineMeta::None,
        );
        self.card_starts.push(self.lines.len());
        self.nesting += 1;
    }

    fn close_card(&mut self) {
        self.nesting -= 1;
        let start = self.card_starts.pop().unwrap_or(0);
        let w = self.card_width(self.nesting);
        let content_area = w.saturating_sub(4);
        let bc = self.theme.json_bracket;

        // Wrap content lines with side borders
        for i in start..self.lines.len() {
            let dw = self.lines[i].display_width();
            let padding = content_area.saturating_sub(dw);
            self.lines[i].spans.insert(
                0,
                StyledSpan {
                    text: "  \u{2502}  ".to_string(),
                    style: style_fg(bc),
                },
            );
            self.lines[i].spans.push(StyledSpan {
                text: format!("{}\u{2502}", " ".repeat(padding)),
                style: style_fg(bc),
            });
        }

        // Bottom border
        self.push_line_raw(
            vec![StyledSpan {
                text: format!("  \u{2570}{}\u{256f}", "\u{2500}".repeat(w - 2)),
                style: style_fg(bc),
            }],
            LineMeta::None,
        );
    }

    /// Push a line directly (not subject to card wrapping).
    fn push_line_raw(&mut self, spans: Vec<StyledSpan>, meta: LineMeta) {
        self.lines.push(Line { spans, meta });
    }

    /// Push a content line (will be wrapped by close_card).
    fn push_line(&mut self, spans: Vec<StyledSpan>, meta: LineMeta) {
        self.lines.push(Line { spans, meta });
    }

    // ── root rendering ────────────────────────────────────────

    fn render_root(&mut self, value: &Value) {
        match value {
            Value::Object(map) => {
                let (simple, sections) = group_entries(map);

                if !simple.is_empty() {
                    let align = compute_align_width(&simple);
                    for (key, val) in &simple {
                        self.emit_kv(key, val, 1, align);
                    }
                }

                for (i, (key, val)) in sections.iter().enumerate() {
                    if !simple.is_empty() || i > 0 {
                        self.emit_blank();
                    }
                    let path = key.to_string();
                    let summary = value_summary(val);
                    let is_expanded = self.expanded.contains(&path);
                    self.emit_toggle(key, &summary, is_expanded, 1, &path);

                    if is_expanded {
                        self.open_card();
                        self.render_value_content(val, &path);
                        self.close_card();
                    }
                }
            }
            Value::Array(arr) if !arr.is_empty() => {
                self.open_card();
                self.render_array_content(arr, "");
                self.close_card();
            }
            _ => {
                self.emit_indented_value(value, 1);
            }
        }
    }

    fn render_value_content(&mut self, value: &Value, path: &str) {
        match value {
            Value::Object(map) => self.render_object_content(map, path),
            Value::Array(arr) => self.render_array_content(arr, path),
            _ => {}
        }
    }

    fn render_object_content(&mut self, map: &serde_json::Map<String, Value>, parent_path: &str) {
        let (simple, sections) = group_entries(map);

        if !simple.is_empty() {
            let align = compute_align_width(&simple);
            for (key, val) in &simple {
                self.emit_kv(key, val, 0, align);
            }
        }

        for (i, (key, val)) in sections.iter().enumerate() {
            if !simple.is_empty() || i > 0 {
                self.emit_blank();
            }
            let child_path = format!("{}.{}", parent_path, key);
            let summary = value_summary(val);
            let is_expanded = self.expanded.contains(&child_path);
            self.emit_toggle(key, &summary, is_expanded, 0, &child_path);

            if is_expanded {
                self.open_card();
                self.render_value_content(val, &child_path);
                self.close_card();
            }
        }
    }

    fn render_array_content(&mut self, arr: &[Value], parent_path: &str) {
        if arr.is_empty() {
            return;
        }

        if should_render_as_table(arr) {
            self.render_table_inline(arr);
            return;
        }

        let all_prim = arr.iter().all(is_primitive_or_empty);

        if all_prim {
            for item in arr {
                self.emit_bullet(item, 0);
            }
        } else {
            let mut prev_complex = false;
            for (i, item) in arr.iter().enumerate() {
                let is_complex = !is_primitive_or_empty(item);
                if i > 0 && (is_complex || prev_complex) {
                    self.emit_blank();
                }
                let item_path = format!("{}[{}]", parent_path, i);

                match item {
                    Value::Object(map) if !map.is_empty() => {
                        let summary = format!("{} keys", map.len());
                        let is_expanded = self.expanded.contains(&item_path);
                        self.emit_toggle(&format!("[{}]", i), &summary, is_expanded, 0, &item_path);
                        if is_expanded {
                            self.open_card();
                            self.render_object_content(map, &item_path);
                            self.close_card();
                        }
                        prev_complex = true;
                    }
                    Value::Array(inner) if !inner.is_empty() => {
                        let summary = format!("{} items", inner.len());
                        let is_expanded = self.expanded.contains(&item_path);
                        self.emit_toggle(&format!("[{}]", i), &summary, is_expanded, 0, &item_path);
                        if is_expanded {
                            self.open_card();
                            self.render_array_content(inner, &item_path);
                            self.close_card();
                        }
                        prev_complex = true;
                    }
                    _ => {
                        self.emit_indexed_value(i, item, 0);
                        prev_complex = false;
                    }
                }
            }
        }
    }

    fn render_table_inline(&mut self, arr: &[Value]) {
        let objects: Vec<&serde_json::Map<String, Value>> =
            arr.iter().filter_map(|v| v.as_object()).collect();
        if objects.is_empty() {
            return;
        }

        let mut headers: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for obj in &objects {
            for key in obj.keys() {
                if seen.insert(key.clone()) {
                    headers.push(key.clone());
                }
            }
        }

        let rows: Vec<Vec<String>> = objects
            .iter()
            .map(|obj| {
                headers
                    .iter()
                    .map(|h| match obj.get(h) {
                        Some(v) => value_to_short_string(v),
                        None => String::new(),
                    })
                    .collect()
            })
            .collect();

        let available = self
            .card_width(self.nesting.saturating_sub(1))
            .saturating_sub(8);

        let mut col_widths: Vec<usize> = headers
            .iter()
            .enumerate()
            .map(|(ci, h)| {
                let hw = UnicodeWidthStr::width(h.as_str());
                let mc = rows
                    .iter()
                    .map(|r| UnicodeWidthStr::width(r[ci].as_str()))
                    .max()
                    .unwrap_or(0);
                hw.max(mc).max(3)
            })
            .collect();

        let seps = if headers.len() > 1 {
            (headers.len() - 1) * 3
        } else {
            0
        };
        let bc4 = 4;
        let total: usize = col_widths.iter().sum::<usize>() + seps + bc4;
        if total > available && available > bc4 + seps + headers.len() {
            let usable = available - bc4 - seps;
            let cur: usize = col_widths.iter().sum();
            for w in &mut col_widths {
                *w = (*w * usable / cur).max(3);
            }
        }

        let bc = self.theme.table_border;
        let hc = self.theme.table_header;

        let top: String = col_widths
            .iter()
            .map(|w| "\u{2500}".repeat(*w))
            .collect::<Vec<_>>()
            .join("\u{2500}\u{252c}\u{2500}");
        self.push_line(
            vec![StyledSpan {
                text: format!("\u{250c}\u{2500}{}\u{2500}\u{2510}", top),
                style: style_fg(bc),
            }],
            LineMeta::None,
        );

        let mut hdr = vec![StyledSpan {
            text: "\u{2502} ".to_string(),
            style: style_fg(bc),
        }];
        for (ci, h) in headers.iter().enumerate() {
            hdr.push(StyledSpan {
                text: pad_or_truncate(h, col_widths[ci]),
                style: Style {
                    fg: Some(hc),
                    bold: true,
                    ..Default::default()
                },
            });
            if ci < headers.len() - 1 {
                hdr.push(StyledSpan {
                    text: " \u{2502} ".to_string(),
                    style: style_fg(bc),
                });
            }
        }
        hdr.push(StyledSpan {
            text: " \u{2502}".to_string(),
            style: style_fg(bc),
        });
        self.push_line(hdr, LineMeta::None);

        let sep: String = col_widths
            .iter()
            .map(|w| "\u{2500}".repeat(*w))
            .collect::<Vec<_>>()
            .join("\u{2500}\u{253c}\u{2500}");
        self.push_line(
            vec![StyledSpan {
                text: format!("\u{251c}\u{2500}{}\u{2500}\u{2524}", sep),
                style: style_fg(bc),
            }],
            LineMeta::None,
        );

        for row in &rows {
            let mut spans = vec![StyledSpan {
                text: "\u{2502} ".to_string(),
                style: style_fg(bc),
            }];
            for (ci, cell) in row.iter().enumerate() {
                let fg = cell_color(cell, self.theme);
                spans.push(StyledSpan {
                    text: pad_or_truncate(cell, col_widths[ci]),
                    style: style_fg(fg),
                });
                if ci < row.len() - 1 {
                    spans.push(StyledSpan {
                        text: " \u{2502} ".to_string(),
                        style: style_fg(bc),
                    });
                }
            }
            spans.push(StyledSpan {
                text: " \u{2502}".to_string(),
                style: style_fg(bc),
            });
            self.push_line(spans, LineMeta::None);
        }

        let bot: String = col_widths
            .iter()
            .map(|w| "\u{2500}".repeat(*w))
            .collect::<Vec<_>>()
            .join("\u{2500}\u{2534}\u{2500}");
        self.push_line(
            vec![StyledSpan {
                text: format!("\u{2514}\u{2500}{}\u{2500}\u{2518}", bot),
                style: style_fg(bc),
            }],
            LineMeta::None,
        );
    }

    // ── emit helpers ──────────────────────────────────────────

    fn emit_toggle(
        &mut self,
        label: &str,
        summary: &str,
        expanded: bool,
        depth: usize,
        path: &str,
    ) {
        let indent = indent_str(depth);
        let arrow = if expanded { "\u{25bc}" } else { "\u{25b6}" };
        let arrow_color = if expanded {
            self.theme.h2
        } else {
            self.theme.json_bracket
        };

        let line_index = self.lines.len();
        self.navigable.push(NavItem {
            line_index,
            path: path.to_string(),
        });

        self.push_line(
            vec![
                StyledSpan {
                    text: format!("{}{} ", indent, arrow),
                    style: Style {
                        fg: Some(arrow_color),
                        bold: true,
                        ..Default::default()
                    },
                },
                StyledSpan {
                    text: label.to_string(),
                    style: Style {
                        fg: Some(self.theme.json_key),
                        bold: true,
                        ..Default::default()
                    },
                },
                StyledSpan {
                    text: format!("  {}", summary),
                    style: Style {
                        fg: Some(self.theme.json_null),
                        dim: true,
                        ..Default::default()
                    },
                },
            ],
            LineMeta::None,
        );
    }

    fn emit_kv(&mut self, key: &str, value: &Value, depth: usize, align: usize) {
        let indent = indent_str(depth);
        let key_w = UnicodeWidthStr::width(key);
        let padding = align.saturating_sub(key_w);

        let mut spans = vec![StyledSpan {
            text: format!("{}{}:{} ", indent, key, " ".repeat(padding)),
            style: Style {
                fg: Some(self.theme.json_key),
                bold: true,
                ..Default::default()
            },
        }];

        match value {
            Value::Object(m) if m.is_empty() => {
                spans.push(StyledSpan {
                    text: "{}".to_string(),
                    style: style_fg(self.theme.json_bracket),
                });
                spans.push(StyledSpan {
                    text: " empty".to_string(),
                    style: Style {
                        fg: Some(self.theme.json_null),
                        dim: true,
                        ..Default::default()
                    },
                });
            }
            Value::Array(a) if a.is_empty() => {
                spans.push(StyledSpan {
                    text: "[]".to_string(),
                    style: style_fg(self.theme.json_bracket),
                });
                spans.push(StyledSpan {
                    text: " empty".to_string(),
                    style: Style {
                        fg: Some(self.theme.json_null),
                        dim: true,
                        ..Default::default()
                    },
                });
            }
            _ => {
                spans.push(self.value_span(value));
            }
        }
        self.push_line(spans, LineMeta::None);
    }

    fn emit_bullet(&mut self, value: &Value, depth: usize) {
        let indent = indent_str(depth);
        let val = self.value_span(value);
        self.push_line(
            vec![
                StyledSpan {
                    text: format!("{}\u{2022} ", indent),
                    style: Style {
                        fg: Some(self.theme.json_bracket),
                        dim: true,
                        ..Default::default()
                    },
                },
                val,
            ],
            LineMeta::None,
        );
    }

    fn emit_indexed_value(&mut self, index: usize, value: &Value, depth: usize) {
        let indent = indent_str(depth);
        let val = self.value_span(value);
        self.push_line(
            vec![
                StyledSpan {
                    text: format!("{}[{}] ", indent, index),
                    style: style_fg(self.theme.json_bracket),
                },
                val,
            ],
            LineMeta::None,
        );
    }

    fn emit_indented_value(&mut self, value: &Value, depth: usize) {
        let indent = indent_str(depth);
        let val = self.value_span(value);
        self.push_line(
            vec![
                StyledSpan {
                    text: indent,
                    style: Style::default(),
                },
                val,
            ],
            LineMeta::None,
        );
    }

    fn emit_blank(&mut self) {
        self.push_line(vec![], LineMeta::None);
    }

    fn value_span(&self, value: &Value) -> StyledSpan {
        match value {
            Value::String(s) => {
                let display = format!("\"{}\"", s);
                if s.starts_with("http://") || s.starts_with("https://") {
                    StyledSpan {
                        text: display,
                        style: Style {
                            fg: Some(self.theme.json_string),
                            underline: true,
                            link_url: Some(s.clone()),
                            ..Default::default()
                        },
                    }
                } else {
                    StyledSpan {
                        text: display,
                        style: style_fg(self.theme.json_string),
                    }
                }
            }
            Value::Number(n) => StyledSpan {
                text: n.to_string(),
                style: style_fg(self.theme.json_number),
            },
            Value::Bool(b) => StyledSpan {
                text: b.to_string(),
                style: style_fg(self.theme.json_bool),
            },
            Value::Null => StyledSpan {
                text: "null".to_string(),
                style: Style {
                    fg: Some(self.theme.json_null),
                    dim: true,
                    ..Default::default()
                },
            },
            _ => StyledSpan {
                text: String::new(),
                style: Style::default(),
            },
        }
    }
}

fn group_entries(
    map: &serde_json::Map<String, Value>,
) -> (Vec<(&String, &Value)>, Vec<(&String, &Value)>) {
    let mut simple = Vec::new();
    let mut sections = Vec::new();
    for (key, val) in map {
        if is_primitive_or_empty(val) {
            simple.push((key, val));
        } else {
            sections.push((key, val));
        }
    }
    (simple, sections)
}

fn compute_align_width(entries: &[(&String, &Value)]) -> usize {
    entries
        .iter()
        .map(|(k, _)| UnicodeWidthStr::width(k.as_str()))
        .max()
        .unwrap_or(0)
        .min(MAX_ALIGN_WIDTH)
}

fn value_summary(value: &Value) -> String {
    match value {
        Value::Object(m) => format!("{} keys", m.len()),
        Value::Array(a) => format!("{} items", a.len()),
        _ => String::new(),
    }
}

// ── free helpers ──────────────────────────────────────────────────

fn indent_str(depth: usize) -> String {
    "  ".repeat(depth)
}

fn is_primitive_or_empty(v: &Value) -> bool {
    matches!(
        v,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    ) || matches!(v, Value::Object(m) if m.is_empty())
        || matches!(v, Value::Array(a) if a.is_empty())
}

fn style_fg(color: Color) -> Style {
    Style {
        fg: Some(color),
        ..Default::default()
    }
}

fn should_render_as_table(arr: &[Value]) -> bool {
    if arr.len() < 2 {
        return false;
    }
    let objects: Vec<&serde_json::Map<String, Value>> =
        arr.iter().filter_map(|v| v.as_object()).collect();
    if objects.len() != arr.len() {
        return false;
    }
    for obj in &objects {
        for val in obj.values() {
            if val.is_object() || val.is_array() {
                return false;
            }
        }
    }
    let all_keys: HashSet<&str> = objects
        .iter()
        .flat_map(|o| o.keys().map(|k| k.as_str()))
        .collect();
    if all_keys.is_empty() {
        return false;
    }
    objects.iter().all(|o| {
        let shared = o.keys().filter(|k| all_keys.contains(k.as_str())).count();
        shared * 2 >= all_keys.len()
    })
}

fn value_to_short_string(v: &Value) -> String {
    match v {
        Value::String(s) => {
            if s.len() > 40 {
                format!("{}\u{2026}", &s[..39])
            } else {
                s.clone()
            }
        }
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Object(_) => "{\u{2026}}".to_string(),
        Value::Array(_) => "[\u{2026}]".to_string(),
    }
}

fn pad_or_truncate(s: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w > width {
        let mut result = String::new();
        let mut current_w = 0;
        for ch in s.chars() {
            let cw = UnicodeWidthStr::width(ch.to_string().as_str());
            if current_w + cw > width.saturating_sub(1) {
                break;
            }
            result.push(ch);
            current_w += cw;
        }
        result.push('\u{2026}');
        let final_w = UnicodeWidthStr::width(result.as_str());
        for _ in final_w..width {
            result.push(' ');
        }
        result
    } else {
        let mut result = s.to_string();
        for _ in w..width {
            result.push(' ');
        }
        result
    }
}

fn cell_color(text: &str, theme: &Theme) -> Color {
    if text == "null" {
        theme.json_null
    } else if text == "true" || text == "false" {
        theme.json_bool
    } else if text.parse::<f64>().is_ok() && !text.is_empty() {
        theme.json_number
    } else {
        theme.json_string
    }
}

/// Walk all nested objects/arrays and collect their paths.
fn collect_all_children(val: &Value, prefix: &str, out: &mut Vec<String>) {
    match val {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{}.{}", prefix, key)
                };
                if !is_primitive_or_empty(child) {
                    out.push(child_path.clone());
                    collect_all_children(child, &child_path, out);
                }
            }
        }
        Value::Array(arr) => {
            for (i, child) in arr.iter().enumerate() {
                let child_path = format!("{}[{}]", prefix, i);
                if !is_primitive_or_empty(child) {
                    out.push(child_path.clone());
                    collect_all_children(child, &child_path, out);
                }
            }
        }
        _ => {}
    }
}

enum PathSegment {
    Key(String),
    Index(usize),
}

fn parse_path_segments(path: &str) -> Vec<PathSegment> {
    let mut segments = Vec::new();
    let mut rest = path;
    while !rest.is_empty() {
        if rest.starts_with('[') {
            // Array index: [N]
            if let Some(end) = rest.find(']') {
                if let Ok(idx) = rest[1..end].parse::<usize>() {
                    segments.push(PathSegment::Index(idx));
                }
                rest = &rest[end + 1..];
                if rest.starts_with('.') {
                    rest = &rest[1..];
                }
            } else {
                break;
            }
        } else {
            // Key: up to next '.' or '['
            let end = rest.find(['.', '[']).unwrap_or(rest.len());
            segments.push(PathSegment::Key(rest[..end].to_string()));
            rest = &rest[end..];
            if rest.starts_with('.') {
                rest = &rest[1..];
            }
        }
    }
    segments
}

/// Format a path string as a breadcrumb: "data.users[0]" → "data > users > [0]"
fn format_breadcrumb(path: &str) -> String {
    let segments = parse_path_segments(path);
    let parts: Vec<String> = segments
        .iter()
        .map(|s| match s {
            PathSegment::Key(k) => k.clone(),
            PathSegment::Index(i) => format!("[{}]", i),
        })
        .collect();
    parts.join(" > ")
}

// ── Diagram view ──────────────────────────────────────────────────

/// Maximum tree depth rendered in the diagram.
const DIAGRAM_MAX_DEPTH: usize = 3;
/// Maximum children shown per node before truncating with "+N more".
const DIAGRAM_MAX_SIBLINGS: usize = 8;

/// A node in the JSON tree for diagram rendering.
struct DiagramNode {
    id: String,
    label: String,
    shape: crate::diagram::NodeShape,
    children: Vec<String>, // child node ids
}

/// Build diagram nodes from a JSON value, walking recursively up to max_depth.
fn build_diagram_nodes(
    value: &Value,
    node_id: &str,
    label: &str,
    depth: usize,
    nodes: &mut Vec<DiagramNode>,
) {
    use crate::diagram::NodeShape;

    let (shape, children_iter): (NodeShape, Vec<(&str, &Value, String)>) = match value {
        Value::Object(map) => {
            let kids: Vec<_> = map
                .iter()
                .map(|(k, v)| (k.as_str(), v, format!("{}.{}", node_id, k)))
                .collect();
            (NodeShape::Rounded, kids)
        }
        Value::Array(arr) => {
            let kids: Vec<_> = arr
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let lbl: &str = ""; // placeholder, real label built below
                    let _ = lbl;
                    (lbl, v, format!("{}[{}]", node_id, i))
                })
                .collect();
            (NodeShape::Rectangle, kids)
        }
        _ => {
            // Leaf node
            nodes.push(DiagramNode {
                id: node_id.to_string(),
                label: label.to_string(),
                shape: NodeShape::Rectangle,
                children: Vec::new(),
            });
            return;
        }
    };

    let mut child_ids = Vec::new();
    let total = children_iter.len();
    let truncated = total > DIAGRAM_MAX_SIBLINGS;
    let show_count = if truncated {
        DIAGRAM_MAX_SIBLINGS
    } else {
        total
    };

    for (i, (_key, child_val, child_id)) in children_iter.iter().enumerate() {
        if i >= show_count {
            break;
        }
        let child_label = match value {
            Value::Object(_) => {
                // Extract the key name from the child_id
                let key = child_id.rsplit('.').next().unwrap_or(child_id);
                match child_val {
                    Value::Object(m) if !m.is_empty() && depth + 1 < DIAGRAM_MAX_DEPTH => {
                        key.to_string()
                    }
                    Value::Object(m) if !m.is_empty() => {
                        format!("{} {{{}}}", key, m.len())
                    }
                    Value::Array(a) if !a.is_empty() && depth + 1 < DIAGRAM_MAX_DEPTH => {
                        key.to_string()
                    }
                    Value::Array(a) if !a.is_empty() => {
                        format!("{} [{}]", key, a.len())
                    }
                    _ => {
                        let val_str = format_primitive_short(child_val);
                        format!("{}: {}", key, val_str)
                    }
                }
            }
            Value::Array(_) => match child_val {
                Value::Object(m) if !m.is_empty() && depth + 1 < DIAGRAM_MAX_DEPTH => {
                    format!("[{}]", i)
                }
                Value::Object(m) if !m.is_empty() => {
                    format!("[{}] {{{}}}", i, m.len())
                }
                Value::Array(a) if !a.is_empty() && depth + 1 < DIAGRAM_MAX_DEPTH => {
                    format!("[{}]", i)
                }
                Value::Array(a) if !a.is_empty() => {
                    format!("[{}] [{}]", i, a.len())
                }
                _ => {
                    let val_str = format_primitive_short(child_val);
                    format!("[{}]: {}", i, val_str)
                }
            },
            _ => unreachable!(),
        };

        child_ids.push(child_id.clone());

        if depth + 1 < DIAGRAM_MAX_DEPTH && !is_primitive_or_empty(child_val) {
            build_diagram_nodes(child_val, child_id, &child_label, depth + 1, nodes);
        } else {
            let child_shape = match child_val {
                Value::Object(_) => NodeShape::Rounded,
                Value::Array(_) => NodeShape::Rectangle,
                _ => NodeShape::Rectangle,
            };
            nodes.push(DiagramNode {
                id: child_id.clone(),
                label: child_label,
                shape: child_shape,
                children: Vec::new(),
            });
        }
    }

    if truncated {
        let more_id = format!("{}_more", node_id);
        let more_label = format!("+{} more", total - show_count);
        child_ids.push(more_id.clone());
        nodes.push(DiagramNode {
            id: more_id,
            label: more_label,
            shape: NodeShape::Rectangle,
            children: Vec::new(),
        });
    }

    nodes.push(DiagramNode {
        id: node_id.to_string(),
        label: label.to_string(),
        shape,
        children: child_ids,
    });
}

fn format_primitive_short(val: &Value) -> String {
    match val {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            if s.len() > 16 {
                format!("\"{}…\"", &s[..14])
            } else {
                format!("\"{}\"", s)
            }
        }
        Value::Object(m) if m.is_empty() => "{}".to_string(),
        Value::Array(a) if a.is_empty() => "[]".to_string(),
        _ => "…".to_string(),
    }
}

/// Render JSON as a top-down tree diagram using the Canvas from diagram.rs.
pub fn render_diagram(
    input: &str,
    width: usize,
    theme: &Theme,
) -> Result<(Vec<Line>, DocumentInfo), String> {
    use crate::diagram::{Canvas, NodeLayout, NodeShape, label_box_width};

    let value: Value =
        serde_json::from_str(input).map_err(|e| format!("JSON parse error: {}", e))?;

    // Build the root label
    let root_label = match &value {
        Value::Object(m) => format!("root {{{}}}", m.len()),
        Value::Array(a) => format!("root [{}]", a.len()),
        _ => "root".to_string(),
    };

    let mut all_nodes = Vec::new();
    build_diagram_nodes(&value, "root", &root_label, 0, &mut all_nodes);

    if all_nodes.is_empty() {
        return Ok((
            Vec::new(),
            DocumentInfo {
                code_blocks: Vec::new(),
            },
        ));
    }

    // Build lookup: id → index
    let id_to_idx: std::collections::HashMap<String, usize> = all_nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    // Assign layers via BFS from root
    let mut layers: Vec<Vec<usize>> = Vec::new();
    let mut visited = HashSet::new();
    if let Some(&root_idx) = id_to_idx.get("root") {
        let mut queue = std::collections::VecDeque::new();
        queue.push_back((root_idx, 0usize));
        visited.insert(root_idx);
        while let Some((idx, depth)) = queue.pop_front() {
            while layers.len() <= depth {
                layers.push(Vec::new());
            }
            layers[depth].push(idx);
            for child_id in &all_nodes[idx].children {
                if let Some(&child_idx) = id_to_idx.get(child_id)
                    && visited.insert(child_idx)
                {
                    queue.push_back((child_idx, depth + 1));
                }
            }
        }
    }

    if layers.is_empty() {
        return Ok((
            Vec::new(),
            DocumentInfo {
                code_blocks: Vec::new(),
            },
        ));
    }

    // Calculate node widths
    let mut node_widths: Vec<usize> = all_nodes
        .iter()
        .map(|n| label_box_width(&n.label, n.shape))
        .collect();

    let node_height: usize = 3;
    let edge_gap: usize = 4;
    let h_gap: usize = 4;

    // Trim layers to fit within the available terminal width
    let max_content_width = width.saturating_sub(10).max(40);
    let mut removed_ids: HashSet<String> = HashSet::new();

    #[allow(clippy::needless_range_loop)]
    for layer_idx in 0..layers.len() {
        // Exclude nodes whose parents were removed (cascade)
        if layer_idx > 0 {
            let mut new_layer = Vec::new();
            for &idx in &layers[layer_idx] {
                let node_id = &all_nodes[idx].id;
                let parent_removed = all_nodes
                    .iter()
                    .any(|pn| pn.children.contains(node_id) && removed_ids.contains(&pn.id));
                if parent_removed {
                    removed_ids.insert(node_id.clone());
                } else {
                    new_layer.push(idx);
                }
            }
            layers[layer_idx] = new_layer;
        }

        // If this layer is too wide, truncate and add "+N more"
        let layer = &mut layers[layer_idx];
        let total_w: usize = layer.iter().map(|&i| node_widths[i]).sum::<usize>()
            + layer.len().saturating_sub(1) * h_gap;

        if total_w > max_content_width && layer.len() > 1 {
            let more_placeholder_w = label_box_width("+99 more", NodeShape::Rectangle);
            let target = max_content_width.saturating_sub(more_placeholder_w + h_gap);

            let mut keep = 0;
            let mut running_w: usize = 0;
            for &idx in layer.iter() {
                let nw = node_widths[idx];
                let next_w = running_w + nw + if keep > 0 { h_gap } else { 0 };
                if next_w > target && keep > 0 {
                    break;
                }
                running_w = next_w;
                keep += 1;
            }
            keep = keep.max(1);

            let removed_count = layer.len() - keep;
            for &idx in &layer[keep..] {
                removed_ids.insert(all_nodes[idx].id.clone());
            }
            layer.truncate(keep);

            // Add a "+N more" summary node
            let more_label = format!("+{} more", removed_count);
            let more_shape = NodeShape::Rectangle;
            let more_idx = all_nodes.len();
            all_nodes.push(DiagramNode {
                id: format!("_more_{}", layer_idx),
                label: more_label.clone(),
                shape: more_shape,
                children: Vec::new(),
            });
            node_widths.push(label_box_width(&more_label, more_shape));
            layer.push(more_idx);
        }
    }

    // Remove empty trailing layers
    while layers.last().is_some_and(|l| l.is_empty()) {
        layers.pop();
    }

    if layers.is_empty() {
        return Ok((
            Vec::new(),
            DocumentInfo {
                code_blocks: Vec::new(),
            },
        ));
    }

    // Find widest layer
    let mut max_layer_width: usize = 0;
    for layer in &layers {
        let w: usize = layer.iter().map(|&i| node_widths[i]).sum::<usize>()
            + layer.len().saturating_sub(1) * h_gap;
        max_layer_width = max_layer_width.max(w);
    }

    let canvas_width = (max_layer_width + 6).max(20);
    let canvas_height = layers.len() * (node_height + edge_gap) - edge_gap;

    let mut canvas = Canvas::new(canvas_width, canvas_height);
    let mut positions: Vec<Option<NodeLayout>> = vec![None; all_nodes.len()];

    let border_fg = Some(theme.code_border);
    let text_fg = Some(theme.fg);
    let canvas_center = canvas_width / 2;

    // Draw nodes layer by layer
    for (layer_idx, layer) in layers.iter().enumerate() {
        let y = layer_idx * (node_height + edge_gap);
        let widths_in_layer: Vec<usize> = layer.iter().map(|&i| node_widths[i]).collect();
        let layer_width: usize =
            widths_in_layer.iter().sum::<usize>() + layer.len().saturating_sub(1) * h_gap;

        let mut centers: Vec<usize> = Vec::new();
        let mut cum = 0;
        for &w in &widths_in_layer {
            centers.push(cum + w / 2);
            cum += w + h_gap;
        }
        let layer_center = if layer_width > 0 { layer_width / 2 } else { 0 };

        for (i, &idx) in layer.iter().enumerate() {
            let w = widths_in_layer[i];
            let cx = (canvas_center as isize + centers[i] as isize - layer_center as isize)
                .max(w as isize / 2) as usize;

            // Pick color based on node type
            let node_fg = match all_nodes[idx].shape {
                NodeShape::Rounded => Some(theme.json_key),
                _ => text_fg,
            };

            canvas.draw_node(
                cx,
                y,
                w,
                &all_nodes[idx].label,
                all_nodes[idx].shape,
                border_fg,
                node_fg,
            );

            positions[idx] = Some(NodeLayout {
                center_x: cx,
                top_y: y,
                width: w,
            });
        }
    }

    // Build a fresh id→index map (includes "+more" nodes added during trimming)
    let full_id_map: std::collections::HashMap<&str, usize> = all_nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    // Draw edges
    let edge_fg = Some(theme.code_border);
    for (idx, node) in all_nodes.iter().enumerate() {
        if let Some(ref src_pos) = positions[idx] {
            for child_id in &node.children {
                if let Some(&child_idx) = full_id_map.get(child_id.as_str())
                    && let Some(ref dst_pos) = positions[child_idx]
                {
                    let src_bottom = src_pos.top_y + 2;
                    let dst_top = dst_pos.top_y;
                    canvas.draw_edge_td(
                        src_pos.center_x,
                        src_bottom,
                        dst_pos.center_x,
                        dst_top,
                        None,
                        edge_fg,
                        None,
                    );
                }
            }
        }
    }

    // Convert canvas to styled span rows
    let rows = canvas.to_span_rows(theme);

    // Wrap in a bordered block (like emit_diagram_block in markdown.rs)
    let block_label = "JSON (tree)";
    let block_width = width.saturating_sub(4).max(20);
    let bc = theme.code_border;
    let bg = theme.code_bg;
    let mut lines = Vec::new();

    // Top border
    let label_with_pad = format!(" {} ", block_label);
    let label_len = label_with_pad.chars().count();
    let fill = block_width.saturating_sub(2 + label_len);
    lines.push(Line {
        spans: vec![
            StyledSpan {
                text: "  \u{256d}\u{2500}".to_string(),
                style: Style {
                    fg: Some(bc),
                    bg: Some(bg),
                    ..Default::default()
                },
            },
            StyledSpan {
                text: label_with_pad,
                style: Style {
                    fg: Some(theme.code_label),
                    bg: Some(bg),
                    ..Default::default()
                },
            },
            StyledSpan {
                text: format!("{}\u{256e}", "\u{2500}".repeat(fill)),
                style: Style {
                    fg: Some(bc),
                    bg: Some(bg),
                    ..Default::default()
                },
            },
        ],
        meta: LineMeta::None,
    });

    // Empty line after header
    lines.push(Line {
        spans: vec![StyledSpan {
            text: format!("  \u{2502}{}\u{2502}", " ".repeat(block_width - 2)),
            style: Style {
                fg: Some(bc),
                bg: Some(bg),
                ..Default::default()
            },
        }],
        meta: LineMeta::None,
    });

    // Content rows
    for row_spans in &rows {
        let row_text: String = row_spans.iter().map(|s| s.text.as_str()).collect();
        let row_width = UnicodeWidthStr::width(row_text.as_str());
        let padding = block_width.saturating_sub(4 + row_width);

        let mut spans = Vec::new();
        spans.push(StyledSpan {
            text: "  \u{2502} ".to_string(),
            style: Style {
                fg: Some(bc),
                bg: Some(bg),
                ..Default::default()
            },
        });
        for s in row_spans {
            spans.push(StyledSpan {
                text: s.text.clone(),
                style: Style {
                    bg: Some(bg),
                    ..s.style.clone()
                },
            });
        }
        spans.push(StyledSpan {
            text: format!("{} \u{2502}", " ".repeat(padding)),
            style: Style {
                fg: Some(bc),
                bg: Some(bg),
                ..Default::default()
            },
        });
        lines.push(Line {
            spans,
            meta: LineMeta::None,
        });
    }

    // Empty line before footer
    lines.push(Line {
        spans: vec![StyledSpan {
            text: format!("  \u{2502}{}\u{2502}", " ".repeat(block_width - 2)),
            style: Style {
                fg: Some(bc),
                bg: Some(bg),
                ..Default::default()
            },
        }],
        meta: LineMeta::None,
    });

    // Bottom border
    lines.push(Line {
        spans: vec![StyledSpan {
            text: format!("  \u{2570}{}\u{256f}", "\u{2500}".repeat(block_width - 2)),
            style: Style {
                fg: Some(bc),
                bg: Some(bg),
                ..Default::default()
            },
        }],
        meta: LineMeta::None,
    });

    Ok((
        lines,
        DocumentInfo {
            code_blocks: Vec::new(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_all_expands_entire_document() {
        let json = r#"{"a":1,"nested":{"b":2,"deep":{"c":3}}}"#;
        let theme = crate::theme::Theme::dark();

        let (_, _, nav) = render_interactive(json, 80, &theme, &HashSet::new()).unwrap();
        let mut state = JsonViewState::new();
        state.navigable = nav;
        state.expand_all(json);

        // Should expand everything in the document, not just cursor's subtree
        assert!(state.expanded.contains("nested"));
        assert!(state.expanded.contains("nested.deep"));
    }

    #[test]
    fn expand_all_with_arrays() {
        let json = r#"{"items":[{"id":1,"sub":{"x":true}},{"id":2}]}"#;
        let theme = crate::theme::Theme::dark();

        let (_, _, nav) = render_interactive(json, 80, &theme, &HashSet::new()).unwrap();
        let mut state = JsonViewState::new();
        state.navigable = nav;
        state.expand_all(json);

        assert!(state.expanded.contains("items"));
        assert!(state.expanded.contains("items[0]"));
        assert!(state.expanded.contains("items[0].sub"));
        assert!(state.expanded.contains("items[1]"));
    }

    #[test]
    fn collapse_all_clears_everything() {
        let json = r#"{"a":1,"nested":{"b":2,"deep":{"c":3}}}"#;
        let theme = crate::theme::Theme::dark();

        let (_, _, nav) = render_interactive(json, 80, &theme, &HashSet::new()).unwrap();
        let mut state = JsonViewState::new();
        state.navigable = nav;
        state.expand_all(json);
        assert!(!state.expanded.is_empty());

        // Re-render with expanded state
        let (_, _, nav2) = render_interactive(json, 80, &theme, &state.expanded).unwrap();
        state.navigable = nav2;
        state.restore_cursor();

        state.collapse_all();
        assert!(state.expanded.is_empty());
    }

    #[test]
    fn breadcrumb_formats_path() {
        assert_eq!(format_breadcrumb("config"), "config");
        assert_eq!(format_breadcrumb("config.theme"), "config > theme");
        assert_eq!(
            format_breadcrumb("config.theme.colors"),
            "config > theme > colors"
        );
        assert_eq!(format_breadcrumb("items[0]"), "items > [0]");
        assert_eq!(format_breadcrumb("items[0].name"), "items > [0] > name");
    }
}
