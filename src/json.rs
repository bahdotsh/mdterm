use std::collections::HashSet;

use crossterm::style::Color;
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::style::{CodeBlockContent, DocumentInfo, Line, LineMeta, Style, StyledSpan};
use crate::theme::Theme;

pub fn render(
    input: &str,
    width: usize,
    theme: &Theme,
) -> Result<(Vec<Line>, DocumentInfo), String> {
    let value: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
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
                let keys: Vec<&String> = map.keys().collect();
                for (i, key) in keys.iter().enumerate() {
                    let val = &map[*key];
                    let is_last = i == keys.len() - 1;

                    // Top-level keys become H1 headings (for TOC)
                    self.emit_heading(1, key);
                    self.emit_blank();

                    self.render_value(val, &[], is_last, 1);

                    if !is_last {
                        self.emit_blank();
                    }
                }
            }
            Value::Array(arr) => {
                self.emit_heading(1, "root");
                self.emit_blank();
                self.render_array(arr, &[], true, 1);
            }
            _ => {
                // Root is a primitive
                self.emit_heading(1, "value");
                self.emit_blank();
                self.emit_primitive_line(value, &[]);
            }
        }
    }

    // ── recursive renderer ────────────────────────────────────────

    fn render_value(
        &mut self,
        value: &Value,
        guides: &[bool], // true = last child at that depth (use space, not │)
        is_last: bool,
        heading_depth: usize,
    ) {
        match value {
            Value::Object(map) => {
                self.render_object(map, guides, heading_depth);
            }
            Value::Array(arr) => {
                self.render_array(arr, guides, is_last, heading_depth);
            }
            _ => {
                self.emit_primitive_line(value, guides);
            }
        }
    }

    fn render_object(
        &mut self,
        map: &serde_json::Map<String, Value>,
        guides: &[bool],
        heading_depth: usize,
    ) {
        if map.is_empty() {
            let bc = self.theme.json_bracket;
            self.emit_line_with_guides(guides, false, |this| {
                this.span("{}", bc, false);
            });
            return;
        }

        let keys: Vec<&String> = map.keys().collect();
        for (i, key) in keys.iter().enumerate() {
            let val = &map[*key];
            let is_last_key = i == keys.len() - 1;

            match val {
                Value::Object(inner) if !inner.is_empty() => {
                    // Nested object: show key as a sub-heading or tree node
                    if heading_depth < 6 {
                        let depth = heading_depth + 1;
                        let indent = guide_prefix(guides);
                        let label = format!("{}{}", indent, key);
                        self.emit_heading(depth as u8, &label);
                        let mut child_guides = guides.to_vec();
                        child_guides.push(is_last_key);
                        self.render_object(inner, &child_guides, depth);
                    } else {
                        self.emit_key_line(key, guides, is_last_key);
                        let mut child_guides = guides.to_vec();
                        child_guides.push(is_last_key);
                        self.render_object(inner, &child_guides, heading_depth);
                    }
                }
                Value::Array(arr) if !arr.is_empty() => {
                    // Show key, then render array beneath
                    let count_label = format!("{} ({} items)", key, arr.len());
                    if heading_depth < 6 {
                        let depth = heading_depth + 1;
                        let indent = guide_prefix(guides);
                        let label = format!("{}{}", indent, count_label);
                        self.emit_heading(depth as u8, &label);
                        let mut child_guides = guides.to_vec();
                        child_guides.push(is_last_key);
                        self.render_array(arr, &child_guides, is_last_key, depth);
                    } else {
                        self.emit_key_with_annotation(
                            key,
                            &format!("({} items)", arr.len()),
                            guides,
                            is_last_key,
                        );
                        let mut child_guides = guides.to_vec();
                        child_guides.push(is_last_key);
                        self.render_array(arr, &child_guides, is_last_key, heading_depth);
                    }
                }
                _ => {
                    // key: primitive value on one line
                    self.emit_key_value_line(key, val, guides, is_last_key);
                }
            }
        }
    }

    fn render_array(
        &mut self,
        arr: &[Value],
        guides: &[bool],
        _is_last: bool,
        heading_depth: usize,
    ) {
        if arr.is_empty() {
            let bc = self.theme.json_bracket;
            self.emit_line_with_guides(guides, false, |this| {
                this.span("[]", bc, false);
            });
            return;
        }

        // Check if this array of objects should render as a table
        if should_render_as_table(arr) {
            self.render_table(arr, guides);
            return;
        }

        for (i, item) in arr.iter().enumerate() {
            let is_last_item = i == arr.len() - 1;
            let index_label = format!("[{}]", i);

            let bc = self.theme.json_bracket;
            match item {
                Value::Object(map) if !map.is_empty() => {
                    self.emit_line_with_guides(guides, is_last_item, |this| {
                        this.span(&index_label, bc, false);
                    });
                    let mut child_guides = guides.to_vec();
                    child_guides.push(is_last_item);
                    self.render_object(map, &child_guides, heading_depth);
                }
                Value::Array(inner) if !inner.is_empty() => {
                    let label = format!("{} ({} items)", index_label, inner.len());
                    self.emit_line_with_guides(guides, is_last_item, |this| {
                        this.span(&label, bc, false);
                    });
                    let mut child_guides = guides.to_vec();
                    child_guides.push(is_last_item);
                    self.render_array(inner, &child_guides, is_last_item, heading_depth);
                }
                _ => {
                    let label = format!("{} ", index_label);
                    self.emit_line_with_guides(guides, is_last_item, |this| {
                        this.span(&label, bc, false);
                        this.push_value_span(item);
                    });
                }
            }
        }
    }

    // ── table rendering ───────────────────────────────────────────

    fn render_table(&mut self, arr: &[Value], guides: &[bool]) {
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
        let indent = guide_prefix(guides);
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
                header_w.max(max_cell).max(3) // minimum 3 chars
            })
            .collect();

        // Shrink columns if total exceeds available width
        // Each column takes col_width + 3 (for " │ " separator), minus last separator
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

        let border_color = self.theme.table_border;
        let header_color = self.theme.table_header;

        // Top border
        let top: String = col_widths
            .iter()
            .map(|w| "─".repeat(*w))
            .collect::<Vec<_>>()
            .join("─┬─");
        self.push_line(
            vec![StyledSpan {
                text: format!("{}┌─{}─┐", indent, top),
                style: style_fg(border_color),
            }],
            LineMeta::None,
        );

        // Header row
        let mut hdr_spans = vec![StyledSpan {
            text: format!("{}│ ", indent),
            style: style_fg(border_color),
        }];
        for (ci, h) in headers.iter().enumerate() {
            let padded = pad_or_truncate(h, col_widths[ci]);
            hdr_spans.push(StyledSpan {
                text: padded,
                style: Style {
                    fg: Some(header_color),
                    bold: true,
                    ..Default::default()
                },
            });
            if ci < headers.len() - 1 {
                hdr_spans.push(StyledSpan {
                    text: " │ ".to_string(),
                    style: style_fg(border_color),
                });
            }
        }
        hdr_spans.push(StyledSpan {
            text: " │".to_string(),
            style: style_fg(border_color),
        });
        self.push_line(hdr_spans, LineMeta::None);

        // Header separator
        let sep: String = col_widths
            .iter()
            .map(|w| "─".repeat(*w))
            .collect::<Vec<_>>()
            .join("─┼─");
        self.push_line(
            vec![StyledSpan {
                text: format!("{}├─{}─┤", indent, sep),
                style: style_fg(border_color),
            }],
            LineMeta::None,
        );

        // Data rows
        for row in &rows {
            let mut row_spans = vec![StyledSpan {
                text: format!("{}│ ", indent),
                style: style_fg(border_color),
            }];
            for (ci, cell) in row.iter().enumerate() {
                let padded = pad_or_truncate(cell, col_widths[ci]);
                // Try to color based on original value type
                let fg = cell_color(cell, self.theme);
                row_spans.push(StyledSpan {
                    text: padded,
                    style: style_fg(fg),
                });
                if ci < row.len() - 1 {
                    row_spans.push(StyledSpan {
                        text: " │ ".to_string(),
                        style: style_fg(border_color),
                    });
                }
            }
            row_spans.push(StyledSpan {
                text: " │".to_string(),
                style: style_fg(border_color),
            });
            self.push_line(row_spans, LineMeta::None);
        }

        // Bottom border
        let bot: String = col_widths
            .iter()
            .map(|w| "─".repeat(*w))
            .collect::<Vec<_>>()
            .join("─┴─");
        self.push_line(
            vec![StyledSpan {
                text: format!("{}└─{}─┘", indent, bot),
                style: style_fg(border_color),
            }],
            LineMeta::None,
        );
    }

    // ── line emission helpers ─────────────────────────────────────

    fn emit_heading(&mut self, level: u8, text: &str) {
        let color = match level {
            1 => self.theme.h1,
            2 => self.theme.h2,
            3 => self.theme.h3,
            4 => self.theme.h4,
            5 => self.theme.h5,
            _ => self.theme.h6,
        };
        let display = text.to_string();
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
                text: display.clone(),
                style: Style {
                    fg: Some(color),
                    bold: true,
                    ..Default::default()
                },
            },
        ];

        // Separator line beneath H1/H2
        self.push_line(
            spans,
            LineMeta::Heading {
                level,
                text: display.clone(),
            },
        );

        if level <= 2 {
            let sep_w = self.width.min(60);
            spans = vec![StyledSpan {
                text: "─".repeat(sep_w),
                style: style_fg(self.theme.heading_separator),
            }];
            self.push_line(spans, LineMeta::None);
        }
    }

    fn emit_blank(&mut self) {
        self.push_line(vec![], LineMeta::None);
    }

    fn emit_primitive_line(&mut self, value: &Value, guides: &[bool]) {
        let prefix = guide_prefix(guides);
        let mut spans = Vec::new();
        if !prefix.is_empty() {
            spans.push(StyledSpan {
                text: prefix,
                style: style_fg(self.theme.json_bracket),
            });
        }
        spans.push(self.value_span(value));
        self.push_line(spans, LineMeta::None);
    }

    fn emit_key_value_line(&mut self, key: &str, value: &Value, guides: &[bool], is_last: bool) {
        let connector = if is_last { "└─ " } else { "├─ " };
        let prefix = guide_prefix(guides);

        let mut spans = vec![
            StyledSpan {
                text: format!("{}{}", prefix, connector),
                style: style_fg(self.theme.json_bracket),
            },
            StyledSpan {
                text: format!("{}: ", key),
                style: Style {
                    fg: Some(self.theme.json_key),
                    bold: true,
                    ..Default::default()
                },
            },
        ];
        spans.push(self.value_span(value));
        self.push_line(spans, LineMeta::None);
    }

    fn emit_key_line(&mut self, key: &str, guides: &[bool], is_last: bool) {
        let connector = if is_last { "└─ " } else { "├─ " };
        let prefix = guide_prefix(guides);

        self.push_line(
            vec![
                StyledSpan {
                    text: format!("{}{}", prefix, connector),
                    style: style_fg(self.theme.json_bracket),
                },
                StyledSpan {
                    text: key.to_string(),
                    style: Style {
                        fg: Some(self.theme.json_key),
                        bold: true,
                        ..Default::default()
                    },
                },
            ],
            LineMeta::None,
        );
    }

    fn emit_key_with_annotation(
        &mut self,
        key: &str,
        annotation: &str,
        guides: &[bool],
        is_last: bool,
    ) {
        let connector = if is_last { "└─ " } else { "├─ " };
        let prefix = guide_prefix(guides);

        self.push_line(
            vec![
                StyledSpan {
                    text: format!("{}{}", prefix, connector),
                    style: style_fg(self.theme.json_bracket),
                },
                StyledSpan {
                    text: format!("{} ", key),
                    style: Style {
                        fg: Some(self.theme.json_key),
                        bold: true,
                        ..Default::default()
                    },
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

    /// Emit a tree line with guide prefix, connector, and caller-provided content.
    fn emit_line_with_guides<F>(&mut self, guides: &[bool], is_last: bool, content_fn: F)
    where
        F: FnOnce(&mut LineBuilder),
    {
        let connector = if is_last { "└─ " } else { "├─ " };
        let prefix = guide_prefix(guides);

        let mut builder = LineBuilder {
            spans: vec![StyledSpan {
                text: format!("{}{}", prefix, connector),
                style: style_fg(self.theme.json_bracket),
            }],
        };
        content_fn(&mut builder);
        self.push_line(builder.spans, LineMeta::None);
    }

    // ── span helpers ──────────────────────────────────────────────

    fn value_span(&self, value: &Value) -> StyledSpan {
        match value {
            Value::String(s) => {
                let display = format!("\"{}\"", s);
                // Detect URLs
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
            // Objects/arrays shouldn't reach here, but handle gracefully
            Value::Object(_) => StyledSpan {
                text: "{...}".to_string(),
                style: style_fg(self.theme.json_bracket),
            },
            Value::Array(_) => StyledSpan {
                text: "[...]".to_string(),
                style: style_fg(self.theme.json_bracket),
            },
        }
    }

    fn push_line(&mut self, spans: Vec<StyledSpan>, meta: LineMeta) {
        self.lines.push(Line { spans, meta });
    }
}

// ── LineBuilder for closures ─────────────────────────────────────

struct LineBuilder {
    spans: Vec<StyledSpan>,
}

impl LineBuilder {
    fn span(&mut self, text: &str, fg: Color, bold: bool) {
        self.spans.push(StyledSpan {
            text: text.to_string(),
            style: Style {
                fg: Some(fg),
                bold,
                ..Default::default()
            },
        });
    }

    fn push_value_span(&mut self, value: &Value) {
        match value {
            Value::String(s) => {
                self.spans.push(StyledSpan {
                    text: format!("\"{}\"", s),
                    style: Style {
                        fg: Some(Color::Rgb {
                            r: 166,
                            g: 227,
                            b: 161,
                        }),
                        link_url: if s.starts_with("http://") || s.starts_with("https://") {
                            Some(s.clone())
                        } else {
                            None
                        },
                        underline: s.starts_with("http://") || s.starts_with("https://"),
                        ..Default::default()
                    },
                });
            }
            Value::Number(n) => {
                self.spans.push(StyledSpan {
                    text: n.to_string(),
                    style: Style {
                        fg: Some(Color::Rgb {
                            r: 250,
                            g: 179,
                            b: 135,
                        }),
                        ..Default::default()
                    },
                });
            }
            Value::Bool(b) => {
                self.spans.push(StyledSpan {
                    text: b.to_string(),
                    style: Style {
                        fg: Some(Color::Rgb {
                            r: 249,
                            g: 226,
                            b: 175,
                        }),
                        ..Default::default()
                    },
                });
            }
            Value::Null => {
                self.spans.push(StyledSpan {
                    text: "null".to_string(),
                    style: Style {
                        fg: Some(Color::Rgb {
                            r: 108,
                            g: 112,
                            b: 134,
                        }),
                        dim: true,
                        ..Default::default()
                    },
                });
            }
            _ => {
                self.spans.push(StyledSpan {
                    text: format!("{}", value),
                    style: Default::default(),
                });
            }
        }
    }
}

// ── free helpers ──────────────────────────────────────────────────

fn guide_prefix(guides: &[bool]) -> String {
    let mut s = String::new();
    for &is_last in guides {
        if is_last {
            s.push_str("   ");
        } else {
            s.push_str("│  ");
        }
    }
    s
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
    // All values should be primitives (no nested objects/arrays)
    for obj in &objects {
        for val in obj.values() {
            if val.is_object() || val.is_array() {
                return false;
            }
        }
    }
    // Check key overlap: collect all keys, each object must have ≥50%
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
                format!("{}…", &s[..39])
            } else {
                s.clone()
            }
        }
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Object(_) => "{...}".to_string(),
        Value::Array(_) => "[...]".to_string(),
    }
}

fn pad_or_truncate(s: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w > width {
        // Truncate with ellipsis
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
        result.push('…');
        // Pad remaining
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
