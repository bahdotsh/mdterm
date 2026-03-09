use std::io::{self, Write};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        read, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEventKind,
    },
    execute, queue,
    style::{Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{
        disable_raw_mode, enable_raw_mode, size, EnterAlternateScreen, LeaveAlternateScreen,
    },
};

use crate::style::{wrap_lines, Line, StyledSpan};

/// RAII guard to restore terminal state on drop (including panics).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            DisableMouseCapture,
            Show,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

pub fn run(lines: Vec<Line>, filename: &str) -> io::Result<()> {
    let mut stdout = io::stdout();

    enable_raw_mode()?;
    execute!(
        stdout,
        EnterAlternateScreen,
        Hide,
        EnableMouseCapture
    )?;
    let _guard = TerminalGuard;

    let (mut cols, mut rows) = size()?;
    let mut wrapped = wrap_lines(&lines, cols as usize);
    let mut offset: usize = 0;

    loop {
        let height = rows as usize;
        let width = cols as usize;
        let viewport = height.saturating_sub(1);
        let max_offset = wrapped.len().saturating_sub(viewport);
        offset = offset.min(max_offset);

        render_frame(&mut stdout, &wrapped, offset, width, viewport, filename)?;

        match read()? {
            Event::Key(ke) if ke.kind == KeyEventKind::Press => match ke.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('c') if ke.modifiers.contains(KeyModifiers::CONTROL) => break,

                KeyCode::Down | KeyCode::Char('j') => {
                    offset = (offset + 1).min(max_offset);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    offset = offset.saturating_sub(1);
                }
                KeyCode::Char(' ') | KeyCode::PageDown | KeyCode::Char('d')
                    if ke.modifiers.contains(KeyModifiers::CONTROL)
                        || ke.code != KeyCode::Char('d') =>
                {
                    offset = (offset + viewport).min(max_offset);
                }
                KeyCode::Char('b') | KeyCode::PageUp | KeyCode::Char('u')
                    if ke.modifiers.contains(KeyModifiers::CONTROL)
                        || ke.code != KeyCode::Char('u') =>
                {
                    offset = offset.saturating_sub(viewport);
                }
                KeyCode::Char('g') | KeyCode::Home => {
                    offset = 0;
                }
                KeyCode::Char('G') | KeyCode::End => {
                    offset = max_offset;
                }
                _ => {}
            },
            Event::Mouse(me) => match me.kind {
                MouseEventKind::ScrollDown => {
                    let max_offset = wrapped.len().saturating_sub(rows as usize - 1);
                    offset = (offset + 3).min(max_offset);
                }
                MouseEventKind::ScrollUp => {
                    offset = offset.saturating_sub(3);
                }
                _ => {}
            },
            Event::Resize(c, r) => {
                cols = c;
                rows = r;
                wrapped = wrap_lines(&lines, cols as usize);
            }
            _ => {}
        }
    }

    // _guard Drop handles cleanup
    Ok(())
}

fn render_frame(
    stdout: &mut io::Stdout,
    lines: &[Line],
    offset: usize,
    width: usize,
    viewport: usize,
    filename: &str,
) -> io::Result<()> {
    queue!(stdout, MoveTo(0, 0))?;

    for row in 0..viewport {
        queue!(stdout, MoveTo(0, row as u16))?;
        if let Some(line) = lines.get(offset + row) {
            let mut col = 0;
            for span in &line.spans {
                write_span(stdout, span)?;
                col += span.text.chars().count();
            }
            if col < width {
                let line_bg = line.spans.iter().find_map(|s| s.style.bg);
                if let Some(bg) = line_bg {
                    queue!(
                        stdout,
                        SetBackgroundColor(bg),
                        Print(" ".repeat(width - col)),
                        SetAttribute(Attribute::Reset)
                    )?;
                } else {
                    queue!(stdout, Print(" ".repeat(width - col)))?;
                }
            }
        } else {
            queue!(stdout, Print(" ".repeat(width)))?;
        }
    }

    // Status bar
    let position = if lines.len() <= viewport {
        "All".to_string()
    } else if offset == 0 {
        "Top".to_string()
    } else if offset >= lines.len().saturating_sub(viewport) {
        "Bot".to_string()
    } else {
        let pct = (offset + viewport) * 100 / lines.len();
        format!("{}%", pct)
    };
    let bar_bg = Color::Rgb { r: 35, g: 38, b: 46 };
    let left = format!(" {}", filename);
    let right = format!("{} ", position);
    let left_w = left.chars().count();
    let right_w = right.chars().count();

    queue!(
        stdout,
        MoveTo(0, viewport as u16),
        SetBackgroundColor(bar_bg),
    )?;

    if left_w + right_w <= width {
        let gap = width - left_w - right_w;
        queue!(
            stdout,
            SetForegroundColor(Color::Rgb { r: 180, g: 180, b: 190 }),
            Print(&left),
            SetForegroundColor(Color::Rgb { r: 90, g: 90, b: 100 }),
            Print(" ".repeat(gap)),
            Print(&right),
        )?;
    } else {
        let truncated: String = left.chars().take(width).collect();
        queue!(
            stdout,
            SetForegroundColor(Color::Rgb { r: 180, g: 180, b: 190 }),
            Print(&truncated),
            Print(" ".repeat(width.saturating_sub(truncated.chars().count()))),
        )?;
    }

    queue!(stdout, SetAttribute(Attribute::Reset))?;

    stdout.flush()
}

fn write_span(stdout: &mut io::Stdout, span: &StyledSpan) -> io::Result<()> {
    let s = &span.style;

    if let Some(fg) = s.fg {
        queue!(stdout, SetForegroundColor(fg))?;
    }
    if let Some(bg) = s.bg {
        queue!(stdout, SetBackgroundColor(bg))?;
    }
    if s.bold {
        queue!(stdout, SetAttribute(Attribute::Bold))?;
    }
    if s.italic {
        queue!(stdout, SetAttribute(Attribute::Italic))?;
    }
    if s.underline {
        queue!(stdout, SetAttribute(Attribute::Underlined))?;
    }
    if s.strikethrough {
        queue!(stdout, SetAttribute(Attribute::CrossedOut))?;
    }
    if s.dim {
        queue!(stdout, SetAttribute(Attribute::Dim))?;
    }

    queue!(stdout, Print(&span.text), SetAttribute(Attribute::Reset))?;
    Ok(())
}

/// Print styled lines directly to stdout (for piped output).
pub fn print_lines(lines: &[Line]) {
    let mut stdout = io::stdout();
    for line in lines {
        for span in &line.spans {
            let _ = write_span(&mut stdout, span);
        }
        let _ = writeln!(stdout);
    }
}
