use crossterm::style::Color;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::style::{Line, Style, StyledSpan};

struct Renderer {
    lines: Vec<Line>,
    current_spans: Vec<StyledSpan>,
    width: usize,

    // Inline style state
    bold: bool,
    italic: bool,
    strikethrough: bool,

    // Block state
    heading_level: Option<HeadingLevel>,
    in_blockquote: bool,
    in_code_block: bool,
    code_block_lang: String,
    code_block_content: String,

    // List state
    list_stack: Vec<ListKind>,

    // Table state
    in_table: bool,
    table_alignments: Vec<Alignment>,
    table_head: Vec<Vec<StyledSpan>>,
    table_rows: Vec<Vec<Vec<StyledSpan>>>,
    table_cell_spans: Vec<StyledSpan>,
    in_table_head: bool,
    table_current_row: Vec<Vec<StyledSpan>>,

    // Link state
    in_link: bool,
    link_url: String,

    // Syntect (loaded once)
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
}

#[derive(Clone)]
enum ListKind {
    Unordered,
    Ordered(u64),
}

impl Renderer {
    fn new(width: usize) -> Self {
        Renderer {
            lines: Vec::new(),
            current_spans: Vec::new(),
            width,
            bold: false,
            italic: false,
            strikethrough: false,
            heading_level: None,
            in_blockquote: false,
            in_code_block: false,
            code_block_lang: String::new(),
            code_block_content: String::new(),
            list_stack: Vec::new(),
            in_table: false,
            table_alignments: Vec::new(),
            table_head: Vec::new(),
            table_rows: Vec::new(),
            table_cell_spans: Vec::new(),
            in_table_head: false,
            table_current_row: Vec::new(),
            in_link: false,
            link_url: String::new(),
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
        }
    }

    fn current_style(&self) -> Style {
        let mut style = Style::default();

        if let Some(level) = self.heading_level {
            style.bold = true;
            style.fg = Some(match level {
                HeadingLevel::H1 => Color::White,
                HeadingLevel::H2 => Color::Rgb { r: 138, g: 180, b: 248 },
                HeadingLevel::H3 => Color::Rgb { r: 190, g: 145, b: 230 },
                HeadingLevel::H4 => Color::Rgb { r: 129, g: 199, b: 132 },
                HeadingLevel::H5 => Color::Rgb { r: 255, g: 183, b: 77 },
                HeadingLevel::H6 => Color::Rgb { r: 150, g: 150, b: 150 },
            });
        }

        if self.bold {
            style.bold = true;
        }
        if self.italic {
            style.italic = true;
        }
        if self.strikethrough {
            style.strikethrough = true;
        }
        if self.in_blockquote {
            style.italic = true;
        }

        style
    }

    fn push_span(&mut self, text: &str, style: Style) {
        self.current_spans.push(StyledSpan {
            text: text.to_string(),
            style,
        });
    }

    fn flush_line(&mut self) {
        if !self.current_spans.is_empty() {
            let mut spans = Vec::new();
            if self.in_blockquote {
                spans.push(StyledSpan {
                    text: "  ┃ ".to_string(),
                    style: Style {
                        fg: Some(Color::Rgb { r: 100, g: 130, b: 180 }),
                        ..Default::default()
                    },
                });
            }
            spans.append(&mut self.current_spans);
            self.lines.push(Line { spans });
        }
    }

    fn push_empty_line(&mut self) {
        // Avoid consecutive empty lines
        if let Some(last) = self.lines.last() {
            if last.spans.is_empty() {
                return;
            }
        }
        if self.in_blockquote {
            self.lines.push(Line {
                spans: vec![StyledSpan {
                    text: "  ┃".to_string(),
                    style: Style {
                        fg: Some(Color::Rgb { r: 100, g: 130, b: 180 }),
                        ..Default::default()
                    },
                }],
            });
        } else {
            self.lines.push(Line::empty());
        }
    }

