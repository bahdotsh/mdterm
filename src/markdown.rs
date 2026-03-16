use crossterm::style::Color;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use unicode_width::UnicodeWidthStr;

use crate::diagram;
use crate::style::{CodeBlockContent, DocumentInfo, Line, LineMeta, Style, StyledSpan};
use crate::theme::Theme;

struct Renderer<'a> {
    theme: &'a Theme,
    lines: Vec<Line>,
    current_spans: Vec<StyledSpan>,
    width: usize,
    line_numbers: bool,

    // Inline style state
    bold: bool,
    italic: bool,
    strikethrough: bool,

    // Block state
    heading_level: Option<HeadingLevel>,
    heading_text: String,
    in_blockquote: bool,
    in_code_block: bool,
    code_block_lang: String,
    code_block_content: String,
    code_block_id: usize,

    // List state
    list_stack: Vec<ListKind>,
    item_has_nested_list: bool,

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

    // Image state
    in_image: bool,
    image_url: String,
    image_alt: String,

    // Document info
    code_blocks: Vec<CodeBlockContent>,

    // Syntect (shared reference)
    syntax_set: &'a SyntaxSet,
    theme_set: &'a ThemeSet,
}

#[derive(Clone)]
enum ListKind {
    Unordered,
    Ordered(u64),
}

impl<'a> Renderer<'a> {
    fn new(
        width: usize,
        theme: &'a Theme,
        line_numbers: bool,
        syntax_set: &'a SyntaxSet,
        theme_set: &'a ThemeSet,
    ) -> Self {
        Renderer {
            theme,
            lines: Vec::new(),
            current_spans: Vec::new(),
            width,
            line_numbers,
            bold: false,
            italic: false,
            strikethrough: false,
            heading_level: None,
            heading_text: String::new(),
            in_blockquote: false,
            in_code_block: false,
            code_block_lang: String::new(),
            code_block_content: String::new(),
            code_block_id: 0,
            list_stack: Vec::new(),
            item_has_nested_list: false,
            in_table: false,
            table_alignments: Vec::new(),
            table_head: Vec::new(),
            table_rows: Vec::new(),
            table_cell_spans: Vec::new(),
            in_table_head: false,
            table_current_row: Vec::new(),
            in_link: false,
            link_url: String::new(),
            in_image: false,
            image_url: String::new(),
            image_alt: String::new(),
            code_blocks: Vec::new(),
            syntax_set,
            theme_set,
        }
    }

