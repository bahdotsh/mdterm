# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

mdterm is a terminal-based Markdown viewer written in Rust. It renders Markdown files with syntax highlighting, styled formatting, and interactive navigation (scrolling, keyboard/mouse controls). When stdout is piped, it outputs plain styled text instead of the interactive TUI.

## Build Commands

```bash
cargo build              # debug build
cargo build --release    # release build
cargo run -- <file.md>   # run with a markdown file
cargo check              # type-check without building
cargo clippy             # lint
cargo fmt                # format code
cargo test               # run tests
```

## Architecture

Nine source files in `src/`:

- **main.rs** — Entry point. Uses `clap` for CLI arg parsing, handles stdin/file input, dispatches to viewer (TTY), piped output, or HTML export.
- **markdown.rs** — Stateful markdown renderer. Processes `pulldown-cmark` events into `(Vec<Line>, DocumentInfo)`. Handles syntax highlighting, math rendering (LaTeX→Unicode), image placeholders, line numbers, mermaid diagram rendering, and metadata tracking (headings, code blocks, slide breaks).
- **style.rs** — Data types (`Style`, `StyledSpan`, `Line`, `LineMeta`, `DocumentInfo`) and word-wrapping logic. `LineMeta` tracks heading/code-block/slide metadata through wrapping.
- **viewer.rs** — Interactive TUI with multiple view modes (Normal, Search, TOC, LinkPicker, FuzzyHeading). Supports slide mode, auto-reload on file changes (via notify crate), multi-file switching, clipboard operations, regex search, overlay panels, and image rendering.
- **theme.rs** — Two complete themes (dark/light) with 40+ color fields including overlay, math, image, and line number colors.
- **config.rs** — Loads `~/.config/mdterm/config.toml` for persistent settings (theme, line_numbers, width).
- **export.rs** — HTML export with inline CSS matching the current theme.
- **image.rs** — Terminal image rendering with three protocols: Kitty (ID-based upload/placement), iTerm2 (inline image sequences), and Unicode half-block fallback. Fetches images on background threads via `std::sync::mpsc` (non-blocking — `start_fetch()` spawns a thread per URL, `poll_completed()` drains results each event-loop tick). Also handles downscaling, caching, and terminal cell metric detection.
- **diagram.rs** — Mermaid flowchart parser and ASCII art renderer. Supports node shapes (rect, rounded, diamond, circle), edge labels, topological layering, and barycenter layout optimization.

**Data flow:** markdown text → `pulldown-cmark` events → `Renderer` (markdown.rs) → `(Vec<Line>, DocumentInfo)` → `wrap_lines` (style.rs) → terminal/HTML output

## Key Dependencies

- **pulldown-cmark 0.11** — CommonMark parser (events/AST, math support)
- **crossterm 0.28** — Terminal control (raw mode, colors, events)
- **syntect 5** — Syntax highlighting for code blocks
- **clap 4** — CLI argument parsing
- **regex 1** — Regex search support
- **open 5** — Open URLs in browser (link picker)
- **serde + toml** — Config file parsing
- **dirs 5** — Platform config directory lookup
- **image 0.25** — Image loading and processing (PNG, JPEG, GIF, WebP, BMP, ICO, TIFF)
- **libc 0.2** — Unix FFI for terminal cell pixel metrics (ioctl TIOCGWINSZ)
- **base64 0.22** — Base64 encoding for image protocol escape sequences
- **ureq 3** — Pure-Rust HTTP client for fetching remote images (replaces shelling out to `curl`)
- **notify 7** — Cross-platform filesystem watcher (inotify/FSEvents/kqueue) for auto-reload

## Rust Edition

Uses Rust edition 2024 (requires rustc 1.85+).