    fn emit_code_block(&mut self) {
        let lang = self.code_block_lang.trim().to_string();
        let code = std::mem::take(&mut self.code_block_content);
        let code_bg = Color::Rgb { r: 30, g: 33, b: 40 };
        let border_fg = Color::Rgb { r: 55, g: 58, b: 65 };
        let label_fg = Color::Rgb { r: 110, g: 115, b: 130 };

        let syntax = if lang.is_empty() {
            self.syntax_set.find_syntax_plain_text()
        } else {
            self.syntax_set
                .find_syntax_by_token(&lang)
                .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
        };

        let theme = &self.theme_set.themes["base16-ocean.dark"];
        let mut highlighter = HighlightLines::new(syntax, theme);

        // Measure content to size the box
        let code_lines: Vec<&str> = code.lines().collect();
        let max_line_len = code_lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        let content_width = max_line_len.max(40);
        // Inner width between ╭/│ and ╮/│: " " + content + " "
        let inner_width = content_width + 2;

        // Language label
        let label = if lang.is_empty() {
            String::new()
        } else {
            format!(" {} ", lang)
        };
        let label_len = label.chars().count();

        // Top border: "  ╭─ lang ──...──╮"
        let dashes_after = inner_width.saturating_sub(1 + label_len);
        let mut top_spans = vec![StyledSpan {
            text: "  ╭─".to_string(),
            style: Style {
                fg: Some(border_fg),
                ..Default::default()
            },
        }];
        if !lang.is_empty() {
            top_spans.push(StyledSpan {
                text: label,
                style: Style {
                    fg: Some(label_fg),
                    ..Default::default()
                },
            });
        }
        top_spans.push(StyledSpan {
            text: format!("{}╮", "─".repeat(dashes_after)),
            style: Style {
                fg: Some(border_fg),
                ..Default::default()
            },
        });
        self.lines.push(Line { spans: top_spans });

        // Code lines with left border, syntax highlighting, padding, and right border
        for line_str in LinesWithEndings::from(&code) {
            let mut spans = vec![
                StyledSpan {
                    text: "  │".to_string(),
                    style: Style {
                        fg: Some(border_fg),
                        ..Default::default()
                    },
                },
                StyledSpan {
                    text: " ".to_string(),
                    style: Style {
                        bg: Some(code_bg),
                        ..Default::default()
                    },
                },
            ];

            let mut char_count = 0;
            if let Ok(ranges) = highlighter.highlight_line(line_str, &self.syntax_set) {
                for (syn_style, text) in ranges {
                    let trimmed = text.trim_end_matches('\n').trim_end_matches('\r');
                    if !trimmed.is_empty() {
                        char_count += trimmed.chars().count();
                        let mut style = syntect_to_style(syn_style);
                        style.bg = Some(code_bg);
                        spans.push(StyledSpan {
                            text: trimmed.to_string(),
                            style,
                        });
                    }
                }
            } else {
                let trimmed = line_str
                    .trim_end_matches('\n')
                    .trim_end_matches('\r');
                char_count = trimmed.chars().count();
                spans.push(StyledSpan {
                    text: trimmed.to_string(),
                    style: Style {
                        bg: Some(code_bg),
                        ..Default::default()
                    },
                });
            }

            // Right padding (fill to content_width) + right margin
            let padding = content_width.saturating_sub(char_count) + 1;
            spans.push(StyledSpan {
                text: " ".repeat(padding),
                style: Style {
                    bg: Some(code_bg),
                    ..Default::default()
                },
            });
            spans.push(StyledSpan {
                text: "│".to_string(),
                style: Style {
                    fg: Some(border_fg),
                    ..Default::default()
                },
            });

            self.lines.push(Line { spans });
        }

        // Bottom border
        self.lines.push(Line {
            spans: vec![StyledSpan {
                text: format!("  ╰{}╯", "─".repeat(inner_width)),
                style: Style {
                    fg: Some(border_fg),
                    ..Default::default()
                },
            }],
        });
    }

