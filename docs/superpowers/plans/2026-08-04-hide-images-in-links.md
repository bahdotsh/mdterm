# Hide Images In Links Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `hide.images_in_links` config flag that suppresses an image entirely (no block, no caption, no fetch) when it is nested inside a link's label, per `docs/superpowers/specs/2026-08-04-hide-images-in-links-design.md`.

**Architecture:** `HideConfig` gains a new boolean field. `markdown.rs`'s existing `hide.images` early-return (in the `Event::End(TagEnd::Image)` handler) extends its condition to also fire when the new flag is set and the renderer is currently inside a link (`self.in_link`, already tracked). No new state, no new fetch/render logic — this is a pure suppression toggle at the exact point `hide.images` already suppresses at.

**Tech Stack:** Rust 2024, `pulldown-cmark`, `serde`/`toml` (unchanged dependencies).

## Global Constraints

- No Rust toolchain may be available in the sandbox. Check `cargo --version` first; if absent, use `docker run --rm -v "$PWD":/w -w /w rust:1-slim <cargo subcommand>` for every build/test/fmt/clippy step in this plan.
- Default `false` — no behavior change for anyone who doesn't set this in `config.toml`.
- Suppressed means "emit nothing at all" (no `LineMeta::Image` line, no caption span) — identical in kind to the existing `hide.images` behavior, just narrower in trigger (only images nested inside a link label).
- Must not affect standalone (non-link-nested) images even when `images_in_links = true`.
- Must apply the same way regardless of whether the nested image is plain CommonMark syntax or a wikilink embed (`![[x]]`) — the trigger is `self.in_link`, not the image's own syntax.
- Match existing code style: 4-space indent, doc comments only where the *why* isn't obvious.

---

### Task 1: `hide.images_in_links` config flag + suppression + tests

**Files:**
- Modify: `src/config.rs:49-60` (`HideConfig` struct)
- Modify: `src/config.rs:156-162` (test helper `fn hide`)
- Modify: `src/markdown.rs:942-952` (`Event::End(TagEnd::Image)` early-return)
- Modify: `src/markdown.rs:1836-1858` (test helpers `hide_langs`, `hide_images`, `hide_frontmatter`)
- Test: `src/markdown.rs` (`#[cfg(test)] mod tests`, ~line 1920)

