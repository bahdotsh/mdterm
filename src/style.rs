use crossterm::style::Color;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub dim: bool,
}

#[derive(Clone, Debug)]
pub struct StyledSpan {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub spans: Vec<StyledSpan>,
}

impl Line {
    pub fn empty() -> Self {
        Line { spans: vec![] }
    }

    pub fn display_width(&self) -> usize {
        self.spans.iter().map(|s| s.text.chars().count()).sum()
    }
}

pub fn wrap_lines(lines: &[Line], width: usize) -> Vec<Line> {
    if width == 0 {
        return lines.to_vec();
    }
    let mut result = Vec::new();
    for line in lines {
        if line.spans.is_empty() {
            result.push(Line::empty());
        } else if line.display_width() <= width {
            result.push(line.clone());
        } else {
            result.extend(word_wrap(line, width));
        }
    }
    result
}

fn word_wrap(line: &Line, width: usize) -> Vec<Line> {
    // Split spans into word/whitespace segments
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
        let seg_width = seg.text.chars().count();
        let is_ws = seg
            .text
            .chars()
            .next()
            .map(|c| c.is_whitespace())
            .unwrap_or(false);

        if !is_ws && col + seg_width > width && col > 0 {
            // Remove trailing whitespace from current line
            if let Some(last) = current.last() {
                if last.text.chars().all(|c| c.is_whitespace()) {
                    current.pop();
                }
            }
            lines.push(Line {
                spans: std::mem::take(&mut current),
            });
            col = 0;
        }

        // Skip leading whitespace on continuation lines
        if col == 0 && is_ws && !lines.is_empty() {
            continue;
        }

        // Handle words longer than width
        if !is_ws && seg_width > width && col == 0 {
            let chars: Vec<char> = seg.text.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let avail = width - col;
                let take = avail.min(chars.len() - i);
                let chunk: String = chars[i..i + take].iter().collect();
                current.push(StyledSpan {
                    text: chunk,
                    style: seg.style.clone(),
                });
                col += take;
                i += take;
                if col >= width && i < chars.len() {
                    lines.push(Line {
                        spans: std::mem::take(&mut current),
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
        lines.push(Line { spans: current });
    }

    if lines.is_empty() {
        lines.push(Line::empty());
    }

    lines
}