    fn emit_table(&mut self) {
        let border_fg = Color::Rgb { r: 55, g: 58, b: 65 };
        let header_fg = Color::Rgb { r: 138, g: 180, b: 248 };

        let all_rows: Vec<&Vec<Vec<StyledSpan>>> = std::iter::once(&self.table_head)
            .chain(self.table_rows.iter())
            .collect();

        let num_cols = self.table_alignments.len();
        if num_cols == 0 {
            return;
        }

        // Measure natural column widths
        let mut col_widths = vec![0usize; num_cols];
        for row in &all_rows {
            for (i, cell) in row.iter().enumerate() {
                if i < num_cols {
                    let w: usize = cell.iter().map(|s| s.text.chars().count()).sum();
                    col_widths[i] = col_widths[i].max(w);
                }
            }
        }

        // Constrain to available width
        // Table line: "  │ cell │ cell │ ... │"
        // Overhead = 2 (indent) + (num_cols+1) (borders) + num_cols*2 (padding)
        let overhead = 3 + 3 * num_cols;
        let total_natural: usize = col_widths.iter().sum();
        let available = self.width.saturating_sub(overhead);

        if available > 0 && total_natural > available {
            // Keep small columns at natural width, shrink only the wide ones
            let fair_share = available / num_cols;
            let mut fixed_width = 0usize;
            let mut flex_natural = 0usize;

            for &w in col_widths.iter() {
                if w <= fair_share {
                    fixed_width += w;
                } else {
                    flex_natural += w;
                }
            }

            let flex_available = available.saturating_sub(fixed_width);
            let mut remaining = flex_available;
            let mut flex_remaining = col_widths.iter().filter(|&&w| w > fair_share).count();

            for w in col_widths.iter_mut() {
                if *w > fair_share {
                    flex_remaining -= 1;
                    if flex_remaining == 0 {
                        *w = remaining;
                    } else if flex_natural > 0 {
                        let share = (*w * flex_available / flex_natural).max(3);
                        *w = share;
                        remaining = remaining.saturating_sub(share);
                    }
                }
            }
        }

        // Minimum column width
        for w in &mut col_widths {
            *w = (*w).max(3);
        }

        let border_style = Style {
            fg: Some(border_fg),
            ..Default::default()
        };

        // Helper: build a horizontal rule line
        let make_rule = |left: &str, mid: &str, right: &str, widths: &[usize]| -> Line {
            let mut s = format!("  {}", left);
            for (i, &w) in widths.iter().enumerate() {
                s.push_str(&"─".repeat(w + 2));
                if i + 1 < widths.len() {
                    s.push_str(mid);
                }
            }
            s.push_str(right);
            Line {
                spans: vec![StyledSpan {
                    text: s,
                    style: border_style.clone(),
                }],
            }
        };

        // Top border
        self.lines.push(make_rule("╭", "┬", "╮", &col_widths));

        // Render each row (with multi-line cell support)
        for (row_idx, row) in all_rows.iter().enumerate() {
            let is_header = row_idx == 0;

            // Wrap each cell into visual lines
            let wrapped_cells: Vec<Vec<Vec<StyledSpan>>> = row
                .iter()
                .enumerate()
                .map(|(col_idx, cell)| {
                    let cw = col_widths.get(col_idx).copied().unwrap_or(3);
                    wrap_cell(cell, cw)
                })
                .collect();

            let num_visual_lines = wrapped_cells.iter().map(|c| c.len()).max().unwrap_or(1);

            for vline in 0..num_visual_lines {
                let mut spans = Vec::new();
                spans.push(StyledSpan {
                    text: "  │".to_string(),
                    style: border_style.clone(),
                });

                for (col_idx, &cw) in col_widths.iter().enumerate() {
                    let cell_lines = wrapped_cells.get(col_idx);
                    let cell_line = cell_lines.and_then(|cl| cl.get(vline));

                    let alignment = self.table_alignments.get(col_idx).unwrap_or(&Alignment::None);

                    if let Some(spans_in_line) = cell_line {
                        let content_width: usize =
                            spans_in_line.iter().map(|s| s.text.chars().count()).sum();
                        let pad = cw.saturating_sub(content_width);

                        let (pad_left, pad_right) = match alignment {
                            Alignment::Center => (pad / 2, pad - pad / 2),
                            Alignment::Right => (pad, 0),
                            _ => (0, pad),
                        };

                        spans.push(StyledSpan {
                            text: format!(" {}", " ".repeat(pad_left)),
                            style: Style::default(),
                        });

                        for span in spans_in_line {
                            let mut style = span.style.clone();
                            if is_header {
                                style.bold = true;
                                style.fg = Some(header_fg);
                            }
                            spans.push(StyledSpan {
                                text: span.text.clone(),
                                style,
                            });
                        }

                        spans.push(StyledSpan {
                            text: format!("{} ", " ".repeat(pad_right)),
                            style: Style::default(),
                        });
                    } else {
                        // Empty line for this cell (other cells in the row are taller)
                        spans.push(StyledSpan {
                            text: format!(" {} ", " ".repeat(cw)),
                            style: Style::default(),
                        });
                    }

                    spans.push(StyledSpan {
                        text: "│".to_string(),
                        style: border_style.clone(),
                    });
                }
                self.lines.push(Line { spans });
            }

            // Separator after header and between body rows
            if row_idx + 1 < all_rows.len() {
                self.lines.push(make_rule("├", "┼", "┤", &col_widths));
            }
        }

        // Bottom border
        self.lines.push(make_rule("╰", "┴", "╯", &col_widths));
    }