**Interfaces:**
- Produces: `HideConfig::images_in_links: bool` (default `false`), consumed by `markdown.rs`'s image-end handler. No other task depends on this — it's a single, self-contained change.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/markdown.rs`, near the existing `hidden_images_emit_no_lines_or_caption` test (~line 1921):

```rust
    fn hide_images_in_links() -> HideConfig {
        HideConfig {
            images: false,
            frontmatter: false,
            code_languages: Vec::new(),
            images_in_links: true,
        }
    }

    #[test]
    fn image_nested_in_link_is_suppressed_when_flag_set() {
        let input = "[Label ![icon](attachments/icon.png)](https://example.com)";
        let (lines, _) = render_hiding(input, &hide_images_in_links());
        let text = all_text(&lines);

        assert!(
            !lines
                .iter()
                .any(|l| matches!(l.meta, LineMeta::Image { .. })),
            "no placeholder lines should be emitted for an image nested in a link"
        );
        assert!(
            text.contains("Label"),
            "the link's surrounding text should still render: {text}"
        );
    }

    #[test]
    fn image_nested_in_link_renders_normally_when_flag_unset() {
        let input = "[Label ![icon](attachments/icon.png)](https://example.com)";
        let (lines, _) = render_test(input);

        assert!(
            lines
                .iter()
                .any(|l| matches!(l.meta, LineMeta::Image { .. })),
            "with the flag off (default), the nested image should render as a block"
        );
    }

    #[test]
    fn standalone_image_is_not_suppressed_by_images_in_links_flag() {
        let input = "before\n\n![alt text](http://example.com/img.png)\n\nafter";
        let (lines, _) = render_hiding(input, &hide_images_in_links());

        assert!(
            lines
                .iter()
                .any(|l| matches!(l.meta, LineMeta::Image { .. })),
            "a standalone (non-link-nested) image must not be suppressed by images_in_links"
        );
    }

    #[test]
    fn wikilink_embed_nested_in_link_is_also_suppressed_when_flag_set() {
        let input = "[Label ![[icon.png]]](https://example.com)";
        let (lines, _) = render_hiding(input, &hide_images_in_links());
        let text = all_text(&lines);

        assert!(
            !lines
                .iter()
                .any(|l| matches!(l.meta, LineMeta::Image { .. })),
            "a wikilink embed nested in a link should be suppressed the same as a plain image"
        );
        assert!(text.contains("Label"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib markdown::tests::image_nested_in_link 2>&1 | tail -30`
Expected: FAIL to compile — `missing field \`images_in_links\` in initializer of \`HideConfig\`` (from the new `hide_images_in_links()` helper referencing a field that doesn't exist yet).

- [ ] **Step 3: Add the config field**

In `src/config.rs`, update `HideConfig`:

```rust
/// Content that is parsed but deliberately never rendered.
#[derive(Deserialize, Default, Clone)]
pub struct HideConfig {
    #[serde(default)]
    pub images: bool,
    /// A leading YAML metadata block delimited by `---`.
    #[serde(default)]
    pub frontmatter: bool,
    /// Fenced code block languages to omit entirely, e.g. `dataviewjs`.
    #[serde(default)]
    pub code_languages: Vec<String>,
    /// Images nested inside a link's label (e.g. an icon prefixing a link,
    /// `[Label ![icon](path)](url)`) render as a block that breaks the
    /// link's line. Off by default — existing vaults that intentionally
    /// embed a real image inside a link (e.g. a clickable thumbnail) keep
    /// today's behavior unless they opt in.
    #[serde(default)]
    pub images_in_links: bool,
}
```

- [ ] **Step 4: Fix the existing `HideConfig` struct-literal construction sites**

These three helpers build `HideConfig` via struct literal (not `..Default::default()`), so the compiler will reject them once the new field is added. Add `images_in_links: false,` to each:

In `src/config.rs`'s test module:

```rust
    fn hide(langs: &[&str]) -> HideConfig {
        HideConfig {
            images: false,
            frontmatter: false,
            code_languages: langs.iter().map(|s| s.to_string()).collect(),
            images_in_links: false,
        }
    }
```

In `src/markdown.rs`'s test module:

```rust
    fn hide_langs(langs: &[&str]) -> HideConfig {
        HideConfig {
            images: false,
            frontmatter: false,
            code_languages: langs.iter().map(|s| s.to_string()).collect(),
            images_in_links: false,
        }
    }

    fn hide_images() -> HideConfig {
        HideConfig {
            images: true,
            frontmatter: false,
            code_languages: Vec::new(),
            images_in_links: false,
        }
    }

    fn hide_frontmatter() -> HideConfig {
        HideConfig {
            images: false,
            frontmatter: true,
            code_languages: Vec::new(),
            images_in_links: false,
        }
    }
```

(`render_test`'s use of `HideConfig::default()` needs no change — `#[derive(Default)]` picks up the new field automatically at `false`.)

- [ ] **Step 5: Extend the suppression condition**

In `src/markdown.rs`, update the `Event::End(TagEnd::Image)` handler:

```rust
            Event::End(TagEnd::Image) => {
                // Hidden images emit nothing at all — no placeholder rows and no
                // caption. viewer.rs queues downloads by scanning for
                // LineMeta::Image lines, so emitting none also means nothing is
                // ever fetched. Also suppressed: an image nested inside a link's
                // label when `images_in_links` is set — e.g. a decorative icon
                // prefixing a link, which would otherwise break the link's line
                // into a text line + an image block + a caption line.
                if self.hide.images || (self.hide.images_in_links && self.in_link) {
                    self.image_alt.clear();
                    self.image_url.clear();
                    self.in_image = false;
                    return;
                }
```

(Only the `if` condition and its comment change — everything after this block is unchanged.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib markdown:: 2>&1 | tail -60`
Expected: PASS — the 4 new tests plus every pre-existing `markdown::tests::*` test (including `hidden_images_emit_no_lines_or_caption`, which exercises the unrelated `hide.images` path and must remain unaffected).

Run: `cargo test --lib config:: 2>&1 | tail -30`
Expected: PASS — confirms the `hide()` test-helper fix in `src/config.rs` compiles and its existing tests are unaffected.

- [ ] **Step 7: Full suite + fmt + clippy**

Run: `cargo test 2>&1 | tail -20`
Expected: PASS — every test in the crate green (last known-good baseline on this branch: 220 passed / 0 failed / 6 ignored; this task adds 4 new tests with 0 failures).

Run: `cargo fmt --check`
Expected: clean (or run `cargo fmt` and include the diff in this task's commit if not).

Run: `cargo clippy -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/config.rs src/markdown.rs
git commit -m "feat(markdown): add hide.images_in_links to suppress link-nested images"
```
