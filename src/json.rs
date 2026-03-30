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
    let value: Value =
        serde_json::from_str(input).map_err(|e| format!("JSON parse error: {}", e))?;
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

// ── Shared span/line builders ─────────────────────────────────────

/// Build a styled span for a JSON value.
fn make_value_span(theme: &Theme, value: &Value) -> StyledSpan {
    match value {
        Value::String(s) => {
            let display = format!("\"{}\"", s);
            if s.starts_with("http://") || s.starts_with("https://") {
                StyledSpan {
                    text: display,
                    style: Style {
                        fg: Some(theme.json_string),
                        underline: true,
                        link_url: Some(s.clone()),
                        ..Default::default()
                    },
                }
            } else {
                StyledSpan {
                    text: display,
                    style: style_fg(theme.json_string),
                }
            }
        }
        Value::Number(n) => StyledSpan {
            text: n.to_string(),
            style: style_fg(theme.json_number),
        },
        Value::Bool(b) => StyledSpan {
            text: b.to_string(),
            style: style_fg(theme.json_bool),
        },
        Value::Null => StyledSpan {
            text: "null".to_string(),
            style: Style {
                fg: Some(theme.json_null),
                dim: true,
                ..Default::default()
            },
        },
        Value::Object(m) if m.is_empty() => StyledSpan {
            text: "{}".to_string(),
            style: style_fg(theme.json_bracket),
        },
        Value::Array(a) if a.is_empty() => StyledSpan {
            text: "[]".to_string(),
            style: style_fg(theme.json_bracket),
        },
        Value::Object(_) => StyledSpan {
            text: "{…}".to_string(),
            style: style_fg(theme.json_bracket),
        },
        Value::Array(_) => StyledSpan {
            text: "[…]".to_string(),
            style: style_fg(theme.json_bracket),
        },
    }
}

/// Build spans for a "key: value" line with alignment padding.
fn make_kv_line(theme: &Theme, key: &str, value: &Value, depth: usize, align_width: usize) -> Line {
    let indent = indent_str(depth);
    let key_w = UnicodeWidthStr::width(key);
    let padding = align_width.saturating_sub(key_w);

    let mut spans = vec![StyledSpan {
        text: format!("{}{}:{} ", indent, key, " ".repeat(padding)),
        style: Style {
            fg: Some(theme.json_key),
            bold: true,
            ..Default::default()
        },
    }];

    match value {
        Value::Object(m) if m.is_empty() => {
            spans.push(StyledSpan {
                text: "{}".to_string(),
                style: style_fg(theme.json_bracket),
            });
            spans.push(StyledSpan {
                text: " empty".to_string(),
                style: Style {
                    fg: Some(theme.json_null),
                    dim: true,
                    ..Default::default()
                },
            });
        }
        Value::Array(a) if a.is_empty() => {
            spans.push(StyledSpan {
                text: "[]".to_string(),
                style: style_fg(theme.json_bracket),
            });
            spans.push(StyledSpan {
                text: " empty".to_string(),
                style: Style {
                    fg: Some(theme.json_null),
                    dim: true,
                    ..Default::default()
                },
            });
        }
        _ => {
            spans.push(make_value_span(theme, value));
        }
    }

    Line {
        spans,
        meta: LineMeta::None,
    }
}

/// Build a bullet line: "  • value"
fn make_bullet_line(theme: &Theme, value: &Value, depth: usize) -> Line {
    let indent = indent_str(depth);
    Line {
        spans: vec![
            StyledSpan {
                text: format!("{}\u{2022} ", indent),
                style: Style {
                    fg: Some(theme.json_bracket),
                    dim: true,
                    ..Default::default()
                },
            },
            make_value_span(theme, value),
        ],
        meta: LineMeta::None,
    }
}

/// Build an indexed value line: "  [N] value"
fn make_indexed_value_line(theme: &Theme, index: usize, value: &Value, depth: usize) -> Line {
    let indent = indent_str(depth);
    Line {
        spans: vec![
            StyledSpan {
                text: format!("{}[{}] ", indent, index),
                style: style_fg(theme.json_bracket),
            },
            make_value_span(theme, value),
        ],
        meta: LineMeta::None,
    }
}

/// Build an indented value line (for root primitives).
fn make_indented_value_line(theme: &Theme, value: &Value, depth: usize) -> Line {
    let indent = indent_str(depth);
    Line {
        spans: vec![
            StyledSpan {
                text: indent,
                style: Style::default(),
            },
            make_value_span(theme, value),
        ],
        meta: LineMeta::None,
    }
}

