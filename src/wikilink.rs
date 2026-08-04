use std::borrow::Cow;
use std::sync::LazyLock;

use regex::{Captures, Regex};

static EMBED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[\[([^\]|]+)(?:\|([^\]]+))?\]\]").unwrap());

static LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]|#]+)(?:#([^\]|]+))?(?:\|([^\]]+))?\]\]").unwrap());

/// Rewrite Obsidian-style `[[wikilinks]]` and `![[embeds]]` into standard
/// Markdown before the document reaches `pulldown-cmark`, which has no
/// notion of double-bracket syntax. Fenced code blocks and inline code
/// spans are left untouched so a literal `[[...]]` shown as a syntax
/// example doesn't get rewritten.
pub fn preprocess(source: &str) -> Cow<'_, str> {
    if !source.contains("[[") {
        return Cow::Borrowed(source);
    }

    let mut out = String::with_capacity(source.len() + 16);
    let mut fence: Option<(char, usize)> = None;

    for line in source.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let ending = &line[content.len()..];

        if let Some((fence_char, fence_len)) = fence {
            out.push_str(content);
            out.push_str(ending);
            if is_closing_fence(content, fence_char, fence_len) {
                fence = None;
            }
            continue;
        }

        if let Some(opened) = opening_fence(content) {
            fence = Some(opened);
            out.push_str(content);
            out.push_str(ending);
            continue;
        }

        out.push_str(&rewrite_line(content));
        out.push_str(ending);
    }

    Cow::Owned(out)
}

fn opening_fence(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let ch = trimmed.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let run_len = trimmed.chars().take_while(|&c| c == ch).count();
    (run_len >= 3).then_some((ch, run_len))
}

fn is_closing_fence(line: &str, fence_char: char, fence_len: usize) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed.chars().all(|c| c == fence_char)
        && trimmed.chars().count() >= fence_len
}

/// Rewrite wikilink/embed syntax on a single line, skipping over
/// backtick-delimited inline code spans.
fn rewrite_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    while let Some(tick_pos) = rest.find('`') {
        out.push_str(&rewrite_prose(&rest[..tick_pos]));

        let after_tick = &rest[tick_pos..];
        let run_len = after_tick.chars().take_while(|&c| c == '`').count();

        match find_closing_run(&after_tick[run_len..], run_len) {
            Some(rel_close) => {
                let close_end = run_len + rel_close + run_len;
                out.push_str(&after_tick[..close_end]);
                rest = &after_tick[close_end..];
            }
            None => {
                // No matching close run: not a real code span.
                out.push_str(&after_tick[..run_len]);
                rest = &after_tick[run_len..];
            }
        }
    }

    out.push_str(&rewrite_prose(rest));
    out
}

fn find_closing_run(s: &str, run_len: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let start = i;
            while i < bytes.len() && bytes[i] == b'`' {
                i += 1;
            }
            if i - start == run_len {
                return Some(start);
            }
        } else {
            i += 1;
        }
    }
    None
}

fn rewrite_prose(segment: &str) -> String {
    let after_embeds = EMBED_RE.replace_all(segment, |caps: &Captures| {
        let target = caps.get(1).unwrap().as_str().trim();
        let label = caps
            .get(2)
            .map(|m| m.as_str().trim())
            .filter(|s| !s.is_empty());
        let alt = label.unwrap_or(target);
        format!("![{}](<{}>)", escape_label(alt), escape_dest(target))
    });

    LINK_RE
        .replace_all(&after_embeds, |caps: &Captures| {
            let target = caps.get(1).unwrap().as_str().trim();
            let heading = caps
                .get(2)
                .map(|m| m.as_str().trim())
                .filter(|s| !s.is_empty());
            let label = caps
                .get(3)
                .map(|m| m.as_str().trim())
                .filter(|s| !s.is_empty());

            let display = label.unwrap_or(target);

            let mut dest = format!("wikilink:{}", escape_dest(target));
            if let Some(heading) = heading {
                dest.push('#');
                dest.push_str(&crate::viewer::heading_to_slug(heading));
            }

            format!("[{}](<{}>)", escape_label(display), dest)
        })
        .into_owned()
}

/// Escape characters that would otherwise be parsed as CommonMark link
/// destination delimiters inside `<...>`.
fn escape_dest(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('<', "\\<")
        .replace('>', "\\>")
}

/// Escape characters that would otherwise be parsed as CommonMark link
/// label delimiters.
fn escape_label(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_leaves_plain_text_borrowed() {
        let input = "Just plain text, no links here.";
        assert!(matches!(preprocess(input), Cow::Borrowed(_)));
    }

    #[test]
    fn preprocess_rewrites_bare_wikilink() {
        let out = preprocess("See [[getting-started]] for more.");
        assert_eq!(
            out,
            "See [getting-started](<wikilink:getting-started>) for more."
        );
    }

    #[test]
    fn preprocess_rewrites_piped_wikilink() {
        let out = preprocess("[[getting-started|Getting Started at Kry]]");
        assert_eq!(out, "[Getting Started at Kry](<wikilink:getting-started>)");
    }

    #[test]
    fn preprocess_rewrites_heading_anchor_slugified() {
        let out = preprocess("[[decision-records#Some Heading]]");
        assert_eq!(
            out,
            "[decision-records](<wikilink:decision-records#some-heading>)"
        );
    }

    #[test]
    fn preprocess_rewrites_nested_path_target() {
        let out = preprocess("[[handbook/processes/processes|Processes]]");
        assert_eq!(out, "[Processes](<wikilink:handbook/processes/processes>)");
    }

    #[test]
    fn preprocess_rewrites_image_embed() {
        let out = preprocess("![[photo.png]]");
        assert_eq!(out, "![photo.png](<photo.png>)");
    }

    #[test]
    fn preprocess_rewrites_image_embed_with_alt() {
        let out = preprocess("![[photo.png|A nice photo]]");
        assert_eq!(out, "![A nice photo](<photo.png>)");
    }

    #[test]
    fn preprocess_leaves_fenced_code_block_untouched() {
        let input = "```\n[[foo|bar]]\n```\n";
        assert_eq!(preprocess(input), input);
    }

    #[test]
    fn preprocess_leaves_inline_code_untouched() {
        let input = "Use `[[foo|bar]]` syntax.";
        assert_eq!(preprocess(input), input);
    }

    #[test]
    fn preprocess_handles_wikilink_in_table_cell() {
        let out = preprocess("| [[note|Label]] | other |");
        assert_eq!(out, "| [Label](<wikilink:note>) | other |");
    }

    #[test]
    fn preprocess_wraps_target_with_spaces() {
        let out = preprocess("[[Getting Started]]");
        assert_eq!(out, "[Getting Started](<wikilink:Getting Started>)");
    }
}
