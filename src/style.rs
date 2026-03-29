use crossterm::style::Color;
use unicode_width::UnicodeWidthStr;

pub const BLOCKQUOTE_PREFIX: &str = "  ┃ ";
pub const BLOCKQUOTE_PREFIX_TRIMMED: &str = "  ┃";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub dim: bool,
    pub link_url: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StyledSpan {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Debug, Default)]
pub enum LineMeta {
    #[default]
    None,
    Heading {
        level: u8,
        text: String,
    },
    CodeContent {
        block_id: usize,
    },
    ListItem {
        list_id: usize,
    },
    TaskItem {
        list_id: usize,
        checked: bool,
        task_index: usize,
    },
    SlideBreak,
    #[allow(dead_code)]
    Image {
        url: String,
        alt: String,
        row: usize,
        total_rows: usize,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub spans: Vec<StyledSpan>,
    pub meta: LineMeta,
}

impl Line {
    pub fn empty() -> Self {
        Line {
            spans: vec![],
            meta: LineMeta::None,
        }
    }

    pub fn display_width(&self) -> usize {
        self.spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.text.as_str()))
            .sum()
    }
}

/// Raw code block content for clipboard copy
#[allow(dead_code)]
pub struct CodeBlockContent {
    pub language: String,
    pub content: String,
}

/// Metadata returned alongside rendered lines
pub struct DocumentInfo {
    pub code_blocks: Vec<CodeBlockContent>,
}

pub fn wrap_lines(lines: &[Line], width: usize) -> Vec<Line> {
    if width == 0 {
        return lines.to_vec();
    }
    let mut result = Vec::new();
    for line in lines {
        if line.spans.is_empty() || line.display_width() <= width {
            result.push(line.clone());
        } else if line
            .spans
            .first()
            .is_some_and(|s| s.text == BLOCKQUOTE_PREFIX || s.text == BLOCKQUOTE_PREFIX_TRIMMED)
        {
            // Strip the blockquote prefix span, wrap at reduced width,
            // then re-add the prefix to all wrapped lines.
            let prefix_span = line.spans[0].clone();
            let prefix_width = UnicodeWidthStr::width(BLOCKQUOTE_PREFIX);
            let inner_width = width.saturating_sub(prefix_width);
            if inner_width == 0 {
                result.push(line.clone());
                continue;
            }
            let content_line = Line {
                spans: line.spans.iter().skip(1).cloned().collect(),
                meta: LineMeta::None,
            };
            let wrapped = word_wrap(&content_line, inner_width);
            let prefix_span = StyledSpan {
                text: BLOCKQUOTE_PREFIX.to_string(),
                style: prefix_span.style,
            };
            for mut w in wrapped {
                w.spans.insert(0, prefix_span.clone());
                w.meta = line.meta.clone();
                result.push(w);
            }
        } else {
            let mut wrapped = word_wrap(line, width);
            // Propagate metadata to all wrapped lines for clickable types
            // (ListItem, Heading) so click-to-copy works on continuation lines.
            // Other types only propagate to the first line.
            let propagate_all = matches!(
                line.meta,
                LineMeta::ListItem { .. } | LineMeta::TaskItem { .. } | LineMeta::Heading { .. }
            );
            if propagate_all {
                for w in &mut wrapped {
                    w.meta = line.meta.clone();
                }
            } else if let Some(first) = wrapped.first_mut() {
                first.meta = line.meta.clone();
            }
            result.extend(wrapped);
        }
    }
    result
}