    fn process(&mut self, event: Event) {
        match event {
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                self.flush_line();
                self.push_empty_line();
            }

            Event::Start(Tag::Heading { level, .. }) => {
                self.heading_level = Some(level);
            }
            Event::End(TagEnd::Heading(_)) => {
                self.flush_line();
                self.heading_level = None;
                self.push_empty_line();
            }

            Event::Start(Tag::Strong) => self.bold = true,
            Event::End(TagEnd::Strong) => self.bold = false,
            Event::Start(Tag::Emphasis) => self.italic = true,
            Event::End(TagEnd::Emphasis) => self.italic = false,
            Event::Start(Tag::Strikethrough) => self.strikethrough = true,
            Event::End(TagEnd::Strikethrough) => self.strikethrough = false,

            Event::Start(Tag::BlockQuote(_)) => {
                self.in_blockquote = true;
            }
            Event::End(TagEnd::BlockQuote) => {
                self.in_blockquote = false;
                self.push_empty_line();
            }

            Event::Start(Tag::CodeBlock(kind)) => {
                self.in_code_block = true;
                self.code_block_lang = match kind {
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code_block_content.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                self.emit_code_block();
                self.in_code_block = false;
                self.push_empty_line();
            }

            Event::Start(Tag::List(ordered)) => match ordered {
                Some(start) => self.list_stack.push(ListKind::Ordered(start)),
                None => self.list_stack.push(ListKind::Unordered),
            },
            Event::End(TagEnd::List(_)) => {
                self.list_stack.pop();
                if self.list_stack.is_empty() {
                    self.push_empty_line();
                }
            }

            Event::Start(Tag::Item) => {
                let depth = self.list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let bullet = match self.list_stack.last_mut() {
                    Some(ListKind::Unordered) => format!("{}  • ", indent),
                    Some(ListKind::Ordered(n)) => {
                        let num = *n;
                        *n += 1;
                        format!("{}  {}. ", indent, num)
                    }
                    None => String::new(),
                };
                self.push_span(
                    &bullet,
                    Style {
                        fg: Some(Color::Rgb { r: 120, g: 120, b: 120 }),
                        ..Default::default()
                    },
                );
            }
            Event::End(TagEnd::Item) => {
                self.flush_line();
            }

            Event::Start(Tag::Link { dest_url, .. }) => {
                self.in_link = true;
                self.link_url = dest_url.to_string();
            }
            Event::End(TagEnd::Link) => {
                let url = std::mem::take(&mut self.link_url);
                self.push_span(
                    &format!(" {}", url),
                    Style {
                        fg: Some(Color::Rgb { r: 90, g: 90, b: 90 }),
                        ..Default::default()
                    },
                );
                self.in_link = false;
            }

            Event::Start(Tag::Table(alignments)) => {
                self.in_table = true;
                self.table_alignments = alignments;
                self.table_head.clear();
                self.table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                self.emit_table();
                self.in_table = false;
                self.table_alignments.clear();
                self.table_head.clear();
                self.table_rows.clear();
                self.push_empty_line();
            }
            Event::Start(Tag::TableHead) => {
                self.in_table_head = true;
                self.table_current_row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                self.in_table_head = false;
                self.table_head = std::mem::take(&mut self.table_current_row);
            }
            Event::Start(Tag::TableRow) => {
                self.table_current_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
                self.table_rows.push(std::mem::take(&mut self.table_current_row));
            }
            Event::Start(Tag::TableCell) => {
                self.table_cell_spans.clear();
            }
            Event::End(TagEnd::TableCell) => {
                self.table_current_row.push(std::mem::take(&mut self.table_cell_spans));
            }

            Event::Text(text) => {
                if self.in_table {
                    let style = self.current_style();
                    self.table_cell_spans.push(StyledSpan {
                        text: text.to_string(),
                        style,
                    });
                } else if self.in_code_block {
                    self.code_block_content.push_str(&text);
                } else if self.in_link {
                    let mut style = self.current_style();
                    style.fg = Some(Color::Rgb { r: 120, g: 170, b: 240 });
                    style.underline = true;
                    self.push_span(&text, style);
                } else {
                    let style = self.current_style();
                    self.push_span(&text, style);
                }
            }

            Event::Code(code) => {
                let tick_style = Style {
                    fg: Some(Color::Rgb { r: 70, g: 70, b: 80 }),
                    bg: Some(Color::Rgb { r: 40, g: 42, b: 48 }),
                    ..Default::default()
                };
                let code_style = Style {
                    fg: Some(Color::Rgb { r: 230, g: 175, b: 110 }),
                    bg: Some(Color::Rgb { r: 40, g: 42, b: 48 }),
                    ..Default::default()
                };
                if self.in_table {
                    self.table_cell_spans.push(StyledSpan { text: "`".to_string(), style: tick_style.clone() });
                    self.table_cell_spans.push(StyledSpan { text: code.to_string(), style: code_style });
                    self.table_cell_spans.push(StyledSpan { text: "`".to_string(), style: tick_style });
                } else {
                    self.push_span("`", tick_style.clone());
                    self.push_span(&code, code_style);
                    self.push_span("`", tick_style);
                }
            }

            Event::SoftBreak => {
                let style = self.current_style();
                self.push_span(" ", style);
            }

            Event::HardBreak => {
                self.flush_line();
            }

            Event::Rule => {
                self.lines.push(Line {
                    spans: vec![StyledSpan {
                        text: "─".repeat(40),
                        style: Style {
                            fg: Some(Color::Rgb { r: 60, g: 60, b: 60 }),
                            ..Default::default()
                        },
                    }],
                });
                self.push_empty_line();
            }

            Event::TaskListMarker(checked) => {
                let (marker, color) = if checked {
                    ("✓ ", Color::Rgb { r: 120, g: 200, b: 120 })
                } else {
                    ("○ ", Color::Rgb { r: 100, g: 100, b: 100 })
                };
                self.push_span(
                    marker,
                    Style {
                        fg: Some(color),
                        ..Default::default()
                    },
                );
            }

            _ => {}
        }
    }
}

