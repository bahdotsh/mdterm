# Image/Embed Path Resolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix local images resolving against the process's working directory instead of the open markdown file's own directory, and add a configurable attachments-folder fallback for wikilink `![[embeds]]`, per `docs/superpowers/specs/2026-08-04-image-embed-resolution-design.md`.

**Architecture:** `wikilink.rs` tags embed-originated image destinations with an `mdembed:` prefix so the fetch layer can tell them apart from plain CommonMark images. `ImageCache` (in `image.rs`) gains a `base_dir` (the open file's directory, refreshed every `rebuild()`) and `attachments_dir` (from config), and a new `resolve_local_image_path` helper resolves local image references against `base_dir`, with an attachments-folder fallback for bare-filename embeds only. All resolution stays on the existing background fetch/pre-render threads — no synchronous filesystem calls are added to the render path.

**Tech Stack:** Rust 2024, `image` crate, `pulldown-cmark`, `serde`/`toml`, `regex`.

## Global Constraints

- No Rust toolchain may be available in the sandbox. Check `cargo --version` first; if absent, use `docker run --rm -v "$PWD":/w -w /w rust:1-slim <cargo subcommand>` for every build/test/fmt/clippy step in this plan. Every "Run:" step below shows the plain `cargo` form — substitute the Docker wrapper if `cargo` isn't on PATH.
- Security invariant: a markdown-declared image target must never resolve outside `base_dir` (or, for embeds, `base_dir`/`attachments_dir`). Absolute paths and `..` components in the declared target are always rejected — this must hold after every task, not just the task that introduces the check.
- No behavior change to remote (`http://`/`https://`) image fetching, the SSRF blocklist, or the Terminology temp-file/ownership checks.
- Match existing code style: 4-space indent, doc comments only where the *why* isn't obvious from the code.

---

### Task 1: Config — `attachments_dir` setting

**Files:**
- Modify: `src/config.rs:5-17` (`Config` struct), `src/config.rs:76-90` (`default_theme`/`Default for Config`)
- Test: `src/config.rs` (existing `#[cfg(test)] mod tests`, ~line 141)

**Interfaces:**
- Produces: `Config::attachments_dir: String` (default `"attachments"`), used by Task 6.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/config.rs`:

```rust
    #[test]
    fn default_attachments_dir_is_attachments() {
        assert_eq!(Config::default().attachments_dir, "attachments");
    }

    #[test]
    fn attachments_dir_deserializes_from_toml() {
        let cfg: Config = toml::from_str("attachments_dir = \"assets\"").unwrap();
        assert_eq!(cfg.attachments_dir, "assets");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib config:: 2>&1 | tail -30`
Expected: FAIL to compile — `no field \`attachments_dir\` on type \`Config\``.

- [ ] **Step 3: Implement the config field**

In `src/config.rs`, add the field to the `Config` struct:

```rust
#[derive(Deserialize)]
pub struct Config {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub line_numbers: bool,
    #[serde(default)]
    pub width: usize,
    #[serde(default)]
    pub hide: HideConfig,
    #[serde(default)]
    pub picker: PickerConfig,
    /// Fallback folder (relative to the current file's own directory) checked
    /// for a bare-filename wikilink embed (`![[photo.png]]`) that isn't found
    /// directly next to the file — mirrors Obsidian's own default attachment
    /// folder. Plain CommonMark images never use this fallback.
    #[serde(default = "default_attachments_dir")]
    pub attachments_dir: String,
}
```

Add the default function next to `default_theme`:

```rust
fn default_attachments_dir() -> String {
    "attachments".to_string()
}
```

Update `impl Default for Config`:

```rust
impl Default for Config {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            line_numbers: false,
            width: 0,
            hide: HideConfig::default(),
            picker: PickerConfig::default(),
            attachments_dir: default_attachments_dir(),
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib config:: 2>&1 | tail -30`
Expected: PASS (all `config::tests::*` green).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add configurable attachments_dir setting"
```

---

### Task 2: Wikilink embeds — mark destination with `mdembed:` prefix

**Files:**
- Modify: `src/wikilink.rs:119-129` (`rewrite_prose`, `EMBED_RE.replace_all` closure)
- Modify: `src/wikilink.rs:248-258` (existing embed tests)

**Interfaces:**
- Produces: embed destinations now look like `![alt](<mdembed:target>)` instead of `![alt](<target>)`. `target` itself is unchanged (still the raw, un-resolved embed target string). Plain CommonMark `![](path)` images never pass through this code and are completely unaffected.
- Consumes: nothing new.

- [ ] **Step 1: Update the existing tests to expect the new prefix (this is the failing-test step for this behavior change)**

Replace in `src/wikilink.rs`:

```rust
    #[test]
    fn preprocess_rewrites_image_embed() {
        let out = preprocess("![[photo.png]]");
        assert_eq!(out, "![photo.png](<mdembed:photo.png>)");
    }

    #[test]
    fn preprocess_rewrites_image_embed_with_alt() {
        let out = preprocess("![[photo.png|A nice photo]]");
        assert_eq!(out, "![A nice photo](<mdembed:photo.png>)");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib wikilink::tests::preprocess_rewrites_image_embed 2>&1 | tail -20`
Expected: FAIL — both assertions mismatch (`![photo.png](<photo.png>)` vs expected `<mdembed:photo.png>`).

- [ ] **Step 3: Implement the prefix**

In `src/wikilink.rs`, update `rewrite_prose`:

```rust
fn rewrite_prose(segment: &str) -> String {
    let after_embeds = EMBED_RE.replace_all(segment, |caps: &Captures| {
        let target = caps.get(1).unwrap().as_str().trim();
        let label = caps
            .get(2)
            .map(|m| m.as_str().trim())
            .filter(|s| !s.is_empty());
        let alt = label.unwrap_or(target);
        format!("![{}](<mdembed:{}>)", escape_label(alt), escape_dest(target))
    });
    // ... LINK_RE handling below is unchanged ...
```

(Only the `format!` line inside the `EMBED_RE.replace_all` closure changes — prepend the literal `mdembed:` marker before the escaped target.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib wikilink:: 2>&1 | tail -40`
Expected: PASS — all `wikilink::tests::*` green, including the two updated embed tests and all unrelated link tests (unaffected).

- [ ] **Step 5: Commit**

```bash
git add src/wikilink.rs
git commit -m "feat(wikilink): mark embed image destinations with mdembed: prefix"
```

---

### Task 3: `resolve_local_image_path` helper in `image.rs`

**Files:**
- Modify: `src/image.rs:1-6` (imports), `src/image.rs:2109-2127` (near `fetch_image`, add the new function above it)
- Test: `src/image.rs` (`#[cfg(test)] mod tests`, ~line 2249)

**Interfaces:**
- Produces: `fn resolve_local_image_path(target: &str, base_dir: &Path, attachments_dir: &str, is_embed: bool) -> Option<PathBuf>` — a pure, synchronous, no-network function. `target` has no `mdembed:` prefix (caller strips it first). Used by Task 4 (`fetch_image`) and Task 5 (`pre_render_terminology`).
- Consumes: nothing new (this task only adds the helper function; nothing calls it yet).

- [ ] **Step 1: Write the failing tests**

Add `use std::path::{Path, PathBuf};` to the top of `src/image.rs` (alongside the existing `use std::collections::{HashMap, HashSet};` etc.).

Add to the `tests` module in `src/image.rs`:

```rust
    fn temp_root(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}"))
    }

    #[test]
    fn resolve_local_image_path_finds_file_relative_to_base_dir() {
        let root = temp_root("mdterm-image-basedir");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("photo.png"), b"fake").unwrap();

        let resolved = resolve_local_image_path("photo.png", &root, "attachments", false);
        assert_eq!(resolved, Some(root.join("photo.png")));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_local_image_path_embed_falls_back_to_attachments_dir() {
        let root = temp_root("mdterm-image-attachfallback");
        std::fs::create_dir_all(root.join("attachments")).unwrap();
        std::fs::write(root.join("attachments").join("photo.png"), b"fake").unwrap();

        let resolved = resolve_local_image_path("photo.png", &root, "attachments", true);
        assert_eq!(resolved, Some(root.join("attachments").join("photo.png")));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_local_image_path_plain_image_does_not_use_attachments_fallback() {
        let root = temp_root("mdterm-image-noattachfallback");
        std::fs::create_dir_all(root.join("attachments")).unwrap();
        std::fs::write(root.join("attachments").join("photo.png"), b"fake").unwrap();

        // is_embed = false: a bare filename must NOT fall back to attachments/.
        let resolved = resolve_local_image_path("photo.png", &root, "attachments", false);
        assert_eq!(resolved, None);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_local_image_path_explicit_subpath_does_not_need_fallback() {
        let root = temp_root("mdterm-image-subpath");
        std::fs::create_dir_all(root.join("attachments")).unwrap();
        std::fs::write(root.join("attachments").join("photo.png"), b"fake").unwrap();

        let resolved =
            resolve_local_image_path("attachments/photo.png", &root, "attachments", true);
        assert_eq!(resolved, Some(root.join("attachments").join("photo.png")));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_local_image_path_rejects_absolute_target() {
        let root = temp_root("mdterm-image-rejectabs");
        std::fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        let abs = "/etc/passwd";
        #[cfg(windows)]
        let abs = "C:\\Windows\\win.ini";
        assert_eq!(
            resolve_local_image_path(abs, &root, "attachments", true),
            None
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_local_image_path_rejects_parent_dir_traversal() {
        let root = temp_root("mdterm-image-rejectdotdot");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(
            resolve_local_image_path("../secret.png", &root, "attachments", true),
            None
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_local_image_path_missing_file_returns_none() {
        let root = temp_root("mdterm-image-missing");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(
            resolve_local_image_path("nope.png", &root, "attachments", true),
            None
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn resolve_local_image_path_treats_backslash_subpath_as_not_bare() {
        let root = temp_root("mdterm-image-winsep");
        std::fs::create_dir_all(root.join("attachments")).unwrap();
        // A target with an explicit subpath (using the platform separator)
        // must NOT be treated as a bare filename, so no attachments-folder
        // fallback is attempted for it.
        let resolved = resolve_local_image_path("sub\\photo.png", &root, "attachments", true);
        assert_eq!(resolved, None);
        std::fs::remove_dir_all(root).unwrap();
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib image::tests::resolve_local_image_path 2>&1 | tail -30`
Expected: FAIL to compile — `cannot find function \`resolve_local_image_path\``.

- [ ] **Step 3: Implement the helper**

In `src/image.rs`, add immediately above `fn fetch_image`:

```rust
/// Resolve a locally-declared image reference to a file that exists on
/// disk, relative to `base_dir` — the directory of the markdown file that
/// declared it, not the process's working directory.
///
/// `target` must already have any `mdembed:` prefix stripped by the
/// caller; `is_embed` records whether it came from a wikilink `![[embed]]`
/// (as opposed to a plain CommonMark `![](path)`), since only embeds fall
/// back to the `attachments_dir` folder for a bare filename with no
/// subpath — mirroring Obsidian's own default-attachment-folder behavior.
///
/// Returns `None` if `target` is an absolute path or contains a `..`
/// component (both rejected to prevent a markdown file from reading
/// arbitrary local files), or if the file doesn't exist at either
/// candidate location.
fn resolve_local_image_path(
    target: &str,
    base_dir: &Path,
    attachments_dir: &str,
    is_embed: bool,
) -> Option<PathBuf> {
    let path = Path::new(target);
    if path.is_absolute() {
        return None;
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }

    let direct = base_dir.join(path);
    if direct.is_file() {
        return Some(direct);
    }

    if is_embed && path.components().count() == 1 {
        let via_attachments = base_dir.join(attachments_dir).join(path);
        if via_attachments.is_file() {
            return Some(via_attachments);
        }
    }

    None
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib image::tests::resolve_local_image_path 2>&1 | tail -40`
Expected: PASS — all 7 (8 on Windows) new tests green. `cargo test --lib image::` overall should still fully pass (nothing else changed yet).

- [ ] **Step 5: Commit**

```bash
git add src/image.rs
git commit -m "feat(image): add resolve_local_image_path helper for base_dir-relative resolution"
```

---

### Task 4: Wire `resolve_local_image_path` into `fetch_image` + `ImageCache`

**Files:**
- Modify: `src/image.rs:850-895` (`ImageCache` struct fields)
- Modify: `src/image.rs:897-926` (`ImageCache::new`)
- Modify: `src/image.rs:928-930` (add setters after `pub fn protocol`)
- Modify: `src/image.rs:970-994` (`start_fetch`)
- Modify: `src/image.rs:1062-1069` (`#[cfg(test)] fn fetch_if_missing`)
- Modify: `src/image.rs:2109-2127` (`fetch_image`)
- Test: `src/image.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `resolve_local_image_path` from Task 3.
- Produces: `ImageCache::set_base_dir(&mut self, dir: PathBuf)`, `ImageCache::set_attachments_dir(&mut self, dir: String)` — both used by Task 6. `fetch_image(url: &str, base_dir: &Path, attachments_dir: &str) -> Option<DynamicImage>` — new signature, used by Task 5's thread closure too (already the case via `start_fetch`, unchanged call site count).

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/image.rs`:

```rust
    #[test]
    fn fetch_if_missing_resolves_relative_to_base_dir_not_cwd() {
        let root = temp_root("mdterm-cache-basedir");
        std::fs::create_dir_all(&root).unwrap();
        let img = image::DynamicImage::new_rgb8(4, 4);
        img.save_with_format(root.join("photo.png"), image::ImageFormat::Png)
            .unwrap();

        let mut cache = ImageCache::new();
        cache.set_base_dir(root.clone());
        cache.fetch_if_missing("photo.png");

        assert!(cache.has_image("photo.png"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fetch_if_missing_embed_falls_back_to_attachments_dir() {
        let root = temp_root("mdterm-cache-embedfallback");
        std::fs::create_dir_all(root.join("attachments")).unwrap();
        let img = image::DynamicImage::new_rgb8(4, 4);
        img.save_with_format(
            root.join("attachments").join("photo.png"),
            image::ImageFormat::Png,
        )
        .unwrap();

        let mut cache = ImageCache::new();
        cache.set_base_dir(root.clone());
        cache.set_attachments_dir("attachments".to_string());
        cache.fetch_if_missing("mdembed:photo.png");

        assert!(cache.has_image("mdembed:photo.png"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fetch_if_missing_plain_image_ignores_attachments_dir() {
        let root = temp_root("mdterm-cache-plainnofallback");
        std::fs::create_dir_all(root.join("attachments")).unwrap();
        let img = image::DynamicImage::new_rgb8(4, 4);
        img.save_with_format(
            root.join("attachments").join("photo.png"),
            image::ImageFormat::Png,
        )
        .unwrap();

        let mut cache = ImageCache::new();
        cache.set_base_dir(root.clone());
        cache.fetch_if_missing("photo.png"); // no mdembed: prefix

        assert!(!cache.has_image("photo.png"));
        std::fs::remove_dir_all(root).unwrap();
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib image::tests::fetch_if_missing 2>&1 | tail -30`
Expected: FAIL to compile — `no method named \`set_base_dir\` found for struct \`ImageCache\``.

- [ ] **Step 3: Add `base_dir`/`attachments_dir` fields to `ImageCache`**

In the `ImageCache` struct definition, add two fields (after `in_tmux: bool,`):

```rust
    /// Whether mdterm is running inside a tmux session.
    /// Used by the render layer to wrap escape sequences in DCS passthrough.
    in_tmux: bool,
    /// Directory of the currently open markdown file. Local image references
    /// are resolved relative to this, not the process's working directory.
    base_dir: PathBuf,
    /// Configured "default attachment folder" name (relative to `base_dir`),
    /// used as a fallback for bare-filename wikilink embeds.
    attachments_dir: String,
```

In `ImageCache::new()`, initialize them (after `in_tmux,`):

```rust
        ImageCache {
            images: HashMap::new(),
            protocol,
            in_tmux,
            base_dir: PathBuf::from("."),
            attachments_dir: "attachments".to_string(),
            kitty_images: HashMap::new(),
```

- [ ] **Step 4: Add setters**

Immediately after `pub fn protocol(&self) -> ImageProtocol { self.protocol }`:

```rust
    /// Set the directory of the currently open markdown file. Local image
    /// references are resolved relative to this directory. Call on every
    /// rebuild so switching files takes effect.
    pub fn set_base_dir(&mut self, dir: PathBuf) {
        self.base_dir = dir;
    }

    /// Set the configured "default attachment folder" name (relative to
    /// `base_dir`), used as a fallback for bare-filename wikilink embeds.
    pub fn set_attachments_dir(&mut self, dir: String) {
        self.attachments_dir = dir;
    }
```

- [ ] **Step 5: Rewrite `fetch_image` to use `resolve_local_image_path`**

Replace the whole function body:

```rust
fn fetch_image(url: &str, base_dir: &Path, attachments_dir: &str) -> Option<DynamicImage> {
    let (target, is_embed) = match url.strip_prefix("mdembed:") {
        Some(target) => (target, true),
        None => (url, false),
    };
    if target.starts_with("http://") || target.starts_with("https://") {
        return fetch_image_http(target);
    }
    let path = resolve_local_image_path(target, base_dir, attachments_dir, is_embed)?;
    image::open(path).ok()
}
```

- [ ] **Step 6: Thread `base_dir`/`attachments_dir` through `start_fetch`**

```rust
    pub fn start_fetch(&mut self, url: &str) -> bool {
        if self.images.contains_key(url) || self.in_flight.contains(url) {
            return true; // already handled
        }
        if self.in_flight.len() >= Self::MAX_CONCURRENT_FETCHES {
            return false;
        }
        self.in_flight.insert(url.to_string());
        let sender = self.sender.clone();
        let url_owned = url.to_string();
        let base_dir = self.base_dir.clone();
        let attachments_dir = self.attachments_dir.clone();
        std::thread::spawn(move || {
            // Guard against panics in image decoding/downscaling so that
            // the channel always receives a result and the in_flight slot
            // is freed by poll_completed(). Without this, a panic would
            // leave the URL stuck in in_flight permanently.
            let img = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                fetch_image(&url_owned, &base_dir, &attachments_dir)
                    .map(|img| downscale(img, MAX_SOURCE_DIM))
            }))
            .unwrap_or(None);
            let _ = sender.send((url_owned, img));
        });
        true
    }
```

- [ ] **Step 7: Update the `#[cfg(test)]` test helper**

```rust
    #[cfg(test)]
    fn fetch_if_missing(&mut self, url: &str) {
        if self.images.contains_key(url) {
            return;
        }
        let img = fetch_image(url, &self.base_dir, &self.attachments_dir)
            .map(|img| Arc::new(downscale(img, MAX_SOURCE_DIM)));
        self.images.insert(url.to_string(), img);
    }
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test --lib image:: 2>&1 | tail -60`
Expected: PASS — the 3 new tests plus every pre-existing `image::tests::*` test still green (nothing in Terminology is touched yet, so `terminology_no_temp_for_local_path` should be unaffected by this task).

- [ ] **Step 9: Commit**

```bash
git add src/image.rs
git commit -m "feat(image): resolve local images against base_dir instead of the CWD"
```

---

### Task 5: Terminology protocol — same base_dir/attachments_dir resolution

**Files:**
- Modify: `src/image.rs:1792-1903` (`pre_render_image`)
- Modify: `src/image.rs:1951-2096` (`pre_render_terminology`)
- Modify: `src/image.rs:1090-1157` (`queue_pre_render`)
- Modify: `src/image.rs:3160-3209` (existing test `terminology_no_temp_for_local_path`)
- Test: `src/image.rs`

**Interfaces:**
- Consumes: `resolve_local_image_path` from Task 3.
- Produces: `pre_render_image(..., base_dir: &Path, attachments_dir: &str, ...)` and `pre_render_terminology(..., base_dir: &Path, attachments_dir: &str)` — new trailing parameters (call sites updated in this same task; nothing else calls these functions).

- [ ] **Step 1: Update the existing test to use `base_dir` instead of an absolute CWD path (the failing-test step for this behavior change)**

Replace `terminology_no_temp_for_local_path` in `src/image.rs`:

```rust
    /// Local-path images (within `base_dir`) must reuse the original path
    /// (`is_temp = false`) and must NOT create a new temp file.
    #[test]
    fn terminology_no_temp_for_local_path() {
        let root = temp_root("mdterm-terminology-basedir");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("photo.png");
        {
            let img = image::DynamicImage::new_rgb8(8, 8);
            img.save_with_format(&source_path, image::ImageFormat::Png)
                .expect("failed to write source fixture PNG");
        }

        let img = image::open(&source_path).expect("failed to open fixture PNG");
        let metrics = CellMetrics {
            aspect: 2.0,
            cell_w_px: 8,
            cell_h_px: 16,
        };

        let priv_dir = std::env::temp_dir().join(format!("mdterm-{}", std::process::id()));
        let before_count = std::fs::read_dir(&priv_dir)
            .map(|rd| rd.flatten().count())
            .unwrap_or(0);

        let result = pre_render_terminology(&img, "photo.png", 80, metrics, &root, "attachments")
            .expect("pre_render_terminology returned None for a local path within base_dir");

        let after_count = std::fs::read_dir(&priv_dir)
            .map(|rd| rd.flatten().count())
            .unwrap_or(0);

        let canonical_source =
            std::fs::canonicalize(&source_path).unwrap_or_else(|_| source_path.clone());
        std::fs::remove_dir_all(&root).ok();

        assert!(!result.is_temp, "local path must not set is_temp=true");
        assert_eq!(
            result.path,
            canonical_source
                .to_str()
                .expect("canonical path is not UTF-8"),
            "path must be the canonicalized local path"
        );
        assert_eq!(
            before_count, after_count,
            "pre_render_terminology must not create new temp files for a local path"
        );
    }
```

(This test now passes a plain relative filename plus an explicit `base_dir`, instead of an absolute CWD-derived path — the old absolute-path input would be rejected by `resolve_local_image_path`'s absolute-path check from Task 3, which Terminology's resolution now shares with `fetch_image`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib image::tests::terminology_no_temp_for_local_path 2>&1 | tail -30`
Expected: FAIL to compile — `this function takes 4 arguments but 6 arguments were supplied`.

- [ ] **Step 3: Update `pre_render_terminology`'s signature and local-path branch**

Update the doc comment and signature:

```rust
/// Pre-render step for the Terminology protocol.
/// Resolves the image to a local filesystem path.
/// - If `url` (after stripping any `mdembed:` prefix) is a local path that
///   resolves to a file under `base_dir` (or, for embeds, under
///   `base_dir`/`attachments_dir`), returns it as-is (after canonicalization
///   to an absolute path), provided the resolved path stays within `base_dir`.
/// - Otherwise, resizes `img` to the display pixel dimensions and writes it
///   atomically to a temporary PNG file in a per-process private temp directory.
///
/// Returns `None` if the resolved path would be unsafe to embed in the escape
/// sequence, escapes `base_dir`, or if any I/O operation fails.
fn pre_render_terminology(
    img: &DynamicImage,
    url: &str,
    content_width: usize,
    cell_metrics: CellMetrics,
    base_dir: &Path,
    attachments_dir: &str,
) -> Option<TerminologyImage> {
    let (img_w, img_h) = img.dimensions();
    let (cols, rows) = calc_display_cells(
        img_w,
        img_h,
        content_width,
        MAX_IMAGE_ROWS,
        cell_metrics.aspect,
    );
    // Terminology hard limit: both width and height must be < 512 (from tycat.c:
    // `if ((w >= 512) || (h >= 512)) return;`). Also ensure neither is zero.
    let cols = (cols as u32).clamp(1, 511);
    let rows = (rows as u32).clamp(1, 511);

    let (target, is_embed) = match url.strip_prefix("mdembed:") {
        Some(t) => (t, true),
        None => (url, false),
    };

    // Treat http://, https://, file://, and data: URLs as remote/non-local.
    // file:// and data: are not valid filesystem paths; canonicalize would fail
    // for them, so they fall through to the temp-file path.
    let is_remote = target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("file://")
        || target.starts_with("data:");

    // Local file path that still exists on disk — reuse directly, no I/O needed.
    // Terminology requires an absolute path, so canonicalize before passing.
    if !is_remote
        && let Some(resolved) = resolve_local_image_path(target, base_dir, attachments_dir, is_embed)
        && let Ok(abs) = std::fs::canonicalize(&resolved)
    {
        // SEC: Reject if the resolved path escapes base_dir. This prevents
        // symlinks like `./img.png -> /etc/passwd` from passing an arbitrary
        // system path to Terminology. Canonicalize base_dir too so both paths
        // use the same prefix form (important on Windows where canonicalize
        // adds the `\\?\` prefix but a plain PathBuf does not, causing
        // starts_with to always fail).
        let base_canonical = std::fs::canonicalize(base_dir).ok();
        if base_canonical.is_some_and(|base| !abs.starts_with(&base)) {
            return None;
        }

        // SEC: Use to_str() (not to_string_lossy()) to reject non-UTF-8 paths.
        // to_string_lossy() would replace invalid bytes with U+FFFD, meaning
        // the safety check runs on a modified string, not the actual bytes.
        let path = abs.to_str()?.to_owned();

        // SEC: Reject paths that would corrupt the escape sequence framing
        // (control bytes, DEL, semicolons). See terminology_path_safe docs.
        if !terminology_path_safe(&path) {
            return None;
        }

        let hashes = "#".repeat(cols as usize);
        return Some(TerminologyImage {
            path,
            cols,
            rows,
            is_temp: false,
            hashes,
        });
    }

    // Remote image (or local path no longer on disk): resize and write a temp file.
    let target_w = (cols * cell_metrics.cell_w_px).max(1);
    // ... (rest of the function, from "let target_w = ..." to the end, is unchanged) ...
```

(Everything from the `// Remote image (or local path no longer on disk)` comment onward — the temp-file-writing branch — stays exactly as it is today; only the local-path branch above it changes.)

- [ ] **Step 4: Thread `base_dir`/`attachments_dir` through `pre_render_image`**

```rust
fn pre_render_image(
    img: &DynamicImage,
    protocol: ImageProtocol,
    content_width: usize,
    cell_metrics: CellMetrics,
    bg: (u8, u8, u8),
    kitty_id: u32,
    base_dir: &Path,
    attachments_dir: &str,
    terminology: Option<&TerminologyCtx<'_>>,
) -> Option<PreRenderedResult> {
```

And update its Terminology arm:

```rust
        ImageProtocol::Terminology => {
            let ctx = terminology
                .expect("pre_render_image: Terminology protocol requires a TerminologyCtx");
            pre_render_terminology(img, ctx.url, content_width, cell_metrics, base_dir, attachments_dir).map(|ti| {
```

(The rest of that arm's body is unchanged.)

- [ ] **Step 5: Thread `base_dir`/`attachments_dir` through `queue_pre_render`'s spawned thread**

In `queue_pre_render`, before `std::thread::spawn`, alongside the existing `let temp_files_ref = Arc::clone(&self.temp_files);`:

```rust
        let temp_files_ref = Arc::clone(&self.temp_files);
        let base_dir = self.base_dir.clone();
        let attachments_dir = self.attachments_dir.clone();
```

And update the `pre_render_image` call inside the spawned closure:

```rust
                pre_render_image(
                    &img,
                    protocol,
                    content_width,
                    cell_metrics,
                    bg,
                    kitty_id,
                    &base_dir,
                    &attachments_dir,
                    terminology.as_ref(),
                )
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib image:: 2>&1 | tail -60`
Expected: PASS — `terminology_no_temp_for_local_path` and every other `image::tests::*` test green.

- [ ] **Step 7: Commit**

```bash
git add src/image.rs
git commit -m "feat(image): resolve Terminology's local image path against base_dir"
```

---

### Task 6: Wire config + file directory through `viewer.rs`/`main.rs`

**Files:**
- Modify: `src/viewer.rs:29-41` (`ViewerOptions` struct)
- Modify: `src/viewer.rs:410-470` (`ViewerState::new`)
- Modify: `src/viewer.rs:608-616` (`rebuild`, top of function)
- Modify: `src/main.rs:164-176` (`ViewerOptions` construction)

**Interfaces:**
- Consumes: `Config::attachments_dir` (Task 1), `ImageCache::set_base_dir`/`set_attachments_dir` (Task 4).
- Produces: nothing new consumed by later tasks — this is the final wiring task.

- [ ] **Step 1: Add `attachments_dir` to `ViewerOptions`**

In `src/viewer.rs`:

```rust
pub struct ViewerOptions {
    pub files: Vec<String>,
    pub initial_content: String,
    pub filename: String,
    pub theme: Theme,
    pub slide_mode: bool,
    pub line_numbers: bool,
    pub width_override: Option<usize>,
    pub picker_root: Option<PathBuf>,
    pub start_in_picker: bool,
    pub hide: HideConfig,
    pub picker: PickerConfig,
    pub attachments_dir: String,
}
```

- [ ] **Step 2: Pass it from `main.rs`**

In `src/main.rs`, add one field to the existing `ViewerOptions` literal:

```rust
        let opts = viewer::ViewerOptions {
            files,
            initial_content: content,
            filename,
            theme: initial_theme,
            slide_mode: cli.slides,
            line_numbers,
            width_override: if width > 0 { Some(width) } else { None },
            picker_root,
            start_in_picker,
            hide,
            picker: picker_config,
            attachments_dir: config.attachments_dir,
        };
```

- [ ] **Step 3: Construct `ImageCache` with the configured attachments dir in `ViewerState::new`**

In `src/viewer.rs`, replace the inline `image_cache: crate::image::ImageCache::new(),` field with a pre-built local variable. Immediately before the `ViewerState { ... }` literal in `fn new(opts: ViewerOptions, cols: u16, rows: u16) -> Self`, add:

```rust
        let mut image_cache = crate::image::ImageCache::new();
        image_cache.set_attachments_dir(opts.attachments_dir.clone());
```

Then inside the struct literal, change:

```rust
            image_cache: crate::image::ImageCache::new(),
```

to:

```rust
            image_cache,
```

- [ ] **Step 4: Refresh `base_dir` on every rebuild**

At the very top of `fn rebuild(&mut self)` in `src/viewer.rs`, before the existing `let saved_offset = self.offset;` line:

```rust
    fn rebuild(&mut self) {
        let base_dir = std::path::Path::new(&self.filename)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        self.image_cache.set_base_dir(base_dir);

        // Save the scroll position before re-rendering.  finalize_layout()
        // adjusts the offset for image-row expansion, but when called from
        // rebuild() the offset was already correct for the previous layout's
        // expanded images — the delta would be double-counted.  Restoring
        // and clamping preserves the user's scroll position.
        let saved_offset = self.offset;
```

- [ ] **Step 5: Build and run the full test suite**

Run: `cargo build 2>&1 | tail -60`
Expected: builds cleanly (this task only wires existing, already-tested functions together — no new unit tests needed here beyond a successful build and the full suite below).

Run: `cargo test 2>&1 | tail -80`
Expected: PASS — every test in the crate green, including `viewer::` and `main`-adjacent tests unaffected by this wiring.

- [ ] **Step 6: Commit**

```bash
git add src/viewer.rs src/main.rs
git commit -m "feat(viewer): resolve images relative to the open file's directory"
```

---

### Task 7: Full verification (fmt, clippy, tests)

**Files:** none (verification only).

- [ ] **Step 1: Format check**

Run: `cargo fmt --check`
If it fails: run `cargo fmt` (no check), review the diff is only whitespace/formatting, `git add -u && git commit -m "style: cargo fmt"`.

- [ ] **Step 2: Clippy**

Run: `cargo clippy -- -D warnings`
Expected: no warnings. If clippy flags something introduced by this plan (e.g. an argument-count lint on `pre_render_image`/`pre_render_terminology` now taking more parameters), address it in place — do not add `#[allow(...)]` without first checking whether a small refactor (e.g. a small params struct) is warranted; only add the allow if the existing codebase already uses that pattern for a similarly-shaped function (`pre_render_terminology` already had `#[allow(clippy::too_many_arguments)]` precedent on `render_block_image` — check `image.rs` for the existing `#[allow(clippy::too_many_arguments)]` on `render_block_image` before deciding).

- [ ] **Step 3: Full test suite**

Run: `cargo test 2>&1 | tail -100`
Expected: PASS — every test across `config::`, `wikilink::`, `image::`, `viewer::`, `markdown::`, etc.

- [ ] **Step 4: Release build sanity check**

Run: `cargo build --release`
Expected: builds cleanly (matches CI's `build` job).

- [ ] **Step 5: Manual smoke test (if a terminal with image support is available)**

Create a scratch vault layout and confirm the fix end-to-end:

```bash
mkdir -p /tmp/mdterm-smoke/attachments
# any small real PNG works; this one-liner needs python3+Pillow, adjust if unavailable
python3 -c "from PIL import Image; Image.new('RGB', (4,4)).save('/tmp/mdterm-smoke/attachments/photo.png')"
cat > /tmp/mdterm-smoke/note.md <<'EOF'
Plain image (relative to file dir): ![Test](attachments/photo.png)

Embed, bare filename (should fall back to attachments/): ![[photo.png]]

Embed, explicit subpath: ![[attachments/photo.png]]
EOF
cd / && cargo run --manifest-path /Users/shane.kakau/kry/code/worktrees/mdviewer/image-bug/Cargo.toml -- /tmp/mdterm-smoke/note.md
```

Expected: all three images render (not "[ Loading: ]" placeholders), confirming the CWD-vs-file-directory bug is fixed even when launched from a directory (`/`) other than the file's own directory (`/tmp/mdterm-smoke/`).

- [ ] **Step 6: If no local toolchain/terminal available, verify via CI**

Push the branch and open a PR against `splusk/mdviewer` `main` (per `tasks/todo.md`'s environment note) to get CI's fmt/clippy/build/test matrix (ubuntu/macos/windows) as the final verification.