fn word_wrap(line: &Line, width: usize) -> Vec<Line> {
    let mut segments: Vec<StyledSpan> = Vec::new();
    for span in &line.spans {
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

    let mut lines = Vec::new();
    let mut current: Vec<StyledSpan> = Vec::new();
    let mut col: usize = 0;

    for seg in &segments {
        let seg_width = UnicodeWidthStr::width(seg.text.as_str());
        let is_ws = seg
            .text
            .chars()
            .next()
            .map(|c| c.is_whitespace())
            .unwrap_or(false);

        if !is_ws && col + seg_width > width && col > 0 {
            if let Some(last) = current.last()
                && last.text.chars().all(|c| c.is_whitespace())
            {
                current.pop();
            }
            lines.push(Line {
                spans: std::mem::take(&mut current),
                meta: LineMeta::None,
            });
            col = 0;
        }

        if col == 0 && is_ws && !lines.is_empty() {
            continue;
        }

        if !is_ws && seg_width > width && col == 0 {
            let chars: Vec<char> = seg.text.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let mut chunk = String::new();
                let mut chunk_w = 0;
                while i < chars.len() {
                    let cw = unicode_width::UnicodeWidthChar::width(chars[i]).unwrap_or(0);
                    if chunk_w + cw > width && chunk_w > 0 {
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
                if col >= width && i < chars.len() {
                    lines.push(Line {
                        spans: std::mem::take(&mut current),
                        meta: LineMeta::None,
                    });
                    col = 0;
                }
            }
            continue;
        }

        col += seg_width;
        current.push(StyledSpan {
            text: seg.text.clone(),
            style: seg.style.clone(),
        });
    }

    if !current.is_empty() {
        lines.push(Line {
            spans: current,
            meta: LineMeta::None,
        });
    }

    if lines.is_empty() {
        lines.push(Line::empty());
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a single-span Line with the given text.
    fn plain_line(text: &str) -> Line {
        Line {
            spans: vec![StyledSpan {
                text: text.to_string(),
                style: Style::default(),
            }],
            meta: LineMeta::None,
        }
    }

    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.text.as_str()).collect()
    }

    // ── wrap_lines basic behaviour ──────────────────────────────────────────

    #[test]
    fn short_line_passes_through_unchanged() {
        let lines = vec![plain_line("hello")];
        let wrapped = wrap_lines(&lines, 80);
        assert_eq!(wrapped.len(), 1);
        assert_eq!(line_text(&wrapped[0]), "hello");
    }

    #[test]
    fn empty_line_passes_through() {
        let lines = vec![Line::empty()];
        let wrapped = wrap_lines(&lines, 80);
        assert_eq!(wrapped.len(), 1);
        assert!(wrapped[0].spans.is_empty());
    }

    #[test]
    fn zero_width_returns_input_unchanged() {
        let lines = vec![plain_line("hello world")];
        let wrapped = wrap_lines(&lines, 0);
        assert_eq!(wrapped.len(), 1);
        assert_eq!(line_text(&wrapped[0]), "hello world");
    }

    #[test]
    fn wraps_at_word_boundary() {
        let lines = vec![plain_line("hello world")];
        let wrapped = wrap_lines(&lines, 6);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(line_text(&wrapped[0]).trim(), "hello");
        assert_eq!(line_text(&wrapped[1]).trim(), "world");
    }

    #[test]
    fn long_word_force_broken() {
        let lines = vec![plain_line("abcdefghij")];
        let wrapped = wrap_lines(&lines, 4);
        assert!(wrapped.len() >= 3);
        // Each wrapped line should be at most 4 chars
        for line in &wrapped {
            assert!(line.display_width() <= 4);
        }
        // All characters preserved
        let all: String = wrapped.iter().map(|l| line_text(l)).collect();
        assert_eq!(all, "abcdefghij");
    }

    #[test]
    fn heading_meta_propagated_to_all_wrapped_lines() {
        let mut line = plain_line("hello world foo bar");
        line.meta = LineMeta::Heading {
            level: 2,
            text: "heading".to_string(),
        };
        let wrapped = wrap_lines(&[line], 10);
        assert!(wrapped.len() >= 2);
        for l in &wrapped {
            assert!(
                matches!(l.meta, LineMeta::Heading { level: 2, .. }),
                "all wrapped lines of a heading should carry the Heading meta"
            );
        }
    }

    #[test]
    fn list_item_meta_propagated_to_all_wrapped_lines() {
        let mut line = plain_line("this is a long list item that wraps");
        line.meta = LineMeta::ListItem { list_id: 42 };
        let wrapped = wrap_lines(&[line], 10);
        assert!(wrapped.len() >= 2);
        for l in &wrapped {
            assert!(
                matches!(l.meta, LineMeta::ListItem { list_id: 42 }),
                "all wrapped lines of a list item should carry the ListItem meta"
            );
        }
    }

    #[test]
    fn code_meta_propagated_to_first_wrapped_line_only() {
        let mut line = plain_line("some very long code line content here");
        line.meta = LineMeta::CodeContent { block_id: 5 };
        let wrapped = wrap_lines(&[line], 10);
        assert!(wrapped.len() >= 2);
        assert!(matches!(
            wrapped[0].meta,
            LineMeta::CodeContent { block_id: 5 }
        ));
        for l in &wrapped[1..] {
            assert!(matches!(l.meta, LineMeta::None));
        }
    }

    #[test]
    fn exact_width_line_not_wrapped() {
        let lines = vec![plain_line("12345")];
        let wrapped = wrap_lines(&lines, 5);
        assert_eq!(wrapped.len(), 1);
        assert_eq!(line_text(&wrapped[0]), "12345");
    }

    #[test]
    fn multiple_lines_wrapped_independently() {
        let lines = vec![plain_line("aaa bbb"), plain_line("ccc ddd")];
        let wrapped = wrap_lines(&lines, 4);
        assert!(
            wrapped.len() >= 4,
            "each input line should wrap into at least 2"
        );
        let texts: Vec<String> = wrapped
            .iter()
            .map(|l| line_text(l).trim().to_string())
            .collect();
        // "aaa" and "bbb" should appear before "ccc" and "ddd"
        let aaa_pos = texts
            .iter()
            .position(|t| t == "aaa")
            .expect("missing 'aaa'");
        let bbb_pos = texts
            .iter()
            .position(|t| t == "bbb")
            .expect("missing 'bbb'");
        let ccc_pos = texts
            .iter()
            .position(|t| t == "ccc")
            .expect("missing 'ccc'");
        let ddd_pos = texts
            .iter()
            .position(|t| t == "ddd")
            .expect("missing 'ddd'");
        assert!(aaa_pos < bbb_pos);
        assert!(bbb_pos < ccc_pos);
        assert!(ccc_pos < ddd_pos);
    }

    #[test]
    fn cjk_characters_count_as_double_width() {
        // Each CJK character is 2 columns wide; with width=6, at most 3 CJK chars fit per line
        let lines = vec![plain_line("你好世界测试")]; // 6 chars, 12 columns
        let wrapped = wrap_lines(&lines, 6);
        assert!(
            wrapped.len() >= 2,
            "CJK text should wrap based on display width, not char count"
        );
        for line in &wrapped {
            assert!(
                line.display_width() <= 6,
                "each line should be at most 6 columns, got {}",
                line.display_width()
            );
        }
        let all: String = wrapped.iter().map(|l| line_text(l)).collect();
        assert_eq!(all, "你好世界测试");
    }

    #[test]
    fn emoji_display_width_respected() {
        // Emoji are typically 2 columns wide
        let lines = vec![plain_line("🎉🎊🎈")]; // 3 emoji, 6 columns
        let wrapped = wrap_lines(&lines, 4);
        assert!(
            wrapped.len() >= 2,
            "emoji text should wrap based on display width"
        );
        for line in &wrapped {
            assert!(line.display_width() <= 4);
        }
        let all: String = wrapped.iter().map(|l| line_text(l)).collect();
        assert_eq!(all, "🎉🎊🎈");
    }

    #[test]
    fn multi_span_line_wraps_preserving_styles() {
        let bold_style = Style {
            bold: true,
            ..Style::default()
        };
        let line = Line {
            spans: vec![
                StyledSpan {
                    text: "bold ".to_string(),
                    style: bold_style.clone(),
                },
                StyledSpan {
                    text: "normal text here".to_string(),
                    style: Style::default(),
                },
            ],
            meta: LineMeta::None,
        };
        // Width 10 should force a wrap within the second span
        let wrapped = wrap_lines(&[line], 10);
        assert!(wrapped.len() >= 2, "multi-span line should wrap");
        // First wrapped line should start with the bold span
        assert!(
            wrapped[0].spans[0].style.bold,
            "first span should preserve bold style"
        );
        // All text should be preserved across wrapped lines
        let all_text: String = wrapped.iter().map(|l| line_text(l)).collect();
        assert!(all_text.contains("bold"));
        assert!(all_text.contains("normal"));
        assert!(all_text.contains("text"));
        assert!(all_text.contains("here"));
    }

    #[test]
    fn blockquote_wrapping_preserves_prefix_on_all_lines() {
        let line = Line {
            spans: vec![
                StyledSpan {
                    text: BLOCKQUOTE_PREFIX.to_string(),
                    style: Style {
                        fg: Some(Color::Rgb {
                            r: 100,
                            g: 100,
                            b: 200,
                        }),
                        ..Default::default()
                    },
                },
                StyledSpan {
                    text: "this is a long blockquote that should wrap to multiple lines easily"
                        .to_string(),
                    style: Style {
                        italic: true,
                        ..Default::default()
                    },
                },
            ],
            meta: LineMeta::None,
        };
        let wrapped = wrap_lines(&[line], 30);
        assert!(wrapped.len() >= 2, "blockquote should wrap");
        for l in &wrapped {
            assert_eq!(
                l.spans[0].text, BLOCKQUOTE_PREFIX,
                "all wrapped lines should start with prefix"
            );
            assert!(
                l.display_width() <= 30,
                "line too wide: {}",
                l.display_width()
            );
        }
    }

    #[test]
    fn blockquote_wrapping_preserves_structural_meta() {
        let line = Line {
            spans: vec![
                StyledSpan {
                    text: BLOCKQUOTE_PREFIX.to_string(),
                    style: Style {
                        fg: Some(Color::Rgb {
                            r: 100,
                            g: 100,
                            b: 200,
                        }),
                        ..Default::default()
                    },
                },
                StyledSpan {
                    text: "a heading inside a blockquote that is long enough to wrap around"
                        .to_string(),
                    style: Style::default(),
                },
            ],
            meta: LineMeta::Heading {
                level: 2,
                text: "a heading inside a blockquote that is long enough to wrap around"
                    .to_string(),
            },
        };
        let wrapped = wrap_lines(&[line], 30);
        assert!(wrapped.len() >= 2, "blockquote heading should wrap");
        for l in &wrapped {
            assert_eq!(l.spans[0].text, BLOCKQUOTE_PREFIX);
            assert!(
                matches!(l.meta, LineMeta::Heading { .. }),
                "structural meta should be preserved on all wrapped lines"
            );
        }
    }

    #[test]
    fn blockquote_short_line_passes_through() {
        let line = Line {
            spans: vec![
                StyledSpan {
                    text: BLOCKQUOTE_PREFIX.to_string(),
                    style: Style {
                        fg: Some(Color::Rgb {
                            r: 100,
                            g: 100,
                            b: 200,
                        }),
                        ..Default::default()
                    },
                },
                StyledSpan {
                    text: "short".to_string(),
                    style: Style::default(),
                },
            ],
            meta: LineMeta::None,
        };
        let wrapped = wrap_lines(&[line], 80);
        assert_eq!(wrapped.len(), 1);
    }
}
