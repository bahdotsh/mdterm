# Design: suppress images nested inside link labels

**Date:** 2026-08-04
**Branch:** `fix/image-embed-resolution` (follow-up scoped onto the same branch — thematically related to embedded-image handling)

## Problem

A common Obsidian authoring pattern prefixes a link's label with a small
icon to indicate link type, e.g.:

```markdown
[Sanal's Evaluation ![icon](attachments/icons/w.png)](https://example.com/doc)
```

`markdown.rs` has no notion of an "inline, same-row" image — every image
(regardless of where it appears in the source) becomes a dedicated block:
the text before it is flushed as its own line, then a full placeholder/image
block is pushed (`crate::image::IMAGE_ROWS` rows, later resized to the
image's real aspect ratio), then a separate dim/italic caption line, then
processing continues with whatever text follows. For a link-prefix icon,
this breaks one intended line into three, defeating the purpose of the
convention.

This is pre-existing `markdown.rs` behavior, unrelated to and unaffected by
the CWD-vs-file-directory resolution fix on this branch — but that fix
changes the *outcome*: previously, icons like this often failed to resolve
(CWD-relative lookup), silently masking the block-breaking behavior behind
a permanent `[ Loading: ]` placeholder. Now that local images resolve
correctly relative to the file's own directory, an icon that exists on disk
will actually load and render as a real block, making the line-breaking
problem newly visible for every note using this convention.

## Requirement

Add a way to suppress an image specifically when it is nested inside a
link's label, distinct from the existing `hide.images` (which suppresses
*all* images unconditionally). When suppressed, the image contributes
nothing to the output — no block, no caption, no fetch — identical in kind
to `hide.images`'s existing "emit nothing at all" behavior, just narrower
in scope (only images inside a link label; standalone body images are
unaffected).

This applies uniformly regardless of whether the nested image is plain
CommonMark syntax (`![icon](path)`, as in the example above) or a wikilink
embed (`![[icon.png]]`) — the trigger is "are we currently inside a link,"
not the image's own syntax.

## Design

### Config

`src/config.rs`'s `HideConfig` gains one new field:

```rust
#[derive(Deserialize, Default, Clone)]
pub struct HideConfig {
    #[serde(default)]
    pub images: bool,
    #[serde(default)]
    pub frontmatter: bool,
    #[serde(default)]
    pub code_languages: Vec<String>,
    /// Images nested inside a link's label (e.g. an icon prefixing a link,
    /// `[Label ![icon](path)](url)`) are decorative and rendered as a
    /// block that breaks the link's line. Off by default — existing
    /// vaults that intentionally embed a real image inside a link (e.g. a
    /// clickable thumbnail) keep today's behavior unless they opt in.
    #[serde(default)]
    pub images_in_links: bool,
}
```

Default `false` — opt-in, no behavior change for anyone who doesn't set it.
Composes with `hide.images`: if `hide.images` is already `true`, this flag
is moot (everything is already suppressed).

### Rendering

`src/markdown.rs`'s `Event::End(TagEnd::Image)` handler already has an
early-return for `self.hide.images` that clears image state and emits
nothing (no `LineMeta::Image` lines, no caption span) — the comment there
already notes this means the image is also never queued for fetching,
since `viewer.rs` discovers fetch candidates by scanning for
`LineMeta::Image` lines.

That early-return's condition extends to:

```rust
if self.hide.images || (self.hide.images_in_links && self.in_link) {
    self.image_alt.clear();
    self.image_url.clear();
    self.in_image = false;
    return;
}
```

`self.in_link` already exists and is tracked correctly across nested
image events — it's set `true` on `Tag::Link` and `false` on
`TagEnd::Link`, and used for the (unrelated) purpose of attaching the link
URL to an image's caption span. No new state is needed.

### Why this is the only viable implementation point

Two alternatives were considered and rejected:

- **Post-processing filter** on already-built `LineMeta::Image` lines:
  would require threading a new "was this image inside a link" field
  through `LineMeta::Image` just to filter it back out afterward —
  strictly more complex than suppressing at the source with no benefit.
- **Detection in `wikilink.rs`'s preprocessing**: wrong layer. Link/image
  nesting is inline CommonMark structure that only exists once
  `pulldown-cmark` parses the document; a plain
  `[text ![img](path)](url)` link isn't wikilink syntax at all, so
  `wikilink.rs`'s regex-based line rewriting never sees it as anything
  special.

### Error handling

None needed. This is a pure rendering-suppression toggle — no filesystem
or network path is involved. A suppressed image is never fetched at all
(same as `hide.images` today), so there's no failure mode to handle beyond
what already exists for `hide.images`.

### Testing

Unit tests in `src/markdown.rs`'s existing test module, following the
`hide_images()` test-config helper pattern already there:

1. A link containing a nested image, with `images_in_links = true` →
   produces no `LineMeta::Image` line; the link's surrounding text
   renders normally on a single line.
2. The same input with `images_in_links = false` (the default) →
   unaffected — regression coverage proving the existing behavior (image
   renders as a block inside the link) is unchanged when the flag is off.
3. A *standalone* image (not inside a link) with `images_in_links = true`
   → **not** suppressed — proving the two hide flags are independent and
   this one never touches body images.
4. A wikilink embed (`![[icon.png]]`) nested inside a link, with
   `images_in_links = true` → also suppressed, confirming the trigger is
   link-nesting, not image syntax.

## Out of scope

- Any notion of true inline (same-row, small) image rendering. mdterm has
  no such capability for any image, and this feature does not add one —
  it only suppresses the block entirely, per the "fully suppressed" choice
  above. When an image is the entire link label (no other text), the whole
  link disappears as well, since there's nothing left for a link picker to
  discover. If the alt text is needed as an at-a-glance signal, the author's
  own workaround (a plain Unicode/emoji character in the link label
  instead of a real embedded image) remains the way to get that.
- Obsidian's embed-sizing syntax (`![[img.png|WIDTHxHEIGHT]]`) — raised in
  the same conversation, tracked separately, not part of this design.
