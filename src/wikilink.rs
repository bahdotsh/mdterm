use std::borrow::Cow;
use std::path::{Path, PathBuf};
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

/// Resolve a wikilink target to a file on disk: first relative to the
/// directory of the file containing the link (matching how a plain
/// relative Markdown link resolves, with `.md` inferred since Obsidian
/// targets omit it), then falling back to a vault-wide search rooted at
/// `search_root` — the same directory tree the file picker (`p`) walks.
/// Ambiguous matches resolve to the first hit in `discover_markdown_files`'s
/// existing sort order (shallowest/alphabetically-first).
pub fn resolve_target(
    target: &str,
    current_dir: &Path,
    search_root: &Path,
    picker: &crate::config::PickerConfig,
) -> Option<PathBuf> {
    let candidate = current_dir.join(target);
    if candidate.is_file() {
        return Some(candidate);
    }

    let with_md = current_dir.join(format!("{target}.md"));
    if with_md.is_file() {
        return Some(with_md);
    }

    let entries = crate::file_picker::discover_markdown_files(search_root, picker).ok()?;
    let target_lower = target.to_lowercase();
    let target_md_lower = format!("{target_lower}.md");

    entries.into_iter().find_map(|entry| {
        let display_lower = entry.display.to_lowercase();
        let matches = display_lower == target_lower
            || display_lower == target_md_lower
            || display_lower.ends_with(&format!("/{target_lower}"))
            || display_lower.ends_with(&format!("/{target_md_lower}"));
        matches.then_some(entry.path)
    })
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

    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}"))
    }

    #[test]
    fn resolve_target_finds_file_relative_to_current_dir() {
        let root = temp_root("mdterm-wikilink-reldir");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("note.md"), "# Note").unwrap();

        let resolved = resolve_target(
            "note.md",
            &root,
            &root,
            &crate::config::PickerConfig::default(),
        );
        assert_eq!(resolved, Some(root.join("note.md")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_target_infers_md_extension() {
        let root = temp_root("mdterm-wikilink-inferext");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("note.md"), "# Note").unwrap();

        let resolved = resolve_target(
            "note",
            &root,
            &root,
            &crate::config::PickerConfig::default(),
        );
        assert_eq!(resolved, Some(root.join("note.md")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_target_falls_back_to_vault_wide_search_by_bare_name() {
        let root = temp_root("mdterm-wikilink-vaultbare");
        let current_dir = root.join("current");
        let elsewhere = root.join("elsewhere");
        fs::create_dir_all(&current_dir).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("getting-started.md"), "# GS").unwrap();

        let resolved = resolve_target(
            "getting-started",
            &current_dir,
            &root,
            &crate::config::PickerConfig::default(),
        );
        assert_eq!(resolved, Some(elsewhere.join("getting-started.md")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_target_falls_back_to_vault_wide_search_by_subpath() {
        let root = temp_root("mdterm-wikilink-vaultsubpath");
        let current_dir = root.join("current");
        fs::create_dir_all(&current_dir).unwrap();
        fs::create_dir_all(root.join("handbook").join("processes")).unwrap();
        fs::write(
            root.join("handbook").join("processes").join("processes.md"),
            "# Processes",
        )
        .unwrap();

        let resolved = resolve_target(
            "handbook/processes/processes",
            &current_dir,
            &root,
            &crate::config::PickerConfig::default(),
        );
        assert_eq!(
            resolved,
            Some(root.join("handbook").join("processes").join("processes.md"))
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_target_returns_none_when_nothing_matches() {
        let root = temp_root("mdterm-wikilink-nomatch");
        fs::create_dir_all(&root).unwrap();

        let resolved = resolve_target(
            "nope",
            &root,
            &root,
            &crate::config::PickerConfig::default(),
        );
        assert_eq!(resolved, None);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_target_picks_first_sorted_match_when_ambiguous() {
        let root = temp_root("mdterm-wikilink-ambiguous");
        let current_dir = root.join("current");
        fs::create_dir_all(&current_dir).unwrap();
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(root.join("a").join("dup.md"), "# A").unwrap();
        fs::write(root.join("b").join("dup.md"), "# B").unwrap();

        let resolved = resolve_target(
            "dup",
            &current_dir,
            &root,
            &crate::config::PickerConfig::default(),
        );
        // discover_markdown_files sorts by display path, so "a/dup.md" sorts before "b/dup.md".
        assert_eq!(resolved, Some(root.join("a").join("dup.md")));

        fs::remove_dir_all(root).unwrap();
    }
}