    fn current_style(&self) -> Style {
        let mut style = Style {
            fg: Some(self.theme.fg),
            ..Default::default()
        };

        if let Some(level) = self.heading_level {
            style.bold = true;
            match level {
                HeadingLevel::H1 => {
                    style.fg = Some(self.theme.h1);
                }
                HeadingLevel::H2 => {
                    style.fg = Some(self.theme.h2);
                }
                HeadingLevel::H3 => {
                    style.fg = Some(self.theme.h3);
                }
                HeadingLevel::H4 => {
                    style.fg = Some(self.theme.h4);
                    style.bold = false;
                }
                HeadingLevel::H5 => {
                    style.fg = Some(self.theme.h5);
                    style.bold = false;
                }
                HeadingLevel::H6 => {
                    style.fg = Some(self.theme.h6);
                    style.bold = false;
                    style.dim = true;
                }
            }
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
        self.flush_line_with_meta(LineMeta::None);
    }

    fn flush_line_with_meta(&mut self, meta: LineMeta) {
        if !self.current_spans.is_empty() {
            let mut spans = Vec::new();
            if self.in_blockquote {
                spans.push(StyledSpan {
                    text: "  ┃ ".to_string(),
                    style: Style {
                        fg: Some(self.theme.blockquote_bar),
                        ..Default::default()
                    },
                });
            }
            spans.append(&mut self.current_spans);
            self.lines.push(Line { spans, meta });
        }
    }

    fn push_empty_line(&mut self) {
        if let Some(last) = self.lines.last()
            && last.spans.is_empty()
        {
            return;
        }
        if self.in_blockquote {
            self.lines.push(Line {
                spans: vec![StyledSpan {
                    text: "  ┃".to_string(),
                    style: Style {
                        fg: Some(self.theme.blockquote_bar),
                        ..Default::default()
                    },
                }],
                meta: LineMeta::None,
            });
        } else {
            self.lines.push(Line::empty());
        }
    }

    fn emit_code_block(&mut self) {
        let lang = self.code_block_lang.trim().to_string();
        let code = std::mem::take(&mut self.code_block_content);
        let code_bg = self.theme.code_bg;
        let border_fg = self.theme.code_border;
        let label_fg = self.theme.code_label;
        let block_id = self.code_block_id;
        self.code_block_id += 1;

        // Save raw content for clipboard copy
        self.code_blocks.push(CodeBlockContent {
            language: lang.clone(),
            content: code.clone(),
        });

        // Check for special diagram blocks
        let is_diagram = matches!(lang.as_str(), "mermaid" | "plantuml" | "dot" | "graphviz");

        // Try to render mermaid diagrams visually
        if lang == "mermaid"
            && let Some((diagram_rows, diagram_width)) = diagram::render_mermaid(&code, self.theme)
        {
            self.emit_diagram_block(block_id, &diagram_rows, diagram_width);
            return;
        }

        let syntax = if lang.is_empty() {
            self.syntax_set.find_syntax_plain_text()
        } else {
            self.syntax_set
                .find_syntax_by_token(&lang)
                .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
        };

        let syntect_theme = self
            .theme_set
            .themes
            .get(self.theme.syntect_theme)
            .unwrap_or_else(|| &self.theme_set.themes["base16-ocean.dark"]);
        let mut highlighter = HighlightLines::new(syntax, syntect_theme);

        let code_lines: Vec<&str> = code.lines().collect();
        let line_num_width = if self.line_numbers {
            code_lines.len().to_string().len()
        } else {
            0
        };
        let max_line_len = code_lines
            .iter()
            .map(|l| UnicodeWidthStr::width(*l))
            .max()
            .unwrap_or(0);
        let content_width = (max_line_len
            + if self.line_numbers {
                line_num_width + 3
            } else {
                0
            })
        .max(40);
        let inner_width = content_width + 2;

        // Diagram label or language label
        let label = if is_diagram {
            format!(" {} (diagram) ", lang)
        } else if lang.is_empty() {
            String::new()
        } else {
            format!(" {} ", lang)
        };
        let label_len = UnicodeWidthStr::width(label.as_str());

        // Top border
        let dashes_after = inner_width.saturating_sub(1 + label_len);
        let mut top_spans = vec![StyledSpan {
            text: "  ╭─".to_string(),
            style: Style {
                fg: Some(border_fg),
                ..Default::default()
            },
        }];
        if !label.is_empty() {
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
        self.lines.push(Line {
            spans: top_spans,
            meta: LineMeta::CodeContent { block_id },
        });

        // Code lines
        for (line_num, line_str) in LinesWithEndings::from(&code).enumerate() {
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

            // Line numbers
            let mut char_count = 0;
            if self.line_numbers {
                let num_str = format!("{:>width$} │ ", line_num + 1, width = line_num_width);
                char_count += UnicodeWidthStr::width(num_str.as_str());
                spans.push(StyledSpan {
                    text: num_str,
                    style: Style {
                        fg: Some(self.theme.line_number),
                        bg: Some(code_bg),
                        ..Default::default()
                    },
                });
            }

            if let Ok(ranges) = highlighter.highlight_line(line_str, self.syntax_set) {
                for (syn_style, text) in ranges {
                    let trimmed = text.trim_end_matches('\n').trim_end_matches('\r');
                    if !trimmed.is_empty() {
                        char_count += UnicodeWidthStr::width(trimmed);
                        let mut style = syntect_to_style(syn_style);
                        style.bg = Some(code_bg);
                        spans.push(StyledSpan {
                            text: trimmed.to_string(),
                            style,
                        });
                    }
                }
            } else {
                let trimmed = line_str.trim_end_matches('\n').trim_end_matches('\r');
                char_count = UnicodeWidthStr::width(trimmed);
                spans.push(StyledSpan {
                    text: trimmed.to_string(),
                    style: Style {
                        bg: Some(code_bg),
                        ..Default::default()
                    },
                });
            }

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

            self.lines.push(Line {
                spans,
                meta: LineMeta::CodeContent { block_id },
            });
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
            meta: LineMeta::CodeContent { block_id },
        });
    }

    fn emit_diagram_block(
        &mut self,
        block_id: usize,
        diagram_rows: &[Vec<StyledSpan>],
        diagram_width: usize,
    ) {
        let border_fg = self.theme.code_border;
        let label_fg = self.theme.code_label;
        let code_bg = self.theme.code_bg;

        let content_width = diagram_width.max(40);
        let inner_width = content_width + 2;

        // Top border with label
        let label = " mermaid (diagram) ";
        let label_len = UnicodeWidthStr::width(label);
        let dashes_after = inner_width.saturating_sub(1 + label_len);
        self.lines.push(Line {
            spans: vec![
                StyledSpan {
                    text: "  ╭─".to_string(),
                    style: Style {
                        fg: Some(border_fg),
                        ..Default::default()
                    },
                },
                StyledSpan {
                    text: label.to_string(),
                    style: Style {
                        fg: Some(label_fg),
                        ..Default::default()
                    },
                },
                StyledSpan {
                    text: format!("{}╮", "─".repeat(dashes_after)),
                    style: Style {
                        fg: Some(border_fg),
                        ..Default::default()
                    },
                },
            ],
            meta: LineMeta::CodeContent { block_id },
        });

        // Diagram content rows
        for row_spans in diagram_rows {
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

            let row_width: usize = row_spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.text.as_str()))
                .sum();
            spans.extend(row_spans.iter().cloned());

            let padding = content_width.saturating_sub(row_width) + 1;
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

            self.lines.push(Line {
                spans,
                meta: LineMeta::CodeContent { block_id },
            });
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
            meta: LineMeta::CodeContent { block_id },
        });
    }

    fn emit_table(&mut self) {
        let border_fg = self.theme.table_border;
        let header_fg = self.theme.table_header;

        let all_rows: Vec<&Vec<Vec<StyledSpan>>> = std::iter::once(&self.table_head)
            .chain(self.table_rows.iter())
            .collect();

        let num_cols = self.table_alignments.len();
        if num_cols == 0 {
            return;
        }

        let mut col_widths = vec![0usize; num_cols];
        for row in &all_rows {
            for (i, cell) in row.iter().enumerate() {
                if i < num_cols {
                    let w: usize = cell
                        .iter()
                        .map(|s| UnicodeWidthStr::width(s.text.as_str()))
                        .sum();
                    col_widths[i] = col_widths[i].max(w);
                }
            }
        }

        let overhead = 3 + 3 * num_cols;
        let total_natural: usize = col_widths.iter().sum();
        let available = self.width.saturating_sub(overhead);

        if available > 0 && total_natural > available {
            let fair_share = available / num_cols;
            let mut fixed_width = 0usize;
            let flex_natural: usize = col_widths.iter().filter(|&&w| w > fair_share).sum();

            for &w in col_widths.iter() {
                if w <= fair_share {
                    fixed_width += w;
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

        for w in &mut col_widths {
            *w = (*w).max(3);
        }

        let border_style = Style {
            fg: Some(border_fg),
            ..Default::default()
        };

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
                meta: LineMeta::None,
            }
        };

        self.lines.push(make_rule("╭", "┬", "╮", &col_widths));

        for (row_idx, row) in all_rows.iter().enumerate() {
            let is_header = row_idx == 0;

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

                    let alignment = self
                        .table_alignments
                        .get(col_idx)
                        .unwrap_or(&Alignment::None);

                    if let Some(spans_in_line) = cell_line {
                        let content_width: usize = spans_in_line
                            .iter()
                            .map(|s| UnicodeWidthStr::width(s.text.as_str()))
                            .sum();
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
                self.lines.push(Line {
                    spans,
                    meta: LineMeta::None,
                });
            }

            if row_idx + 1 < all_rows.len() {
                self.lines.push(make_rule("├", "┼", "┤", &col_widths));
            }
        }

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
                if !self.lines.is_empty() {
                    if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) {
                        self.push_empty_line();
                        self.lines.push(Line {
                            spans: vec![StyledSpan {
                                text: "─".repeat(self.width.min(60)),
                                style: Style {
                                    fg: Some(self.theme.heading_separator),
                                    dim: true,
                                    ..Default::default()
                                },
                            }],
                            meta: LineMeta::None,
                        });
                        self.push_empty_line();
                    } else {
                        self.push_empty_line();
                    }
                }
                self.heading_level = Some(level);
                self.heading_text.clear();
                match level {
                    HeadingLevel::H3 => {
                        self.push_span(
                            "▸ ",
                            Style {
                                fg: Some(self.theme.h3),
                                dim: true,
                                ..Default::default()
                            },
                        );
                    }
                    HeadingLevel::H4 => {
                        self.push_span(
                            "  ▸ ",
                            Style {
                                fg: Some(self.theme.h4),
                                dim: true,
                                ..Default::default()
                            },
                        );
                    }
                    HeadingLevel::H5 => {
                        self.push_span(
                            "    ▸ ",
                            Style {
                                fg: Some(self.theme.h5),
                                dim: true,
                                ..Default::default()
                            },
                        );
                    }
                    HeadingLevel::H6 => {
                        self.push_span(
                            "      ▸ ",
                            Style {
                                fg: Some(self.theme.h6),
                                dim: true,
                                ..Default::default()
                            },
                        );
                    }
                    _ => {}
                }
            }
            Event::End(TagEnd::Heading(level)) => {
                let heading_text = std::mem::take(&mut self.heading_text);
                let lvl = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.flush_line_with_meta(LineMeta::Heading {
                    level: lvl,
                    text: heading_text,
                });
                if matches!(level, HeadingLevel::H1) {
                    let last_w = self.lines.last().map(|l| l.display_width()).unwrap_or(0);
                    if last_w > 0 {
                        self.lines.push(Line {
                            spans: vec![StyledSpan {
                                text: "━".repeat(last_w.min(self.width)),
                                style: Style {
                                    fg: Some(self.theme.h1),
                                    dim: true,
                                    ..Default::default()
                                },
                            }],
                            meta: LineMeta::None,
                        });
                    }
                }
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

            Event::Start(Tag::List(ordered)) => {
                self.flush_line();
                if !self.list_stack.is_empty() {
                    self.item_has_nested_list = true;
                }
                match ordered {
                    Some(start) => self.list_stack.push(ListKind::Ordered(start)),
                    None => self.list_stack.push(ListKind::Unordered),
                }
            }
            Event::End(TagEnd::List(_)) => {
                self.list_stack.pop();
                if self.list_stack.is_empty() {
                    self.push_empty_line();
                }
            }

            Event::Start(Tag::Item) => {
                self.item_has_nested_list = false;
                let depth = self.list_stack.len().saturating_sub(1);
                let indent = "    ".repeat(depth);
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
                        fg: Some(self.theme.bullet),
                        ..Default::default()
                    },
                );
            }
            Event::End(TagEnd::Item) => {
                self.flush_line();
                if self.list_stack.len() <= 1 && self.item_has_nested_list {
                    self.push_empty_line();
                }
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
                        fg: Some(self.theme.link_url),
                        link_url: Some(url),
                        ..Default::default()
                    },
                );
                self.in_link = false;
            }

            // Image handling
            Event::Start(Tag::Image { dest_url, .. }) => {
                self.in_image = true;
                self.image_url = dest_url.to_string();
                self.image_alt.clear();
            }
            Event::End(TagEnd::Image) => {
                let alt = if self.image_alt.is_empty() {
                    "image".to_string()
                } else {
                    std::mem::take(&mut self.image_alt)
                };
                let url = std::mem::take(&mut self.image_url);

                // Flush any pending content
                self.flush_line();

                let total_rows = crate::image::IMAGE_ROWS;

                // Push placeholder lines for the image
                for row in 0..total_rows {
                    self.lines.push(Line {
                        spans: vec![],
                        meta: LineMeta::Image {
                            url: url.clone(),
                            alt: alt.clone(),
                            row,
                            total_rows,
                        },
                    });
                }

                // Caption line below the image
                self.push_span(
                    &format!("  {}", alt),
                    Style {
                        fg: Some(self.theme.image_fg),
                        dim: true,
                        italic: true,
                        link_url: if url.is_empty() {
                            None
                        } else {
                            Some(url.clone())
                        },
                        ..Default::default()
                    },
                );
                self.flush_line();

                self.in_image = false;
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
                self.table_rows
                    .push(std::mem::take(&mut self.table_current_row));
            }
            Event::Start(Tag::TableCell) => {
                self.table_cell_spans.clear();
            }
            Event::End(TagEnd::TableCell) => {
                self.table_current_row
                    .push(std::mem::take(&mut self.table_cell_spans));
            }

            Event::Text(text) => {
                if self.in_image {
                    self.image_alt.push_str(&text);
                } else if self.in_table {
                    let style = self.current_style();
                    self.table_cell_spans.push(StyledSpan {
                        text: text.to_string(),
                        style,
                    });
                } else if self.in_code_block {
                    self.code_block_content.push_str(&text);
                } else if self.in_link {
                    let mut style = self.current_style();
                    style.fg = Some(self.theme.link);
                    style.underline = true;
                    style.link_url = Some(self.link_url.clone());
                    self.push_span(&text, style);
                    if self.heading_level.is_some() {
                        self.heading_text.push_str(&text);
                    }
                } else {
                    if self.heading_level.is_some() {
                        self.heading_text.push_str(&text);
                    }
                    let style = self.current_style();
                    self.push_span(&text, style);
                }
            }

            Event::Code(code) => {
                if self.heading_level.is_some() {
                    self.heading_text.push_str(&code);
                }
                let tick_style = Style {
                    fg: Some(self.theme.inline_code_tick),
                    bg: Some(self.theme.inline_code_bg),
                    ..Default::default()
                };
                let code_style = Style {
                    fg: Some(self.theme.inline_code_fg),
                    bg: Some(self.theme.inline_code_bg),
                    ..Default::default()
                };
                if self.in_table {
                    self.table_cell_spans.push(StyledSpan {
                        text: "`".to_string(),
                        style: tick_style.clone(),
                    });
                    self.table_cell_spans.push(StyledSpan {
                        text: code.to_string(),
                        style: code_style,
                    });
                    self.table_cell_spans.push(StyledSpan {
                        text: "`".to_string(),
                        style: tick_style,
                    });
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
                            fg: Some(self.theme.rule),
                            ..Default::default()
                        },
                    }],
                    meta: LineMeta::SlideBreak,
                });
                self.push_empty_line();
            }

            Event::TaskListMarker(checked) => {
                let (marker, color) = if checked {
                    ("✓ ", self.theme.task_done)
                } else {
                    ("○ ", self.theme.task_pending)
                };
                self.push_span(
                    marker,
                    Style {
                        fg: Some(color),
                        ..Default::default()
                    },
                );
            }

            // Math rendering
            Event::InlineMath(math) => {
                let rendered = render_math(&math);
                self.push_span(
                    &rendered,
                    Style {
                        fg: Some(self.theme.math_fg),
                        ..Default::default()
                    },
                );
            }
            Event::DisplayMath(math) => {
                self.flush_line();
                let rendered = render_math(&math);
                for math_line in rendered.lines() {
                    self.push_span(
                        &format!("    {}", math_line),
                        Style {
                            fg: Some(self.theme.math_fg),
                            ..Default::default()
                        },
                    );
                    self.flush_line();
                }
                self.push_empty_line();
            }

            _ => {}
        }
    }
}