/// A wrapping unit: either a whitespace segment or a group of consecutive
/// non-whitespace segments (e.g. `` ` `` + `code` + `` ` `` stays together).
enum WrapUnit {
    Whitespace(StyledSpan),
    Word(Vec<StyledSpan>, usize), // segments, total char width
}

/// Wrap a cell's styled spans into multiple visual lines fitting within `width`.
/// Groups consecutive non-whitespace segments so backticks stay with their content.
fn wrap_cell(spans: &[StyledSpan], width: usize) -> Vec<Vec<StyledSpan>> {
    if width == 0 {
        return vec![spans.to_vec()];
    }

    let total: usize = spans.iter().map(|s| s.text.chars().count()).sum();
    if total <= width {
        return vec![spans.to_vec()];
    }

    // Split spans into word/whitespace segments
    let mut segments: Vec<StyledSpan> = Vec::new();
    for span in spans {
        let mut chars = span.text.chars().peekable();
        while chars.peek().is_some() {
            let is_ws = chars.peek().unwrap().is_whitespace();
            let mut text = String::new();
            while let Some(&ch) = chars.peek() {
                if ch.is_whitespace() != is_ws {
                    break;
                }
                text.push(ch);
                chars.next();
            }
            segments.push(StyledSpan {
                text,
                style: span.style.clone(),
            });
        }
    }

    // Group consecutive non-whitespace segments into word units
    let mut units: Vec<WrapUnit> = Vec::new();
    let mut word_segs: Vec<StyledSpan> = Vec::new();
    let mut word_width: usize = 0;

    for seg in segments {
        let is_ws = seg.text.starts_with(|c: char| c.is_whitespace());
        if is_ws {
            if !word_segs.is_empty() {
                units.push(WrapUnit::Word(
                    std::mem::take(&mut word_segs),
                    word_width,
                ));
                word_width = 0;
            }
            units.push(WrapUnit::Whitespace(seg));
        } else {
            word_width += seg.text.chars().count();
            word_segs.push(seg);
        }
    }
    if !word_segs.is_empty() {
        units.push(WrapUnit::Word(word_segs, word_width));
    }

    // Wrap using word units
    let mut lines: Vec<Vec<StyledSpan>> = Vec::new();
    let mut current: Vec<StyledSpan> = Vec::new();
    let mut col = 0;

    for unit in &units {
        match unit {
            WrapUnit::Whitespace(seg) => {
                if col == 0 && !lines.is_empty() {
                    continue; // skip leading whitespace on continuation lines
                }
                col += seg.text.chars().count();
                current.push(seg.clone());
            }
            WrapUnit::Word(segs, ww) => {
                // Would overflow: wrap to next line
                if col + ww > width && col > 0 {
                    // Remove trailing whitespace
                    if let Some(last) = current.last() {
                        if last.text.chars().all(|c| c.is_whitespace()) {
                            current.pop();
                        }
                    }
                    lines.push(std::mem::take(&mut current));
                    col = 0;
                }

                if *ww <= width {
                    // Word group fits on a line
                    for seg in segs {
                        col += seg.text.chars().count();
                        current.push(seg.clone());
                    }
                } else {
                    // Word group wider than column: character-level break
                    for seg in segs {
                        let chars: Vec<char> = seg.text.chars().collect();
                        let mut i = 0;
                        while i < chars.len() {
                            let avail = if col < width { width - col } else { width };
                            if col >= width {
                                lines.push(std::mem::take(&mut current));
                                col = 0;
                                continue;
                            }
                            let take = avail.min(chars.len() - i);
                            current.push(StyledSpan {
                                text: chars[i..i + take].iter().collect(),
                                style: seg.style.clone(),
                            });
                            col += take;
                            i += take;
                        }
                    }
                }
            }
        }
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }

    lines
}

fn syntect_to_style(syn: SynStyle) -> Style {
    Style {
        fg: Some(Color::Rgb {
            r: syn.foreground.r,
            g: syn.foreground.g,
            b: syn.foreground.b,
        }),
        bold: syn.font_style.contains(FontStyle::BOLD),
        italic: syn.font_style.contains(FontStyle::ITALIC),
        underline: syn.font_style.contains(FontStyle::UNDERLINE),
        ..Default::default()
    }
}

pub fn render(input: &str, width: usize) -> Vec<Line> {
    let mut renderer = Renderer::new(width);

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(input, options);

    for event in parser {
        renderer.process(event);
    }

    // Flush any remaining content
    renderer.flush_line();

    renderer.lines
}
