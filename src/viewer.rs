use std::io::{self, Write};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEventKind, read,
    },
    execute, queue,
    style::{Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode, size,
    },
};

use crate::style::{Line, StyledSpan, wrap_lines};

/// RAII guard to restore terminal state on drop (including panics).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = execute!(stdout, DisableMouseCapture, Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

pub fn run(content: &str, filename: &str) -> io::Result<()> {
    let mut stdout = io::stdout();

    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, Hide, EnableMouseCapture)?;
    let _guard = TerminalGuard;

    let (mut cols, mut rows) = size()?;
    let content_width = (cols as usize).saturating_sub(4);
    let lines = crate::markdown::render(content, content_width);
    let mut wrapped = wrap_lines(&lines, content_width);
    let mut offset: usize = 0;

    loop {
        let height = rows as usize;
        let width = cols as usize;
        let viewport = height.saturating_sub(2);
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
                let content_width = (cols as usize).saturating_sub(4);
                let lines = crate::markdown::render(content, content_width);
                wrapped = wrap_lines(&lines, content_width);
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
    let border_fg = Color::Rgb {
        r: 55,
        g: 58,
        b: 65,
    };
    let label_fg = Color::Rgb {
        r: 120,
        g: 125,
        b: 140,
    };
    let pos_fg = Color::Rgb {
        r: 90,
        g: 95,
        b: 110,
    };
    let content_width = width.saturating_sub(4); // │ + space + content + space + │

    // ── Top border: ╭─ filename ──...──╮ ──
    let file_label = format!(" {} ", filename);
    let file_label_len = file_label.chars().count();
    let top_fill = width.saturating_sub(3 + file_label_len);

    queue!(
        stdout,
        MoveTo(0, 0),
        SetForegroundColor(border_fg),
        Print("╭─"),
        SetForegroundColor(label_fg),
        Print(&file_label),
        SetForegroundColor(border_fg),
        Print(format!("{}╮", "─".repeat(top_fill))),
        SetAttribute(Attribute::Reset),
    )?;

    // ── Content lines with left/right borders ──
    for row in 0..viewport {
        queue!(stdout, MoveTo(0, (row + 1) as u16))?;

        // Left border
        queue!(
            stdout,
            SetForegroundColor(border_fg),
            Print("│ "),
            SetAttribute(Attribute::Reset),
        )?;

        if let Some(line) = lines.get(offset + row) {
            let mut col = 0;
            for span in &line.spans {
                write_span(stdout, span)?;
                col += span.text.chars().count();
            }
            if col < content_width {
                // Only extend background when all spans share the same bg
                // (e.g. code block lines). Avoids inline code bg bleeding.
                let common_bg = line.spans.first().and_then(|s| s.style.bg).and_then(|bg| {
                    if line.spans.iter().all(|s| s.style.bg == Some(bg)) {
                        Some(bg)
                    } else {
                        None
                    }
                });
                if let Some(bg) = common_bg {
                    queue!(
                        stdout,
                        SetBackgroundColor(bg),
                        Print(" ".repeat(content_width - col)),
                        SetAttribute(Attribute::Reset)
                    )?;
                } else {
                    queue!(stdout, Print(" ".repeat(content_width - col)))?;
                }
            }
        } else {
            queue!(stdout, Print(" ".repeat(content_width)))?;
        }

        // Right border
        queue!(
            stdout,
            SetForegroundColor(border_fg),
            Print(" │"),
            SetAttribute(Attribute::Reset),
        )?;
    }

    // ── Bottom border with position: ╰──...── position ─╯ ──
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

    let pos_label = format!(" {} ", position);
    let pos_label_len = pos_label.chars().count();
    let bot_fill = width.saturating_sub(3 + pos_label_len);

    queue!(
        stdout,
        MoveTo(0, (viewport + 1) as u16),
        SetForegroundColor(border_fg),
        Print(format!("╰{}", "─".repeat(bot_fill))),
        SetForegroundColor(pos_fg),
        Print(&pos_label),
        SetForegroundColor(border_fg),
        Print("─╯"),
        SetAttribute(Attribute::Reset),
    )?;

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