/// Build table lines from an array of homogeneous objects.
fn build_table_lines(theme: &Theme, arr: &[Value], indent: &str, available: usize) -> Vec<Line> {
    let objects: Vec<&serde_json::Map<String, Value>> =
        arr.iter().filter_map(|v| v.as_object()).collect();
    if objects.is_empty() {
        return Vec::new();
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

    // Build cell text + type matrix (preserve type for correct coloring)
    let rows: Vec<Vec<(String, CellType)>> = objects
        .iter()
        .map(|obj| {
            headers
                .iter()
                .map(|h| match obj.get(h) {
                    Some(v) => (value_to_short_string(v), CellType::from_value(v)),
                    None => (String::new(), CellType::String),
                })
                .collect()
        })
        .collect();

    // Compute column widths
    let mut col_widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(ci, h)| {
            let header_w = UnicodeWidthStr::width(h.as_str());
            let max_cell = rows
                .iter()
                .map(|r| UnicodeWidthStr::width(r[ci].0.as_str()))
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

    let bc = theme.table_border;
    let hc = theme.table_header;
    let mut lines = Vec::new();

    // Top border
    let top: String = col_widths
        .iter()
        .map(|w| "─".repeat(*w))
        .collect::<Vec<_>>()
        .join("─┬─");
    lines.push(Line {
        spans: vec![StyledSpan {
            text: format!("{}┌─{}─┐", indent, top),
            style: style_fg(bc),
        }],
        meta: LineMeta::None,
    });

    // Header row
    let mut hdr = vec![StyledSpan {
        text: format!("{}│ ", indent),
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
                text: " │ ".to_string(),
                style: style_fg(bc),
            });
        }
    }
    hdr.push(StyledSpan {
        text: " │".to_string(),
        style: style_fg(bc),
    });
    lines.push(Line {
        spans: hdr,
        meta: LineMeta::None,
    });

    // Header separator
    let sep: String = col_widths
        .iter()
        .map(|w| "─".repeat(*w))
        .collect::<Vec<_>>()
        .join("─┼─");
    lines.push(Line {
        spans: vec![StyledSpan {
            text: format!("{}├─{}─┤", indent, sep),
            style: style_fg(bc),
        }],
        meta: LineMeta::None,
    });

    // Data rows
    for row in &rows {
        let mut spans = vec![StyledSpan {
            text: format!("{}│ ", indent),
            style: style_fg(bc),
        }];
        for (ci, (text, cell_type)) in row.iter().enumerate() {
            let fg = cell_type.color(theme);
            spans.push(StyledSpan {
                text: pad_or_truncate(text, col_widths[ci]),
                style: style_fg(fg),
            });
            if ci < row.len() - 1 {
                spans.push(StyledSpan {
                    text: " │ ".to_string(),
                    style: style_fg(bc),
                });
            }
        }
        spans.push(StyledSpan {
            text: " │".to_string(),
            style: style_fg(bc),
        });
        lines.push(Line {
            spans,
            meta: LineMeta::None,
        });
    }

    // Bottom border
    let bot: String = col_widths
        .iter()
        .map(|w| "─".repeat(*w))
        .collect::<Vec<_>>()
        .join("─┴─");
    lines.push(Line {
        spans: vec![StyledSpan {
            text: format!("{}└─{}─┘", indent, bot),
            style: style_fg(bc),
        }],
        meta: LineMeta::None,
    });

    lines
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

    fn render_table(&mut self, arr: &[Value], depth: usize) {
        let indent = indent_str(depth);
        let indent_w = UnicodeWidthStr::width(indent.as_str());
        let available = self.width.saturating_sub(indent_w);
        self.lines
            .extend(build_table_lines(self.theme, arr, &indent, available));
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
        self.lines.push(Line {
            spans: vec![],
            meta: LineMeta::None,
        });
    }

    fn emit_kv(&mut self, key: &str, value: &Value, depth: usize, align_width: usize) {
        self.lines
            .push(make_kv_line(self.theme, key, value, depth, align_width));
    }

    fn emit_indented_value(&mut self, value: &Value, depth: usize) {
        self.lines
            .push(make_indented_value_line(self.theme, value, depth));
    }

    fn emit_bullet(&mut self, value: &Value, depth: usize) {
        self.lines.push(make_bullet_line(self.theme, value, depth));
    }

    fn emit_index_label(&mut self, index: usize, depth: usize) {
        let indent = indent_str(depth);
        self.lines.push(Line {
            spans: vec![StyledSpan {
                text: format!("{}[{}]", indent, index),
                style: style_fg(self.theme.json_bracket),
            }],
            meta: LineMeta::None,
        });
    }

    fn emit_index_label_with_annotation(&mut self, index: usize, annotation: &str, depth: usize) {
        let indent = indent_str(depth);
        self.lines.push(Line {
            spans: vec![
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
            meta: LineMeta::None,
        });
    }

    fn emit_indexed_value(&mut self, index: usize, value: &Value, depth: usize) {
        self.lines
            .push(make_indexed_value_line(self.theme, index, value, depth));
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
    /// Card ID this row belongs to (graph view only).
    pub card_id: String,
    /// X position of the card for horizontal auto-pan.
    pub nav_x: usize,
    /// Width of the card for horizontal auto-pan.
    pub card_width: usize,
    /// If this row links to a child card, that card's id.
    pub child_card_id: Option<String>,
    /// Nav index of the parent row that connects to this card.
    pub parent_nav_index: Option<usize>,
    /// Nav index of the first row in the child card.
    pub child_nav_index: Option<usize>,
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
    /// Horizontal scroll offset for diagram mode.
    pub h_offset: usize,
    /// Total canvas width of the last diagram render.
    pub diagram_canvas_width: usize,
}

impl JsonViewState {
    pub fn new() -> Self {
        Self {
            expanded: HashSet::new(),
            cursor: 0,
            navigable: Vec::new(),
            cursor_path_save: None,
            diagram_mode: false,
            h_offset: 0,
            diagram_canvas_width: 0,
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
        let last = self.navigable.len() - 1;
        if delta > 0 {
            self.cursor = (self.cursor + delta as usize).min(last);
        } else {
            self.cursor = self.cursor.saturating_sub(delta.unsigned_abs() as usize);
        }
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
    pub fn expand_all(&mut self, root: &Value) {
        if let Some(nav) = self.navigable.get(self.cursor) {
            self.cursor_path_save = Some(nav.path.clone());
        }
        let mut paths = Vec::new();
        collect_all_children(root, "", &mut paths);
        for p in paths {
            self.expanded.insert(p);
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
    value: &Value,
    width: usize,
    theme: &Theme,
    expanded: &HashSet<String>,
) -> (Vec<Line>, DocumentInfo, Vec<NavItem>) {
    let mut r = CardRenderer {
        theme,
        lines: Vec::new(),
        width,
        expanded,
        navigable: Vec::new(),
        card_starts: Vec::new(),
        nesting: 0,
    };
    r.render_root(value);
    (
        r.lines,
        DocumentInfo {
            code_blocks: Vec::new(),
        },
        r.navigable,
    )
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
        let available = self
            .card_width(self.nesting.saturating_sub(1))
            .saturating_sub(8);
        self.lines
            .extend(build_table_lines(self.theme, arr, "", available));
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
            card_id: String::new(),
            nav_x: 0,
            card_width: 0,
            child_card_id: None,
            parent_nav_index: None,
            child_nav_index: None,
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
        self.push_line_obj(make_kv_line(self.theme, key, value, depth, align));
    }

    fn emit_bullet(&mut self, value: &Value, depth: usize) {
        self.push_line_obj(make_bullet_line(self.theme, value, depth));
    }

    fn emit_indexed_value(&mut self, index: usize, value: &Value, depth: usize) {
        self.push_line_obj(make_indexed_value_line(self.theme, index, value, depth));
    }

    fn emit_indented_value(&mut self, value: &Value, depth: usize) {
        self.push_line_obj(make_indented_value_line(self.theme, value, depth));
    }

    fn emit_blank(&mut self) {
        self.push_line(vec![], LineMeta::None);
    }

    fn push_line_obj(&mut self, line: Line) {
        self.lines.push(line);
    }
}

type EntryList<'a> = Vec<(&'a String, &'a Value)>;

fn group_entries(map: &serde_json::Map<String, Value>) -> (EntryList<'_>, EntryList<'_>) {
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
            let char_count = s.chars().count();
            if char_count > 40 {
                let truncated: String = s.chars().take(39).collect();
                format!("{}\u{2026}", truncated)
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

/// Tracks the JSON type of a table cell for accurate coloring.
#[derive(Clone, Copy)]
enum CellType {
    String,
    Number,
    Bool,
    Null,
}

impl CellType {
    fn from_value(v: &Value) -> Self {
        match v {
            Value::Null => CellType::Null,
            Value::Bool(_) => CellType::Bool,
            Value::Number(_) => CellType::Number,
            _ => CellType::String,
        }
    }

    fn color(self, theme: &Theme) -> Color {
        match self {
            CellType::Null => theme.json_null,
            CellType::Bool => theme.json_bool,
            CellType::Number => theme.json_number,
            CellType::String => theme.json_string,
        }
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

// ── Horizontal clipping for graph view ───────────────────────────

/// Clip a row of spans to the visible window `[h_offset, h_offset + visible_width)`.
/// Characters before h_offset are dropped; characters past visible_width are truncated.
fn clip_spans_horizontal(
    spans: &[StyledSpan],
    h_offset: usize,
    visible_width: usize,
) -> Vec<StyledSpan> {
    if h_offset == 0 && visible_width == usize::MAX {
        return spans.to_vec();
    }
    let end = h_offset + visible_width;
    let mut result = Vec::new();
    let mut col = 0usize;

    for span in spans {
        let span_w = UnicodeWidthStr::width(span.text.as_str());
        let span_end = col + span_w;

        if span_end <= h_offset || col >= end {
            // Entirely outside the visible window
            col = span_end;
            continue;
        }

        // Compute which chars are visible
        let skip_left = h_offset.saturating_sub(col);
        let take = (span_end.min(end) - col).saturating_sub(skip_left);

        if take == 0 {
            col = span_end;
            continue;
        }

        // Walk chars, skipping skip_left display columns, taking `take` columns
        let mut text = String::new();
        let mut skipped = 0usize;
        let mut taken = 0usize;
        for ch in span.text.chars() {
            let cw = UnicodeWidthStr::width(ch.to_string().as_str());
            if skipped < skip_left {
                skipped += cw;
                continue;
            }
            if taken + cw > take {
                break;
            }
            text.push(ch);
            taken += cw;
        }

        if !text.is_empty() {
            result.push(StyledSpan {
                text,
                style: span.style.clone(),
            });
        }

        col = span_end;
    }

    result
}

// ── Graph view (jsoncrack-style) ──────────────────────────────────

/// Maximum rows shown per card before truncating with "+N more".
const GRAPH_MAX_ROWS: usize = 12;

/// A card in the JSON graph.  Each JSON object/array becomes one card
/// containing all of its key-value pairs as rows.
struct GraphCard {
    id: String,
    title: String,
    rows: Vec<GraphCardRow>,
}

struct GraphCardRow {
    key: String,
    value_text: String,
    value_color: Option<Color>,
    /// If this row links to an expanded child card, its id.
    child_card_id: Option<String>,
    /// Navigation path for expand/collapse.
    nav_path: String,
    /// True when the child is expanded and an edge should be drawn.
    is_connector: bool,
}

/// Build graph cards from a JSON value. Each object/array becomes one card
/// containing rows for all its children. Primitive values are shown inline;
/// nested containers either show a summary (collapsed) or link to a child card
/// (expanded). No depth limit — the user can expand anything.
#[allow(clippy::too_many_arguments)]
fn build_graph_cards(
    value: &Value,
    node_id: &str,
    path: &str,
    title: &str,
    is_root: bool,
    cards: &mut Vec<GraphCard>,
    expanded: &HashSet<String>,
    theme: &Theme,
) {
    let is_obj = matches!(value, Value::Object(_));
    let is_arr = matches!(value, Value::Array(_));

    if !is_obj && !is_arr {
        // Primitive at root — single card with one inline row
        let val_text = format_primitive_short(value);
        let val_color = primitive_color(value, theme);
        cards.push(GraphCard {
            id: node_id.to_string(),
            title: title.to_string(),
            rows: vec![GraphCardRow {
                key: String::new(),
                value_text: val_text,
                value_color: Some(val_color),
                child_card_id: None,
                nav_path: path.to_string(),
                is_connector: false,
            }],
        });
        return;
    }

    let is_expanded = is_root || expanded.contains(path);

    if !is_expanded {
        // Collapsed card — single row showing count so it's clearly expandable
        let summary = if is_obj {
            let m = value.as_object().unwrap();
            format!("▶ {{{} keys}}", m.len())
        } else {
            let a = value.as_array().unwrap();
            format!("▶ [{} items]", a.len())
        };
        cards.push(GraphCard {
            id: node_id.to_string(),
            title: title.to_string(),
            rows: vec![GraphCardRow {
                key: String::new(),
                value_text: summary,
                value_color: Some(theme.json_bracket),
                child_card_id: None,
                nav_path: path.to_string(),
                is_connector: false,
            }],
        });
        return;
    }

    // Collect children metadata: (child_id, child_path, key_label, &Value)
    let children_meta: Vec<(String, String, String, &Value)> = if is_obj {
        let map = value.as_object().unwrap();
        map.iter()
            .map(|(k, v)| {
                let child_id = format!("{}/{}", node_id, k);
                let child_path = if path.is_empty() {
                    k.to_string()
                } else {
                    format!("{}.{}", path, k)
                };
                (child_id, child_path, k.to_string(), v)
            })
            .collect()
    } else {
        let arr = value.as_array().unwrap();
        arr.iter()
            .enumerate()
            .map(|(i, v)| {
                let child_id = format!("{}[{}]", node_id, i);
                let child_path = format!("{}[{}]", path, i);
                (child_id, child_path, format!("#{}", i + 1), v)
            })
            .collect()
    };

    let total = children_meta.len();
    let truncated = total > GRAPH_MAX_ROWS;
    let show_count = if truncated { GRAPH_MAX_ROWS } else { total };

    let mut rows = Vec::new();

    for (i, (child_id, child_path, key_label, child_val)) in children_meta.iter().enumerate() {
        if i >= show_count {
            break;
        }

        let is_container = !is_primitive_or_empty(child_val);

        if !is_container {
            // Primitive or empty — inline value
            let val_text = format_primitive_short(child_val);
            let val_color = primitive_color(child_val, theme);
            rows.push(GraphCardRow {
                key: key_label.clone(),
                value_text: val_text,
                value_color: Some(val_color),
                child_card_id: None,
                nav_path: child_path.clone(),
                is_connector: false,
            });
        } else {
            let child_expanded = expanded.contains(child_path.as_str());

            if child_expanded {
                // Expanded child → single connector row + child card
                let child_title = if is_arr {
                    format!("{} {}", title, key_label)
                } else {
                    key_label.clone()
                };
                rows.push(GraphCardRow {
                    key: key_label.clone(),
                    value_text: String::new(),
                    value_color: None,
                    child_card_id: Some(child_id.clone()),
                    nav_path: child_path.clone(),
                    is_connector: true,
                });
                build_graph_cards(
                    child_val,
                    child_id,
                    child_path,
                    &child_title,
                    false, // user must explicitly expand
                    cards,
                    expanded,
                    theme,
                );
            } else {
                // Collapsed child — summary with ▶ indicator
                let summary = match child_val {
                    Value::Object(m) => format!("▶ {{{}}}", m.len()),
                    Value::Array(a) => format!("▶ [{}]", a.len()),
                    _ => "…".to_string(),
                };
                rows.push(GraphCardRow {
                    key: key_label.clone(),
                    value_text: summary,
                    value_color: Some(theme.json_bracket),
                    child_card_id: None,
                    nav_path: child_path.clone(),
                    is_connector: false,
                });
            }
        }
    }

    if truncated {
        rows.push(GraphCardRow {
            key: String::new(),
            value_text: format!("+{} more", total - show_count),
            value_color: Some(theme.overlay_muted),
            child_card_id: None,
            nav_path: String::new(),
            is_connector: false,
        });
    }

    // Handle empty container
    if rows.is_empty() {
        let empty_text = if is_obj { "{}" } else { "[]" };
        rows.push(GraphCardRow {
            key: String::new(),
            value_text: empty_text.to_string(),
            value_color: Some(theme.json_bracket),
            child_card_id: None,
            nav_path: String::new(),
            is_connector: false,
        });
    }

    cards.push(GraphCard {
        id: node_id.to_string(),
        title: title.to_string(),
        rows,
    });
}

fn primitive_color(val: &Value, theme: &Theme) -> Color {
    match val {
        Value::String(_) => theme.json_string,
        Value::Number(_) => theme.json_number,
        Value::Bool(_) => theme.json_bool,
        Value::Null => theme.json_null,
        Value::Object(_) | Value::Array(_) => theme.json_bracket,
    }
}

fn format_primitive_short(val: &Value) -> String {
    match val {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            let char_count = s.chars().count();
            if char_count > 40 {
                let truncated: String = s.chars().take(37).collect();
                format!("\"{}...\"", truncated)
            } else {
                format!("\"{}\"", s)
            }
        }
        Value::Object(m) if m.is_empty() => "{}".to_string(),
        Value::Array(a) if a.is_empty() => "[]".to_string(),
        _ => "…".to_string(),
    }
}

/// Render JSON as a left-to-right graph of connected cards (jsoncrack style).
/// Returns (lines, doc_info, navigable_items, canvas_width).
pub fn render_diagram(
    value: &Value,
    width: usize,
    theme: &Theme,
    expanded: &HashSet<String>,
    cursor_path: Option<&str>,
    h_offset: usize,
) -> (Vec<Line>, DocumentInfo, Vec<NavItem>, usize) {
    use crate::diagram::{Canvas, CardDrawRow};

    let mut all_cards = Vec::new();
    build_graph_cards(
        value,
        "root",
        "",
        "root",
        true,
        &mut all_cards,
        expanded,
        theme,
    );

    if all_cards.is_empty() {
        return (
            Vec::new(),
            DocumentInfo {
                code_blocks: Vec::new(),
            },
            Vec::new(),
            0,
        );
    }

    // Build id → index lookup
    let id_to_idx: std::collections::HashMap<&str, usize> = all_cards
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();

    // ── Column assignment via BFS from root ──
    let mut columns: Vec<Vec<usize>> = Vec::new();
    let mut card_col: Vec<usize> = vec![0; all_cards.len()];
    {
        let mut visited = HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        if let Some(&root_idx) = id_to_idx.get("root") {
            queue.push_back((root_idx, 0usize));
            visited.insert(root_idx);
        }
        while let Some((idx, col)) = queue.pop_front() {
            while columns.len() <= col {
                columns.push(Vec::new());
            }
            columns[col].push(idx);
            card_col[idx] = col;

            for row in &all_cards[idx].rows {
                if let Some(ref child_id) = row.child_card_id
                    && let Some(&child_idx) = id_to_idx.get(child_id.as_str())
                    && visited.insert(child_idx)
                {
                    queue.push_back((child_idx, col + 1));
                }
            }
        }
    }

    if columns.is_empty() {
        return (
            Vec::new(),
            DocumentInfo {
                code_blocks: Vec::new(),
            },
            Vec::new(),
            0,
        );
    }

    // ── Card dimensions ──
    let max_card_width = 60usize;
    let min_card_width = 12usize;
    let h_gap = 8usize;
    let v_gap = 2usize;

    let card_widths: Vec<usize> = all_cards
        .iter()
        .map(|card| {
            let title_w = card.title.chars().count() + 6; // "╭─ title ─╮"
            let max_key = card
                .rows
                .iter()
                .map(|r| r.key.chars().count())
                .max()
                .unwrap_or(0);
            let max_val = card
                .rows
                .iter()
                .map(|r| {
                    if r.is_connector {
                        4 // "──▶ "
                    } else {
                        r.value_text.chars().count()
                    }
                })
                .max()
                .unwrap_or(0);
            let row_w = max_key + max_val + 5; // "│ key  value │"
            title_w.max(row_w).max(min_card_width).min(max_card_width)
        })
        .collect();

    let card_heights: Vec<usize> = all_cards
        .iter()
        .map(|card| {
            let row_count = card.rows.len().max(1);
            row_count + 2 // top border + rows + bottom border
        })
        .collect();

    // Column widths = max card width in each column
    let col_widths: Vec<usize> = columns
        .iter()
        .map(|col| {
            col.iter()
                .map(|&idx| card_widths[idx])
                .max()
                .unwrap_or(min_card_width)
        })
        .collect();

    // Column x positions
    let mut col_x: Vec<usize> = Vec::with_capacity(columns.len());
    {
        let mut x = 2; // left margin
        for (i, &cw) in col_widths.iter().enumerate() {
            col_x.push(x);
            if i + 1 < col_widths.len() {
                x += cw + h_gap;
            }
        }
    }

    // ── Vertical positioning ──
    // For each child card, try to align it with the parent row that references it.
    // Process columns left to right. Resolve overlaps by pushing down.
    struct CardPos {
        left_x: usize,
        top_y: usize,
        width: usize,
        height: usize,
        row_ys: Vec<usize>, // y coordinate of each content row (filled after drawing)
    }
    let mut positions: Vec<Option<CardPos>> = (0..all_cards.len()).map(|_| None).collect();

    // Column 0: start at y=1
    for (ci, col) in columns.iter().enumerate() {
        let mut next_y = 1usize;

        for &idx in col {
            let w = col_widths[ci];
            let h = card_heights[idx];

            // Compute ideal y: align with the parent row that references this card
            let ideal_y = if ci == 0 {
                next_y
            } else {
                // Find the parent card and row that references this card
                let mut found_y = next_y;
                'outer: for &parent_idx in &columns[ci - 1] {
                    if let Some(ref parent_pos) = positions[parent_idx] {
                        for (ri, row) in all_cards[parent_idx].rows.iter().enumerate() {
                            if row.child_card_id.as_deref() == Some(&all_cards[idx].id) {
                                // Align this card's first content row with parent's row
                                let parent_row_y = parent_pos.top_y + 1 + ri;
                                found_y = parent_row_y.saturating_sub(1); // align top border near row
                                break 'outer;
                            }
                        }
                    }
                }
                found_y
            };

            let y = ideal_y.max(next_y);
            positions[idx] = Some(CardPos {
                left_x: col_x[ci],
                top_y: y,
                width: w,
                height: h,
                row_ys: Vec::new(),
            });
            next_y = y + h + v_gap;
        }
    }

    // ── Canvas size ──
    let canvas_width = {
        let last_col = columns.len() - 1;
        col_x[last_col] + col_widths[last_col] + 2
    };
    let canvas_height = positions
        .iter()
        .filter_map(|p| p.as_ref().map(|p| p.top_y + p.height))
        .max()
        .unwrap_or(1)
        + 1;

    let mut canvas = Canvas::new(canvas_width, canvas_height);

    let border_fg = Some(theme.code_border);
    let cursor_highlight = Some(theme.link);
    let key_fg = Some(theme.json_key);

    // Determine which card is focused (contains the cursor)
    let focused_card_idx: Option<usize> = cursor_path.and_then(|cp| {
        all_cards
            .iter()
            .position(|card| card.rows.iter().any(|r| r.nav_path == cp))
    });

    // Highlight bg for the focused card
    let focus_bg = Some(theme.json_focus_bg);

    // ── Draw cards ──
    for (idx, card) in all_cards.iter().enumerate() {
        let pos = match positions[idx].as_ref() {
            Some(p) => p,
            None => continue,
        };

        // Build draw rows
        let draw_rows: Vec<CardDrawRow> = card
            .rows
            .iter()
            .map(|r| CardDrawRow {
                key: r.key.clone(),
                value_text: r.value_text.clone(),
                value_color: r.value_color,
                is_connector: r.is_connector,
            })
            .collect();

        // Determine which rows to highlight (cursor row within focused card)
        let is_focused = focused_card_idx == Some(idx);
        let mut highlight_rows = HashSet::new();
        if is_focused && let Some(cp) = cursor_path {
            for (ri, row) in card.rows.iter().enumerate() {
                if row.nav_path == cp {
                    highlight_rows.insert(ri);
                }
            }
        }

        let card_border = if is_focused {
            cursor_highlight
        } else {
            border_fg
        };
        let card_title_fg = if is_focused {
            cursor_highlight
        } else {
            Some(theme.json_key)
        };

        let row_ys = canvas.draw_card(
            pos.left_x,
            pos.top_y,
            pos.width,
            &card.title,
            &draw_rows,
            card_border,
            card_title_fg,
            key_fg,
            &highlight_rows,
            cursor_highlight,
            if is_focused { focus_bg } else { None },
        );

        // Store row y positions for edge routing
        if let Some(p) = positions[idx].as_mut() {
            p.row_ys = row_ys;
        }
    }

    // ── Draw edges (staggered to avoid overlap) ──
    let edge_fg = Some(theme.code_border);
    for (idx, card) in all_cards.iter().enumerate() {
        let src_pos = match positions[idx].as_ref() {
            Some(p) => p,
            None => continue,
        };

        // Count connector rows for stagger calculation
        let connector_indices: Vec<usize> = card
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.child_card_id.is_some())
            .map(|(ri, _)| ri)
            .collect();
        let num_connectors = connector_indices.len();

        for (ri, row) in card.rows.iter().enumerate() {
            if let Some(ref child_id) = row.child_card_id
                && let Some(&child_idx) = id_to_idx.get(child_id.as_str())
                && let Some(ref dst_pos) = positions[child_idx]
            {
                let src_right_x = src_pos.left_x + src_pos.width - 1;
                let src_cy = if ri < src_pos.row_ys.len() {
                    src_pos.row_ys[ri]
                } else {
                    src_pos.top_y + 1
                };
                let dst_left_x = dst_pos.left_x;
                let dst_cy = dst_pos.top_y + 1; // title row of child

                // Compute staggered mid_x to avoid edge overlap
                let mid_x_override = if num_connectors > 1 {
                    let edge_idx = connector_indices
                        .iter()
                        .position(|&ci| ci == ri)
                        .unwrap_or(0);
                    let gap = dst_left_x.saturating_sub(src_right_x + 2);
                    let step = gap / (num_connectors + 1);
                    Some(src_right_x + 1 + step * (edge_idx + 1))
                } else {
                    None
                };

                canvas.draw_edge_lr(
                    src_pos.left_x + src_pos.width / 2,
                    src_right_x,
                    src_cy,
                    dst_left_x,
                    dst_cy,
                    None,
                    edge_fg,
                    None,
                    mid_x_override,
                );
            }
        }
    }

    // ── Convert canvas to styled output ──
    let rows = canvas.to_span_rows(theme);

    let mut lines = Vec::new();
    let canvas_line_offset = 0;
    let visible_width = width;

    // Direct canvas rows with horizontal clipping
    for row_spans in &rows {
        let clipped = clip_spans_horizontal(row_spans, h_offset, visible_width);
        lines.push(Line {
            spans: clipped,
            meta: LineMeta::None,
        });
    }

    // ── Build navigable items ──
    // ALL rows in all cards are navigable. Each item knows its card_id,
    // and parent/child links for graph-aware navigation.
    // Order: by column (left to right), then by y position.
    let mut navigable: Vec<NavItem> = Vec::new();

    for col in &columns {
        for &idx in col {
            let card = &all_cards[idx];
            let pos = match positions[idx].as_ref() {
                Some(p) => p,
                None => continue,
            };

            for (ri, row) in card.rows.iter().enumerate() {
                let y = if ri < pos.row_ys.len() {
                    pos.row_ys[ri]
                } else {
                    pos.top_y + 1 + ri
                };
                let line_index = canvas_line_offset + y;
                navigable.push(NavItem {
                    line_index,
                    path: row.nav_path.clone(),
                    card_id: card.id.clone(),
                    nav_x: pos.left_x,
                    card_width: pos.width,
                    child_card_id: row.child_card_id.clone(),
                    parent_nav_index: None, // filled in below
                    child_nav_index: None,  // filled in below
                });
            }
        }
    }

    // ── Build parent/child nav indices ──
    // For each nav item that has a child_card_id, find the first nav item
    // in that child card. For each card, find the parent nav item that
    // connects to it.
    let nav_len = navigable.len();
    let mut parent_indices: Vec<Option<usize>> = vec![None; nav_len];
    let mut child_indices: Vec<Option<usize>> = vec![None; nav_len];

    for i in 0..nav_len {
        if let Some(ref child_id) = navigable[i].child_card_id {
            // Find the first nav item belonging to this child card
            if let Some(child_nav_idx) = navigable.iter().position(|n| n.card_id == *child_id) {
                child_indices[i] = Some(child_nav_idx);
                // Mark all items in the child card as having this parent
                for j in child_nav_idx..nav_len {
                    if navigable[j].card_id == *child_id {
                        parent_indices[j] = Some(i);
                    }
                }
            }
        }
    }

    // Write the indices into nav items
    for i in 0..nav_len {
        navigable[i].parent_nav_index = parent_indices[i];
        navigable[i].child_nav_index = child_indices[i];
    }

    (
        lines,
        DocumentInfo {
            code_blocks: Vec::new(),
        },
        navigable,
        canvas_width,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_all_expands_entire_document() {
        let json = r#"{"a":1,"nested":{"b":2,"deep":{"c":3}}}"#;
        let theme = crate::theme::Theme::dark();
        let value: Value = serde_json::from_str(json).unwrap();

        let (_, _, nav) = render_interactive(&value, 80, &theme, &HashSet::new());
        let mut state = JsonViewState::new();
        state.navigable = nav;
        state.expand_all(&value);

        // Should expand everything in the document, not just cursor's subtree
        assert!(state.expanded.contains("nested"));
        assert!(state.expanded.contains("nested.deep"));
    }

    #[test]
    fn expand_all_with_arrays() {
        let json = r#"{"items":[{"id":1,"sub":{"x":true}},{"id":2}]}"#;
        let theme = crate::theme::Theme::dark();
        let value: Value = serde_json::from_str(json).unwrap();

        let (_, _, nav) = render_interactive(&value, 80, &theme, &HashSet::new());
        let mut state = JsonViewState::new();
        state.navigable = nav;
        state.expand_all(&value);

        assert!(state.expanded.contains("items"));
        assert!(state.expanded.contains("items[0]"));
        assert!(state.expanded.contains("items[0].sub"));
        assert!(state.expanded.contains("items[1]"));
    }

    #[test]
    fn collapse_all_clears_everything() {
        let json = r#"{"a":1,"nested":{"b":2,"deep":{"c":3}}}"#;
        let theme = crate::theme::Theme::dark();
        let value: Value = serde_json::from_str(json).unwrap();

        let (_, _, nav) = render_interactive(&value, 80, &theme, &HashSet::new());
        let mut state = JsonViewState::new();
        state.navigable = nav;
        state.expand_all(&value);
        assert!(!state.expanded.is_empty());

        // Re-render with expanded state
        let (_, _, nav2) = render_interactive(&value, 80, &theme, &state.expanded);
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
