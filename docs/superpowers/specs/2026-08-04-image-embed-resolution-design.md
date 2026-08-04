# Design: base_dir-relative image resolution + wikilink embed attachments fallback

**Date:** 2026-08-04
**Branch:** `fix/image-embed-resolution`

## Problem

1. **General bug (pre-existing, not wikilink-specific).** Local image references
   in a markdown file (e.g. `![Test](attachments/IMG_3779.png)`) are resolved
   via `image::open(url)` / `std::fs::canonicalize(url)` in `src/image.rs`,
   both of which resolve a relative path against the *process's current
   working directory*, not the directory of the markdown file that declared
   the reference. If mdterm is launched from a directory other than the
   file's own directory, the image fails to load. Reproduces identically with
   plain CommonMark image syntax, confirming it's independent of the
   wikilinks feature.
2. **New requirement.** Obsidian-style embeds (`![[target]]` /
   `![[target|alt]]`, added in PR #2 / v2.1.1) were deliberately scoped to
   reuse the plain image pipeline unchanged, with zero wikilink-specific
   resolution. That scope is now revised: `![[IMG_3779.png]]` and
   `![[attachments/IMG_3779.png]]` should both resolve to a real file.

## Resolution order (embeds)

1. Relative to the current markdown file's own directory (`base_dir`) — this
   also fixes bug (1) above, since it's the same join, applied uniformly.
2. If not found there, and the embed target has no subpath (a bare filename,
   e.g. `IMG_3779.png`), also check inside a configurable "attachments"
   folder relative to `base_dir` — mirroring Obsidian's own default
   attachment folder.
3. `![[attachments/IMG_3779.png]]` (explicit relative subpath) already works
   once (1) is fixed — it's just a relative path under `base_dir`.

Plain CommonMark images (`![](path)`) only ever get resolution step 1 — the
attachments-folder fallback is embed-specific, matching Obsidian's own
behavior (plain markdown images have no notion of a default attachment
folder).

## Architecture & data flow

```
markdown source
  │
  ├─ wikilink.rs::preprocess()
  │    ![[target]]        → ![alt](<mdembed:target>)      (NEW: scheme marker)
  │    ![[target|alt]]    → ![alt](<mdembed:target>)
  │    ![](path)          → unchanged, never touches wikilink.rs
  │
  ▼
pulldown-cmark → markdown.rs (Tag::Image { dest_url }) → LineMeta::Image { url }
  (url is exactly the declared string: "mdembed:target", "path", or "http(s)://…")
  │
  ▼
viewer.rs rebuild()
  - refreshes image_cache.set_base_dir(Path::new(&self.filename).parent())
  - queues url into pending_image_urls (unchanged)
  │
  ▼
image.rs ImageCache::start_fetch(url)  [background thread]
  - remote (http/https) → fetch_image_http (unchanged)
  - local → resolve_local_image(url, base_dir, attachments_dir):
      1. strip "mdembed:" prefix if present → (target, is_embed)
      2. reject if target is absolute or has a ".." component (unchanged
         security check, now checked pre-join instead of pre-CWD-open)
      3. try image::open(base_dir.join(target))
      4. if that fails AND is_embed AND target is a single path component
         (Path::new(target).components().count() == 1 — covers both `/`
         and `\` separators, so Windows-authored vaults behave the same) →
         try image::open(base_dir.join(attachments_dir).join(target))
      5. first success wins; both failing → None (unchanged failure path →
         "[ Loading: ]" placeholder forever)
```

`wikilink.rs`'s embed rewrite gets a scheme-prefix marker (`mdembed:`) so that
`image.rs` can distinguish an embed-originated URL from a plain markdown
image URL by the time it reaches the fetch layer — without threading any new
parameters through `markdown.rs`'s parsing. Plain images never pass through
`wikilink.rs`'s regex at all, so their declared string is untouched.

`pre_render_terminology()` (used only by the Terminology terminal protocol)
gets the same `base_dir`/`attachments_dir` treatment. It currently
re-derives a path from the raw `url` independently via its own
`canonicalize` + working-directory boundary check; that boundary check moves
from "must be under CWD" to "must be under `base_dir`".

`ImageCache` gains two fields:
- `attachments_dir: String` — set once at construction from `Config`.
- `base_dir: PathBuf` — refreshed on every `rebuild()` from
  `Path::new(&self.filename).parent()`, cloned into each spawned
  fetch/pre-render thread alongside the existing `cell_metrics`/`protocol`
  clones.

## Why lazy resolution (in `image.rs`), not eager (in `markdown.rs`)

Two approaches were considered:

- **Lazy (chosen):** resolution happens at fetch time, on the background
  threads `start_fetch`/`queue_pre_render` already spawn. No change to
  `markdown.rs`'s parsing contract or its parameter list.
- **Eager:** resolve immediately when `Tag::Image { dest_url }` is captured
  in `markdown.rs`, storing an already-resolved path in `LineMeta::Image.url`.
  Simpler (one resolution site), but requires threading `base_dir` and
  `attachments_dir` into `render_with()`, and performs synchronous
  `stat`/`is_file` filesystem calls on the main thread during `rebuild()` —
  which runs on every resize, edit, and file switch. This regresses the
  codebase's existing discipline of doing all image I/O on background
  threads (`start_fetch` exists specifically so the first frame renders
  immediately without blocking on disk/network).

Lazy resolution was chosen to preserve that non-blocking guarantee.

## Error handling & security invariants

- **Security invariant preserved exactly.** The existing checks — reject
  absolute paths, reject any `..` path component — still run on the
  *declared* target string (whatever follows `mdembed:`, or the whole `url`
  for plain images), before any join. A hostile `![[../../etc/passwd]]` or
  `![](/etc/passwd)` is rejected exactly as today; only the *base* of the
  join changes (file's directory instead of process CWD).
- **Failure mode unchanged.** If neither candidate path opens as a valid
  image, `fetch_image` returns `None`, gets cached as `None`, and the viewer
  shows `[ Loading: <alt or target> ]` forever — identical to today's
  behavior for a genuinely missing file. No new error states are introduced.
- **No change** to the SSRF/remote-image checks, Terminology's temp-file /
  ownership checks, or the reject-absolute/`..` logic itself — only *where*
  the safe relative join happens (base_dir instead of CWD).

## Config

`src/config.rs` gains a flat field on `Config`:

```rust
#[serde(default = "default_attachments_dir")]
pub attachments_dir: String,   // default: "attachments"
```

The attachments folder is always resolved relative to the *embedding file's
own directory* (`base_dir.join(attachments_dir)`), never a vault root — a
note in `handbook/processes/notes.md` checks
`handbook/processes/attachments/`, not a single vault-wide folder. This
matches Obsidian's own per-folder default when "Default location for new
attachments" isn't pinned to one fixed vault-wide folder — the common case
this feature is scoped to.

## Known limitation (fixed)

The image cache (`ImageCache.images` and friends) is keyed by the raw
declared URL string, not by a resolved absolute path. Two different open
files that both reference the same relative string (e.g. both have an
`attachments/photo.png`) still share a cache entry under that string. Under
CWD-based resolution this was harmless (CWD was fixed for the whole
process, so the same relative string always resolved to the same target),
but under `base_dir`-based resolution it would otherwise show the wrong
file's image after switching. `ImageCache::set_base_dir` now clears the
fetch and render caches whenever the directory actually changes, so
switching between files resolves each one's images fresh instead of
reusing a stale entry.

## Testing plan

- `src/wikilink.rs`: update `preprocess_rewrites_image_embed` and
  `preprocess_rewrites_image_embed_with_alt` to expect the `mdembed:`
  prefix.
- `src/image.rs`: new unit tests for the resolution helper, covering:
  1. plain relative path resolves against `base_dir`, not CWD.
  2. `mdembed:` bare filename resolves against `base_dir` first, falls back
     to `base_dir/attachments_dir` when not found directly.
  3. `mdembed:` with an explicit subpath resolves via the first join only.
  4. plain (non-embed) bare filename does **not** get the attachments
     fallback.
  5. absolute path and `..`-containing target both still rejected
     (regression test for the existing security check).
  6. `pre_render_terminology`'s boundary check rejects paths escaping
     `base_dir`.
  7. bare-filename check is platform-correct: a single path component under
     both `/`- and `\`-style separators.
- `src/config.rs`: default `attachments_dir` is `"attachments"`; round-trip
  a config with a custom value.
- Verification: no local Rust toolchain available (`cargo` not found).
  Docker is available (`docker --version` succeeds) — run the full suite via
  `docker run --rm -v "$PWD":/w -w /w rust:1-slim cargo test` before
  considering this done. Fall back to pushing a branch + relying on GitHub
  Actions CI if Docker verification fails for some reason.