/// Word-wrap a table cell
fn wrap_cell(spans: &[StyledSpan], width: usize) -> Vec<Vec<StyledSpan>> {
    if width == 0 {
        return vec![spans.to_vec()];
    }

    let total: usize = spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.text.as_str()))
        .sum();
    if total <= width {
        return vec![spans.to_vec()];
    }

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

    enum WrapUnit {
        Whitespace(StyledSpan),
        Word(Vec<StyledSpan>, usize),
    }

    let mut units: Vec<WrapUnit> = Vec::new();
    let mut word_segs: Vec<StyledSpan> = Vec::new();
    let mut word_width: usize = 0;

    for seg in segments {
        let is_ws = seg.text.starts_with(|c: char| c.is_whitespace());
        if is_ws {
            if !word_segs.is_empty() {
                units.push(WrapUnit::Word(std::mem::take(&mut word_segs), word_width));
                word_width = 0;
            }
            units.push(WrapUnit::Whitespace(seg));
        } else {
            word_width += UnicodeWidthStr::width(seg.text.as_str());
            word_segs.push(seg);
        }
    }
    if !word_segs.is_empty() {
        units.push(WrapUnit::Word(word_segs, word_width));
    }

    let mut lines: Vec<Vec<StyledSpan>> = Vec::new();
    let mut current: Vec<StyledSpan> = Vec::new();
    let mut col = 0;

    for unit in &units {
        match unit {
            WrapUnit::Whitespace(seg) => {
                if col == 0 && !lines.is_empty() {
                    continue;
                }
                col += UnicodeWidthStr::width(seg.text.as_str());
                current.push(seg.clone());
            }
            WrapUnit::Word(segs, ww) => {
                if col + ww > width && col > 0 {
                    if let Some(last) = current.last()
                        && last.text.chars().all(|c| c.is_whitespace())
                    {
                        current.pop();
                    }
                    lines.push(std::mem::take(&mut current));
                    col = 0;
                }

                if *ww <= width {
                    for seg in segs {
                        col += UnicodeWidthStr::width(seg.text.as_str());
                        current.push(seg.clone());
                    }
                } else {
                    for seg in segs {
                        let chars: Vec<char> = seg.text.chars().collect();
                        let mut i = 0;
                        while i < chars.len() {
                            if col >= width {
                                lines.push(std::mem::take(&mut current));
                                col = 0;
                                continue;
                            }
                            let mut chunk = String::new();
                            let mut chunk_w = 0;
                            while i < chars.len() {
                                let cw =
                                    unicode_width::UnicodeWidthChar::width(chars[i]).unwrap_or(0);
                                if chunk_w + cw > width - col && chunk_w > 0 {
                                    break;
                                }
                                chunk.push(chars[i]);
                                chunk_w += cw;
                                i += 1;
                            }
                            col += chunk_w;
                            current.push(StyledSpan {
                                text: chunk,
                                style: seg.style.clone(),
                            });
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

/// Convert basic LaTeX math to Unicode approximation
pub fn render_math(latex: &str) -> String {
    let mut s = latex.to_string();

    let replacements = [
        // Greek lowercase
        ("\\alpha", "α"),
        ("\\beta", "β"),
        ("\\gamma", "γ"),
        ("\\delta", "δ"),
        ("\\epsilon", "ε"),
        ("\\varepsilon", "ε"),
        ("\\zeta", "ζ"),
        ("\\eta", "η"),
        ("\\theta", "θ"),
        ("\\iota", "ι"),
        ("\\kappa", "κ"),
        ("\\lambda", "λ"),
        ("\\mu", "μ"),
        ("\\nu", "ν"),
        ("\\xi", "ξ"),
        ("\\pi", "π"),
        ("\\rho", "ρ"),
        ("\\sigma", "σ"),
        ("\\tau", "τ"),
        ("\\upsilon", "υ"),
        ("\\phi", "φ"),
        ("\\varphi", "φ"),
        ("\\chi", "χ"),
        ("\\psi", "ψ"),
        ("\\omega", "ω"),
        // Greek uppercase
        ("\\Gamma", "Γ"),
        ("\\Delta", "Δ"),
        ("\\Theta", "Θ"),
        ("\\Lambda", "Λ"),
        ("\\Xi", "Ξ"),
        ("\\Pi", "Π"),
        ("\\Sigma", "Σ"),
        ("\\Phi", "Φ"),
        ("\\Psi", "Ψ"),
        ("\\Omega", "Ω"),
        // Operators
        ("\\sum", "∑"),
        ("\\prod", "∏"),
        ("\\int", "∫"),
        ("\\iint", "∬"),
        ("\\iiint", "∭"),
        ("\\oint", "∮"),
        ("\\infty", "∞"),
        ("\\partial", "∂"),
        ("\\nabla", "∇"),
        ("\\pm", "±"),
        ("\\mp", "∓"),
        ("\\times", "×"),
        ("\\div", "÷"),
        ("\\cdot", "·"),
        ("\\circ", "∘"),
        ("\\bullet", "•"),
        ("\\star", "⋆"),
        // Relations
        ("\\leq", "≤"),
        ("\\geq", "≥"),
        ("\\neq", "≠"),
        ("\\approx", "≈"),
        ("\\equiv", "≡"),
        ("\\sim", "∼"),
        ("\\simeq", "≃"),
        ("\\cong", "≅"),
        ("\\propto", "∝"),
        ("\\ll", "≪"),
        ("\\gg", "≫"),
        // Set theory
        ("\\subset", "⊂"),
        ("\\supset", "⊃"),
        ("\\subseteq", "⊆"),
        ("\\supseteq", "⊇"),
        ("\\in", "∈"),
        ("\\notin", "∉"),
        ("\\cup", "∪"),
        ("\\cap", "∩"),
        ("\\emptyset", "∅"),
        ("\\varnothing", "∅"),
        // Logic
        ("\\forall", "∀"),
        ("\\exists", "∃"),
        ("\\nexists", "∄"),
        ("\\neg", "¬"),
        ("\\land", "∧"),
        ("\\lor", "∨"),
        ("\\implies", "⟹"),
        ("\\iff", "⟺"),
        // Arrows
        ("\\rightarrow", "→"),
        ("\\leftarrow", "←"),
        ("\\Rightarrow", "⇒"),
        ("\\Leftarrow", "⇐"),
        ("\\leftrightarrow", "↔"),
        ("\\Leftrightarrow", "⇔"),
        ("\\uparrow", "↑"),
        ("\\downarrow", "↓"),
        ("\\mapsto", "↦"),
        ("\\to", "→"),
        // Misc
        ("\\sqrt", "√"),
        ("\\ldots", "…"),
        ("\\cdots", "⋯"),
        ("\\vdots", "⋮"),
        ("\\ddots", "⋱"),
        ("\\langle", "⟨"),
        ("\\rangle", "⟩"),
        ("\\lfloor", "⌊"),
        ("\\rfloor", "⌋"),
        ("\\lceil", "⌈"),
        ("\\rceil", "⌉"),
        ("\\|", "‖"),
        ("\\{", "{"),
        ("\\}", "}"),
        ("\\,", " "),
        ("\\;", " "),
        ("\\!", ""),
        ("\\quad", "  "),
        ("\\qquad", "    "),
    ];

    // Apply longest matches first (already sorted by length within groups)
    for (from, to) in &replacements {
        s = s.replace(from, to);
    }

    // Superscript digits
    s = s
        .replace("^{0}", "⁰")
        .replace("^{1}", "¹")
        .replace("^{2}", "²")
        .replace("^{3}", "³")
        .replace("^{4}", "⁴")
        .replace("^{5}", "⁵")
        .replace("^{6}", "⁶")
        .replace("^{7}", "⁷")
        .replace("^{8}", "⁸")
        .replace("^{9}", "⁹")
        .replace("^{n}", "ⁿ")
        .replace("^{i}", "ⁱ")
        .replace("^{+}", "⁺")
        .replace("^{-}", "⁻")
        .replace("^{=}", "⁼")
        .replace("^{(}", "⁽")
        .replace("^{)}", "⁾");

    // Single-char superscripts
    s = s
        .replace("^0", "⁰")
        .replace("^1", "¹")
        .replace("^2", "²")
        .replace("^3", "³")
        .replace("^4", "⁴")
        .replace("^5", "⁵")
        .replace("^6", "⁶")
        .replace("^7", "⁷")
        .replace("^8", "⁸")
        .replace("^9", "⁹")
        .replace("^n", "ⁿ")
        .replace("^i", "ⁱ");

    // Subscript digits
    s = s
        .replace("_{0}", "₀")
        .replace("_{1}", "₁")
        .replace("_{2}", "₂")
        .replace("_{3}", "₃")
        .replace("_{4}", "₄")
        .replace("_{5}", "₅")
        .replace("_{6}", "₆")
        .replace("_{7}", "₇")
        .replace("_{8}", "₈")
        .replace("_{9}", "₉")
        .replace("_{i}", "ᵢ")
        .replace("_{j}", "ⱼ")
        .replace("_{n}", "ₙ");

    // Single-char subscripts
    s = s
        .replace("_0", "₀")
        .replace("_1", "₁")
        .replace("_2", "₂")
        .replace("_3", "₃")
        .replace("_4", "₄")
        .replace("_5", "₅")
        .replace("_6", "₆")
        .replace("_7", "₇")
        .replace("_8", "₈")
        .replace("_9", "₉");

    // Simple \frac{a}{b} -> a/b
    while let Some(idx) = s.find("\\frac{") {
        let after = &s[idx + 6..];
        if let Some(close1) = after.find('}') {
            let numer = &after[..close1];
            let rest = &after[close1 + 1..];
            if rest.starts_with('{')
                && let Some(close2) = rest[1..].find('}')
            {
                let denom = &rest[1..1 + close2];
                let end_pos = idx + 6 + close1 + 1 + 1 + close2 + 1;
                s = format!("{}{}/{}{}", &s[..idx], numer, denom, &s[end_pos..]);
                continue;
            }
        }
        break;
    }

    // Clean up remaining \text{...} -> ...
    while let Some(idx) = s.find("\\text{") {
        let after = &s[idx + 6..];
        if let Some(close) = after.find('}') {
            let text = &after[..close];
            let end_pos = idx + 6 + close + 1;
            s = format!("{}{}{}", &s[..idx], text, &s[end_pos..]);
            continue;
        }
        break;
    }

    // Remove remaining curly braces that were just grouping
    s = s.replace(['{', '}'], "");

    s
}

/// Pre-loaded syntect resources to avoid re-loading on every render.
pub struct SyntectRes {
    pub syntax_set: SyntaxSet,
    pub theme_set: ThemeSet,
}

impl SyntectRes {
    pub fn load() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
        }
    }
}

pub fn render(
    input: &str,
    width: usize,
    theme: &Theme,
    line_numbers: bool,
) -> (Vec<Line>, DocumentInfo) {
    let res = SyntectRes::load();
    render_with(input, width, theme, line_numbers, &res)
}

pub fn render_with(
    input: &str,
    width: usize,
    theme: &Theme,
    line_numbers: bool,
    syntect_res: &SyntectRes,
) -> (Vec<Line>, DocumentInfo) {
    let mut renderer = Renderer::new(
        width,
        theme,
        line_numbers,
        &syntect_res.syntax_set,
        &syntect_res.theme_set,
    );

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_MATH);

    let parser = Parser::new_ext(input, options);

    for event in parser {
        renderer.process(event);
    }

    renderer.flush_line();

    let doc_info = DocumentInfo {
        code_blocks: renderer.code_blocks,
    };

    (renderer.lines, doc_info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::LineMeta;
    use crate::theme::Theme;

    fn render_test(input: &str) -> (Vec<Line>, DocumentInfo) {
        let theme = Theme::dark();
        render(input, 80, &theme, false)
    }

    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.text.as_str()).collect()
    }

    // ── Headings ────────────────────────────────────────────────────────────

    #[test]
    fn h1_produces_heading_meta() {
        let (lines, _) = render_test("# Hello");
        let heading = lines
            .iter()
            .find(|l| matches!(&l.meta, LineMeta::Heading { level: 1, .. }));
        let heading = heading.expect("expected LineMeta::Heading level 1");
        let LineMeta::Heading { text, .. } = &heading.meta else {
            panic!("expected LineMeta::Heading");
        };
        assert_eq!(text, "Hello");
    }

    #[test]
    fn h2_produces_heading_meta() {
        let (lines, _) = render_test("## Section");
        let heading = lines
            .iter()
            .find(|l| matches!(&l.meta, LineMeta::Heading { level: 2, .. }));
        let heading = heading.expect("expected LineMeta::Heading level 2");
        let LineMeta::Heading { text, .. } = &heading.meta else {
            panic!("expected LineMeta::Heading");
        };
        assert_eq!(text, "Section");
    }

    #[test]
    fn multiple_heading_levels() {
        let (lines, _) = render_test("# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6");
        let levels: Vec<u8> = lines
            .iter()
            .filter_map(|l| match &l.meta {
                LineMeta::Heading { level, .. } => Some(*level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, vec![1, 2, 3, 4, 5, 6]);
    }

    // ── Code blocks ─────────────────────────────────────────────────────────

    #[test]
    fn code_block_tracked_in_document_info() {
        let input = "```rust\nfn main() {}\n```";
        let (_, doc_info) = render_test(input);
        assert_eq!(doc_info.code_blocks.len(), 1);
        assert_eq!(doc_info.code_blocks[0].language, "rust");
        assert!(doc_info.code_blocks[0].content.contains("fn main()"));
    }

    #[test]
    fn code_block_lines_have_in_code_block_meta() {
        let input = "```\nhello\nworld\n```";
        let (lines, _) = render_test(input);
        let code_lines: Vec<_> = lines
            .iter()
            .filter(|l| matches!(l.meta, LineMeta::CodeContent { .. }))
            .collect();
        assert!(!code_lines.is_empty(), "expected CodeContent meta lines");
    }

    #[test]
    fn multiple_code_blocks_tracked() {
        let input = "```python\nprint(1)\n```\n\n```js\nconsole.log(2)\n```";
        let (_, doc_info) = render_test(input);
        assert_eq!(doc_info.code_blocks.len(), 2);
        assert_eq!(doc_info.code_blocks[0].language, "python");
        assert_eq!(doc_info.code_blocks[1].language, "js");
    }

    // ── Image placeholders ──────────────────────────────────────────────────

    #[test]
    fn image_produces_placeholder_lines() {
        let input = "![alt text](http://example.com/img.png)";
        let (lines, _) = render_test(input);
        let image_lines: Vec<_> = lines
            .iter()
            .filter(|l| matches!(&l.meta, LineMeta::Image { .. }))
            .collect();
        assert_eq!(
            image_lines.len(),
            crate::image::IMAGE_ROWS,
            "expected IMAGE_ROWS placeholder lines"
        );
        // Check URL and alt are propagated
        let LineMeta::Image {
            url,
            alt,
            row,
            total_rows,
        } = &image_lines[0].meta
        else {
            panic!("expected LineMeta::Image");
        };
        assert_eq!(url, "http://example.com/img.png");
        assert_eq!(alt, "alt text");
        assert_eq!(*row, 0);
        assert_eq!(*total_rows, crate::image::IMAGE_ROWS);
    }

    #[test]
    fn image_without_alt_gets_default() {
        let input = "![](http://example.com/img.png)";
        let (lines, _) = render_test(input);
        let image_line = lines
            .iter()
            .find(|l| matches!(&l.meta, LineMeta::Image { .. }));
        let image_line = image_line.expect("expected LineMeta::Image");
        let LineMeta::Image { alt, .. } = &image_line.meta else {
            panic!("expected LineMeta::Image");
        };
        assert_eq!(alt, "image");
    }

    // ── Lists ───────────────────────────────────────────────────────────────

    #[test]
    fn unordered_list_has_bullets() {
        let input = "- item one\n- item two";
        let (lines, _) = render_test(input);
        let texts: Vec<String> = lines.iter().map(|l| line_text(l)).collect();
        let bullet_lines: Vec<_> = texts.iter().filter(|t| t.contains('•')).collect();
        assert_eq!(bullet_lines.len(), 2);
    }

    #[test]
    fn ordered_list_has_numbers() {
        let input = "1. first\n2. second";
        let (lines, _) = render_test(input);
        let texts: Vec<String> = lines.iter().map(|l| line_text(l)).collect();
        assert!(texts.iter().any(|t| t.contains("1.")));
        assert!(texts.iter().any(|t| t.contains("2.")));
    }

    // ── Blockquotes ─────────────────────────────────────────────────────────

    #[test]
    fn blockquote_produces_styled_output() {
        let input = "> quoted text";
        let (lines, _) = render_test(input);
        let texts: Vec<String> = lines.iter().map(|l| line_text(l)).collect();
        assert!(texts.iter().any(|t| t.contains("quoted text")));
    }

    // ── Inline math ─────────────────────────────────────────────────────────

    #[test]
    fn render_math_basic_symbols() {
        let result = render_math("\\alpha + \\beta");
        assert!(result.contains('α'));
        assert!(result.contains('β'));
    }

    #[test]
    fn render_math_fractions() {
        let result = render_math("\\frac{a}{b}");
        assert_eq!(result, "a/b");
    }

    // ── Line numbers ────────────────────────────────────────────────────────

    #[test]
    fn line_numbers_enabled_adds_numbers_in_code_blocks() {
        let theme = Theme::dark();
        let input = "```\nfirst\nsecond\nthird\n```";
        let (lines_with, _) = render(input, 80, &theme, true);
        let (lines_without, _) = render(input, 80, &theme, false);
        // With line numbers, code block lines should contain "1", "2", "3"
        let code_text: String = lines_with
            .iter()
            .filter(|l| matches!(l.meta, LineMeta::CodeContent { .. }))
            .flat_map(|l| l.spans.iter().map(|s| s.text.as_str()))
            .collect();
        assert!(code_text.contains("1"), "expected line number 1");
        assert!(code_text.contains("2"), "expected line number 2");
        assert!(code_text.contains("3"), "expected line number 3");
        // With line numbers enabled, code lines should have more spans (the number prefix)
        let spans_with: usize = lines_with
            .iter()
            .filter(|l| matches!(l.meta, LineMeta::CodeContent { .. }))
            .map(|l| l.spans.len())
            .sum();
        let spans_without: usize = lines_without
            .iter()
            .filter(|l| matches!(l.meta, LineMeta::CodeContent { .. }))
            .map(|l| l.spans.len())
            .sum();
        assert!(
            spans_with > spans_without,
            "line numbers should add extra spans"
        );
    }
}
