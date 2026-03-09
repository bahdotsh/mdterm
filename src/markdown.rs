use crossterm::style::Color;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::style::{Line, Style, StyledSpan};

struct Renderer {
    lines: Vec<Line>,
    current_spans: Vec<StyledSpan>,

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
    fn new() -> Self {
        Renderer {
            lines: Vec::new(),
            current_spans: Vec::new(),
            bold: false,
            italic: false,
            strikethrough: false,
            heading_level: None,
            in_blockquote: false,
            in_code_block: false,
            code_block_lang: String::new(),
            code_block_content: String::new(),
            list_stack: Vec::new(),
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
                HeadingLevel::H1 => Color::Cyan,
                HeadingLevel::H2 => Color::Green,
                HeadingLevel::H3 => Color::Yellow,
                HeadingLevel::H4 => Color::Magenta,
                HeadingLevel::H5 => Color::Blue,
                HeadingLevel::H6 => Color::Red,
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
            style.dim = true;
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
                    text: "  │ ".to_string(),
                    style: Style {
                        fg: Some(Color::DarkGrey),
                        ..Default::default()
                    },
                });
            }
            spans.append(&mut self.current_spans);
            self.lines.push(Line { spans });
        }
    }

    fn push_empty_line(&mut self) {
        if self.in_blockquote {
            self.lines.push(Line {
                spans: vec![StyledSpan {
                    text: "  │".to_string(),
                    style: Style {
                        fg: Some(Color::DarkGrey),
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

        let syntax = if lang.is_empty() {
            self.syntax_set.find_syntax_plain_text()
        } else {
            self.syntax_set
                .find_syntax_by_token(&lang)
                .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
        };

        let theme = &self.theme_set.themes["base16-ocean.dark"];
        let mut highlighter = HighlightLines::new(syntax, theme);

        // Top border
        let label = if lang.is_empty() {
            String::new()
        } else {
            format!(" {} ", lang)
        };
        let border_width = 60usize.saturating_sub(label.len() + 4);
        self.lines.push(Line {
            spans: vec![StyledSpan {
                text: format!("  ╭─{}{}╮", label, "─".repeat(border_width)),
                style: Style {
                    fg: Some(Color::DarkGrey),
                    ..Default::default()
                },
            }],
        });

        for line_str in LinesWithEndings::from(&code) {
            let mut spans = vec![StyledSpan {
                text: "  │ ".to_string(),
                style: Style {
                    fg: Some(Color::DarkGrey),
                    ..Default::default()
                },
            }];

            if let Ok(ranges) = highlighter.highlight_line(line_str, &self.syntax_set) {
                for (syn_style, text) in ranges {
                    let trimmed = text.trim_end_matches('\n').trim_end_matches('\r');
                    if !trimmed.is_empty() {
                        spans.push(StyledSpan {
                            text: trimmed.to_string(),
                            style: syntect_to_style(syn_style),
                        });
                    }
                }
            } else {
                spans.push(StyledSpan {
                    text: line_str
                        .trim_end_matches('\n')
                        .trim_end_matches('\r')
                        .to_string(),
                    style: Style::default(),
                });
            }

            self.lines.push(Line { spans });
        }

        // Bottom border
        self.lines.push(Line {
            spans: vec![StyledSpan {
                text: format!("  ╰{}╯", "─".repeat(border_width + label.len() + 1)),
                style: Style {
                    fg: Some(Color::DarkGrey),
                    ..Default::default()
                },
            }],
        });
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
            Event::End(TagEnd::Heading(level)) => {
                self.flush_line();
                self.heading_level = None;
                match level {
                    HeadingLevel::H1 => {
                        let w = self.lines.last().map(|l| l.display_width()).unwrap_or(20);
                        self.lines.push(Line {
                            spans: vec![StyledSpan {
                                text: "═".repeat(w.max(20)),
                                style: Style {
                                    fg: Some(Color::Cyan),
                                    bold: true,
                                    ..Default::default()
                                },
                            }],
                        });
                    }
                    HeadingLevel::H2 => {
                        let w = self.lines.last().map(|l| l.display_width()).unwrap_or(20);
                        self.lines.push(Line {
                            spans: vec![StyledSpan {
                                text: "─".repeat(w.max(20)),
                                style: Style {
                                    fg: Some(Color::Green),
                                    ..Default::default()
                                },
                            }],
                        });
                    }
                    _ => {}
                }
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
                        fg: Some(Color::Cyan),
                        bold: true,
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
                    &format!(" ({})", url),
                    Style {
                        fg: Some(Color::DarkGrey),
                        ..Default::default()
                    },
                );
                self.in_link = false;
            }

            Event::Text(text) => {
                if self.in_code_block {
                    self.code_block_content.push_str(&text);
                } else if self.in_link {
                    let mut style = self.current_style();
                    style.fg = Some(Color::Blue);
                    style.underline = true;
                    self.push_span(&text, style);
                } else {
                    let style = self.current_style();
                    self.push_span(&text, style);
                }
            }

            Event::Code(code) => {
                self.push_span(
                    &format!(" {} ", code),
                    Style {
                        fg: Some(Color::Yellow),
                        ..Default::default()
                    },
                );
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
                        text: "─".repeat(60),
                        style: Style {
                            fg: Some(Color::DarkGrey),
                            ..Default::default()
                        },
                    }],
                });
                self.push_empty_line();
            }

            Event::TaskListMarker(checked) => {
                let marker = if checked { "☑ " } else { "☐ " };
                self.push_span(
                    marker,
                    Style {
                        fg: Some(Color::Cyan),
                        ..Default::default()
                    },
                );
            }

            _ => {}
        }
    }
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

pub fn render(input: &str) -> Vec<Line> {
    let mut renderer = Renderer::new();

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
