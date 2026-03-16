use std::io::{self, Write};

use crossterm::style::Color;

use crate::markdown;
use crate::style::{LineMeta, wrap_lines};
use crate::theme::Theme;

pub fn to_html(content: &str, width: usize, theme: &Theme) {
    let (lines, _) = markdown::render(content, width, theme, false);
    let wrapped = wrap_lines(&lines, width);

    let mut out = io::stdout();
    let _ = writeln!(out, "<!DOCTYPE html>");
    let _ = writeln!(out, "<html><head>");
    let _ = writeln!(out, "<meta charset='utf-8'>");
    let _ = writeln!(
        out,
        "<style>body {{ font-family: 'SF Mono','Menlo','Consolas',monospace; background:{}; color:{}; padding:2em; line-height:1.4; }} pre {{ margin:0; }} .line {{ white-space:pre; min-height:1.2em; }}</style>",
        color_css(theme.bg),
        color_css(theme.fg)
    );
    let _ = writeln!(out, "</head><body>");

    for line in &wrapped {
        // Handle image placeholder lines
        if let LineMeta::Image {
            ref url,
            ref alt,
            row,
            ..
        } = line.meta
        {
            if row == 0 {
                if is_safe_img_src(url) {
                    let _ = writeln!(
                        out,
                        "<div class='line'><img src='{}' alt='{}' style='max-width:100%;height:auto;'></div>",
                        html_escape(url),
                        html_escape(alt)
                    );
                } else {
                    let _ = writeln!(
                        out,
                        "<div class='line'>{}</div>",
                        html_escape(alt)
                    );
                }
            }
            continue;
        }

        let _ = write!(out, "<div class='line'>");
        if line.spans.is_empty() {
            let _ = write!(out, "&nbsp;");
        }
        for span in &line.spans {
            let mut styles = Vec::new();
            if let Some(fg) = span.style.fg {
                styles.push(format!("color:{}", color_css(fg)));
            }
            if let Some(bg) = span.style.bg {
                styles.push(format!("background:{}", color_css(bg)));
            }
            if span.style.bold {
                styles.push("font-weight:bold".into());
            }
            if span.style.italic {
                styles.push("font-style:italic".into());
            }
            match (span.style.underline, span.style.strikethrough) {
                (true, true) => {
                    styles.push("text-decoration:underline line-through".into());
                }
                (true, false) => {
                    styles.push("text-decoration:underline".into());
                }
                (false, true) => {
                    styles.push("text-decoration:line-through".into());
                }
                _ => {}
            }
            if span.style.dim {
                styles.push("opacity:0.5".into());
            }

            let text = html_escape(&span.text);

            if styles.is_empty() {
                let _ = write!(out, "{}", text);
            } else {
                let _ = write!(out, "<span style='{}'>", styles.join(";"));
                if let Some(ref url) = span.style.link_url {
                    if is_safe_url(url) {
                        let _ = write!(
                            out,
                            "<a href='{}' style='color:inherit;text-decoration:inherit'>{}</a>",
                            html_escape(url),
                            text
                        );
                    } else {
                        let _ = write!(out, "{}", text);
                    }
                } else {
                    let _ = write!(out, "{}", text);
                }
                let _ = write!(out, "</span>");
            }
        }
        let _ = writeln!(out, "</div>");
    }

    let _ = writeln!(out, "</body></html>");
}

fn color_css(c: Color) -> String {
    match c {
        Color::Rgb { r, g, b } => format!("#{:02x}{:02x}{:02x}", r, g, b),
        _ => "#000".into(),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Returns true if the URL scheme is safe for use in `<a href>`.
fn is_safe_url(url: &str) -> bool {
    let trimmed = url.trim();
    let lower = trimmed.to_lowercase();
    // Allow common safe schemes, anchors, and relative paths
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || trimmed.starts_with('#')
    {
        return true;
    }
    // Block known dangerous schemes
    if lower.starts_with("javascript:")
        || lower.starts_with("vbscript:")
        || lower.starts_with("data:")
    {
        return false;
    }
    // Allow relative paths (no colon before first slash)
    !lower.split('/').next().unwrap_or("").contains(':')
}

/// Returns true if the URL is safe for use in `<img src>`.
fn is_safe_img_src(url: &str) -> bool {
    let trimmed = url.trim();
    let lower = trimmed.to_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:image/")
    {
        return true;
    }
    // Block dangerous schemes
    if lower.starts_with("javascript:")
        || lower.starts_with("vbscript:")
        || lower.starts_with("data:")
    {
        return false;
    }
    // Allow relative paths
    !lower.split('/').next().unwrap_or("").contains(':')
}
