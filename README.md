# mdterm

A terminal-based Markdown viewer written in Rust. Renders Markdown files with syntax highlighting, styled formatting, and interactive navigation.

## Features

- **Interactive TUI** — Scroll, navigate with keyboard and mouse
- **Syntax highlighting** — Code blocks highlighted via syntect (base16-ocean.dark / InspiredGitHub themes)
- **Rich formatting** — Headings, bold, italic, strikethrough, lists, blockquotes, tables, task lists
- **Inline images** — Renders images in the terminal via Kitty, iTerm2, or Unicode half-block fallback
- **Clickable links** — OSC 8 hyperlinks in supporting terminals
- **In-document search** — `/` to search with regex support, `n`/`N` to jump between matches
- **Table of contents** — Press `o` to browse and jump to any heading
- **Fuzzy heading search** — Press `:` to filter headings by name
- **Heading jumps** — `[` / `]` to jump between sections
- **Link picker** — Press `f` to list all links, type a number to open in browser
- **Clipboard** — `y` copies current section, `Y` copies full document, `c` copies a code block
- **Mermaid diagrams** — Visual rendering of flowcharts/graphs in the terminal with box-drawing characters
- **Math rendering** — LaTeX to Unicode: `$\alpha + \beta$` renders as `α + β`
- **Slide mode** — `--slides` treats `---` as slide separators for terminal presentations
- **Follow mode** — `--follow` watches the file and auto-reloads on changes
- **Stdin support** — Pipe markdown from any command: `curl ... | mdterm`
- **Multiple files** — `mdterm a.md b.md`, switch with `Tab` / `Shift+Tab`
- **HTML export** — `--export html` outputs themed, self-contained HTML
- **Dark/light themes** — Toggle with `t`, or set via `--theme` / config file
- **Line numbers** — Toggle with `l` for code blocks
- **Config file** — `~/.config/mdterm/config.toml` for persistent preferences
- **Word wrapping** — Responsive re-wrapping on terminal resize
- **Pipe-friendly** — Outputs plain styled text when stdout is piped

## Installation

Requires Rust 1.85+ (edition 2024).

```bash
cargo install --path .
```

## Usage

```bash
mdterm README.md                    # view a file
mdterm a.md b.md                    # multiple files (Tab to switch)
cat README.md | mdterm              # read from stdin
mdterm --slides deck.md             # slide mode
mdterm --follow notes.md            # auto-reload on changes
mdterm --export html doc.md > out.html  # export to HTML
mdterm --theme light README.md      # light theme
mdterm -l README.md                 # line numbers in code blocks
```

When piped, mdterm outputs styled text without the interactive viewer:

```bash
mdterm README.md | less -R
```

## Controls

### Navigation

| Key | Action |
|-----|--------|
| `j` / `Down` | Scroll down one line |
| `k` / `Up` | Scroll up one line |
| `Space` / `Page Down` | Page down |
| `b` / `Page Up` | Page up |
| `Ctrl+d` / `Ctrl+u` | Half-page down / up |
| `g` / `Home` | Jump to top |
| `G` / `End` | Jump to bottom |
| `[` / `]` | Previous / next heading |
| Mouse scroll | Scroll up/down |

### Search

| Key | Action |
|-----|--------|
| `/` | Open search (supports regex) |
| `Enter` | Execute search |
| `n` / `N` | Next / previous match |
| `Esc` | Clear search |

### Features

| Key | Action |
|-----|--------|
| `o` | Table of contents overlay |
| `:` | Fuzzy heading search |
| `f` | Link picker (open in browser) |
| `t` | Toggle dark/light theme |
| `l` | Toggle line numbers in code blocks |
| `y` | Copy current section to clipboard |
| `Y` | Copy entire document to clipboard |
| `c` | Copy nearest code block to clipboard |
| `Tab` / `Shift+Tab` | Switch between files |
| `q` / `Ctrl+C` | Quit |

### Slide Mode (`--slides`)

| Key | Action |
|-----|--------|
| `Right` / `Space` / `l` / `j` / `Down` / `Page Down` | Next slide |
| `Left` / `b` / `h` / `k` / `Up` / `Page Up` | Previous slide |
| `g` / `Home` | First slide |
| `G` / `End` | Last slide |

## Configuration

Create `~/.config/mdterm/config.toml`:

```toml
theme = "dark"          # "dark" or "light"
line_numbers = false     # show line numbers in code blocks
width = 0               # display width (0 = auto)
```

CLI flags override config file settings.

## CLI Reference

```
mdterm [OPTIONS] [FILES]...

Arguments:
  [FILES]...               Markdown file(s) to view

Options:
  -T, --theme <THEME>      Theme: dark or light
  -w, --width <WIDTH>      Display width override (0 = auto)
  -s, --slides             Slide mode (--- as slide separators)
  -f, --follow             Watch file for changes and auto-reload
  -l, --line-numbers       Show line numbers in code blocks
      --export <FORMAT>    Export format (html)
      --no-color           Disable colors
  -h, --help               Print help
  -V, --version            Print version
```

## Building

```bash
cargo build --release
```

## License

MIT
