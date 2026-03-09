use std::io::{self, Write};
use std::time::Duration;

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEventKind,
    },
    execute, queue,
    style::{Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode, size,
    },
};

use crate::markdown::SyntectRes;
use crate::style::{DocumentInfo, Line, LineMeta, StyledSpan, wrap_lines};
use crate::theme::Theme;

// ── Public API ──────────────────────────────────────────────────────────────

pub struct ViewerOptions {
    pub files: Vec<String>,
    pub initial_content: String,
    pub filename: String,
    pub theme: Theme,
    pub slide_mode: bool,
    pub follow_mode: bool,
    pub line_numbers: bool,
    pub width_override: Option<usize>,
}

pub fn run(opts: ViewerOptions) -> io::Result<()> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, Hide, EnableMouseCapture)?;
    let _guard = TerminalGuard;

    let (cols, rows) = size()?;
    let mut state = ViewerState::new(opts, cols, rows);
    state.rebuild();

    loop {
        let max_offset = state.max_offset();
        state.offset = state.offset.min(max_offset);

        render_frame(&mut stdout, &state)?;

        // Clear transient status after rendering so it shows for one frame
        state.status_msg = None;

        let timeout = if state.follow_mode {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(3600)
        };

        if event::poll(timeout)? {
            let ev = event::read()?;
            if handle_event(&mut state, ev) {
                break;
            }
        } else if state.follow_mode {
            state.check_file_changed();
        }
    }

    Ok(())
}

pub fn print_lines(lines: &[Line]) {
    let mut stdout = io::stdout();
    for line in lines {
        for span in &line.spans {
            let _ = write_span(&mut stdout, span, None);
        }
        let _ = writeln!(stdout);
    }
}

pub fn print_lines_plain(lines: &[Line]) {
    let mut stdout = io::stdout();
    for line in lines {
        for span in &line.spans {
            let _ = write!(stdout, "{}", span.text);
        }
        let _ = writeln!(stdout);
    }
}

// ── Terminal guard ──────────────────────────────────────────────────────────

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = execute!(stdout, DisableMouseCapture, Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

// ── View modes ──────────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum ViewMode {
    Normal,
    Search,
    Toc,
    LinkPicker,
    FuzzyHeading,
}

// ── Viewer state ────────────────────────────────────────────────────────────

struct ViewerState {
    // File management
    files: Vec<String>,
    current_file_idx: usize,
    content: String,
    filename: String,

    // Display
    theme: Theme,
    wrapped: Vec<Line>,
    doc_info: DocumentInfo,
    offset: usize,
    cols: u16,
    rows: u16,

    // Syntect resources (loaded once)
    syntect_res: SyntectRes,

    // Options
    slide_mode: bool,
    follow_mode: bool,
    line_numbers: bool,
    width_override: Option<usize>,

    // Mode
    mode: ViewMode,

    // Search
    search: SearchState,

    // TOC
    toc_entries: Vec<TocEntry>,
    toc_selected: usize,
    toc_scroll: usize,

    // Link picker
    link_entries: Vec<LinkEntry>,
    link_input: String,

    // Fuzzy heading search
    fuzzy_input: String,
    fuzzy_selected: usize,
    fuzzy_scroll: usize,

    // Slide mode
    current_slide: usize,
    slide_boundaries: Vec<usize>, // wrapped line indices

    // Follow mode
    last_mtime: Option<std::time::SystemTime>,

    // Status message
    status_msg: Option<String>,
}

#[derive(Clone)]
struct TocEntry {
    line_idx: usize,
    level: u8,
    text: String,
}

#[derive(Clone)]
#[allow(dead_code)]
struct LinkEntry {
    url: String,
    text: String,
}

impl ViewerState {
    fn new(opts: ViewerOptions, cols: u16, rows: u16) -> Self {
        let last_mtime = if opts.follow_mode && !opts.files.is_empty() {
            std::fs::metadata(
                &opts.files[opts
                    .files
                    .iter()
                    .position(|f| *f == opts.filename)
                    .unwrap_or(0)],
            )
            .and_then(|m| m.modified())
            .ok()
        } else {
            None
        };

        ViewerState {
            files: opts.files,
            current_file_idx: 0,
            content: opts.initial_content,
            filename: opts.filename,
            theme: opts.theme,
            wrapped: Vec::new(),
            doc_info: DocumentInfo {
                code_blocks: Vec::new(),
            },
            offset: 0,
            cols,
            rows,
            syntect_res: SyntectRes::load(),
            slide_mode: opts.slide_mode,
            follow_mode: opts.follow_mode,
            line_numbers: opts.line_numbers,
            width_override: opts.width_override,
            mode: ViewMode::Normal,
            search: SearchState::new(),
            toc_entries: Vec::new(),
            toc_selected: 0,
            toc_scroll: 0,
            link_entries: Vec::new(),
            link_input: String::new(),
            fuzzy_input: String::new(),
            fuzzy_selected: 0,
            fuzzy_scroll: 0,
            current_slide: 0,
            slide_boundaries: Vec::new(),
            last_mtime,
            status_msg: None,
        }
    }

    fn content_width(&self) -> usize {
        if let Some(w) = self.width_override {
            w.saturating_sub(4)
        } else {
            (self.cols as usize).saturating_sub(4)
        }
    }

    fn viewport(&self) -> usize {
        (self.rows as usize).saturating_sub(2)
    }

    fn max_offset(&self) -> usize {
        if self.slide_mode {
            return 0; // slides handle their own offset
        }
        self.wrapped.len().saturating_sub(self.viewport())
    }

    fn rebuild(&mut self) {
        let cw = self.content_width();
        let (lines, doc_info) = crate::markdown::render_with(
            &self.content,
            cw,
            &self.theme,
            self.line_numbers,
            &self.syntect_res,
        );
        self.wrapped = wrap_lines(&lines, cw);
        self.doc_info = doc_info;

        // Build TOC
        self.toc_entries.clear();
        for (i, line) in self.wrapped.iter().enumerate() {
            if let LineMeta::Heading { level, ref text } = line.meta {
                self.toc_entries.push(TocEntry {
                    line_idx: i,
                    level,
                    text: text.clone(),
                });
            }
        }

        // Build link list
        self.link_entries.clear();
        let mut seen_urls = std::collections::HashSet::new();
        for line in &self.wrapped {
            for span in &line.spans {
                if let Some(ref url) = span.style.link_url
                    && seen_urls.insert(url.clone())
                {
                    let text = span.text.trim().to_string();
                    self.link_entries.push(LinkEntry {
                        url: url.clone(),
                        text,
                    });
                }
            }
        }

        // Build slide boundaries
        if self.slide_mode {
            self.slide_boundaries.clear();
            self.slide_boundaries.push(0);
            for (i, line) in self.wrapped.iter().enumerate() {
                if matches!(line.meta, LineMeta::SlideBreak) {
                    self.slide_boundaries.push(i + 1);
                }
            }
        }

        // Re-search if active
        if self.search.has_results() {
            self.search.find_matches(&self.wrapped);
            self.search.jump_nearest(self.offset);
        }

        let max = self.max_offset();
        self.offset = self.offset.min(max);
    }

    fn check_file_changed(&mut self) {
        if self.files.is_empty() {
            return;
        }
        let path = &self.files[self.current_file_idx];
        if let Ok(meta) = std::fs::metadata(path)
            && let Ok(mtime) = meta.modified()
        {
            if self.last_mtime.is_some()
                && Some(mtime) != self.last_mtime
                && let Ok(new_content) = std::fs::read_to_string(path)
            {
                self.content = new_content;
                self.rebuild();
                self.status_msg = Some("File reloaded".into());
            }
            self.last_mtime = Some(mtime);
        }
    }

    fn switch_file(&mut self, idx: usize) {
        if idx >= self.files.len() || idx == self.current_file_idx {
            return;
        }
        let path = self.files[idx].clone();
        if let Ok(c) = std::fs::read_to_string(&path) {
            self.current_file_idx = idx;
            self.filename = path.clone();
            self.content = c;
            self.offset = 0;
            self.search.clear();
            self.current_slide = 0;
            self.rebuild();
            if self.follow_mode {
                self.last_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            }
        }
    }

    fn heading_lines(&self) -> Vec<usize> {
        self.toc_entries.iter().map(|e| e.line_idx).collect()
    }

    fn find_code_block_at_offset(&self) -> Option<usize> {
        let line_idx = self.offset + self.viewport() / 2;
        // Search around the center of the viewport
        for delta in 0..self.viewport() {
            for &idx in &[line_idx.wrapping_sub(delta), line_idx + delta] {
                if let Some(line) = self.wrapped.get(idx)
                    && let LineMeta::CodeContent { block_id } = line.meta
                {
                    return Some(block_id);
                }
            }
        }
        None
    }

    fn visible_section_text(&self) -> String {
        // Find current heading section
        let headings = self.heading_lines();
        let current_pos = self.offset;

        let start = headings
            .iter()
            .rev()
            .find(|&&h| h <= current_pos)
            .copied()
            .unwrap_or(0);
        let end = headings
            .iter()
            .find(|&&h| h > current_pos)
            .copied()
            .unwrap_or(self.wrapped.len());

        self.wrapped[start..end]
            .iter()
            .map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn full_text(&self) -> String {
        self.wrapped
            .iter()
            .map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ── Event handling ──────────────────────────────────────────────────────────

fn handle_event(state: &mut ViewerState, ev: Event) -> bool {
    match ev {
        Event::Key(ke) if ke.kind == KeyEventKind::Press => {
            if ke.code == KeyCode::Char('c') && ke.modifiers.contains(KeyModifiers::CONTROL) {
                return true;
            }
            match state.mode {
                ViewMode::Normal => return handle_normal(state, ke.code, ke.modifiers),
                ViewMode::Search => handle_search(state, ke.code),
                ViewMode::Toc => handle_toc(state, ke.code),
                ViewMode::LinkPicker => handle_link_picker(state, ke.code),
                ViewMode::FuzzyHeading => handle_fuzzy(state, ke.code, ke.modifiers),
            }
        }
        Event::Mouse(me) => match me.kind {
            MouseEventKind::ScrollDown => match state.mode {
                ViewMode::Toc => {
                    handle_toc(state, KeyCode::Down);
                }
                ViewMode::FuzzyHeading => {
                    handle_fuzzy(state, KeyCode::Down, KeyModifiers::empty());
                }
                _ if state.slide_mode => {
                    if state.current_slide + 1 < state.slide_boundaries.len() {
                        state.current_slide += 1;
                    }
                }
                _ => {
                    let max = state.max_offset();
                    state.offset = (state.offset + 3).min(max);
                }
            },
            MouseEventKind::ScrollUp => match state.mode {
                ViewMode::Toc => {
                    handle_toc(state, KeyCode::Up);
                }
                ViewMode::FuzzyHeading => {
                    handle_fuzzy(state, KeyCode::Up, KeyModifiers::empty());
                }
                _ if state.slide_mode => {
                    state.current_slide = state.current_slide.saturating_sub(1);
                }
                _ => {
                    state.offset = state.offset.saturating_sub(3);
                }
            },
            _ => {}
        },
        Event::Resize(c, r) => {
            state.cols = c;
            state.rows = r;
            state.rebuild();
        }
        _ => {}
    }
    false
}

fn handle_normal(state: &mut ViewerState, code: KeyCode, mods: KeyModifiers) -> bool {
    let viewport = state.viewport();
    let max_offset = state.max_offset();

    if state.slide_mode {
        return handle_slide_keys(state, code);
    }

    match code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc => {
            if state.search.has_results() {
                state.search.clear();
            } else {
                return true;
            }
        }

        // Theme toggle
        KeyCode::Char('t') => {
            state.theme = state.theme.toggle();
            state.rebuild();
        }

        // Line numbers toggle
        KeyCode::Char('l') => {
            state.line_numbers = !state.line_numbers;
            state.rebuild();
            state.status_msg = Some(if state.line_numbers {
                "Line numbers ON".into()
            } else {
                "Line numbers OFF".into()
            });
        }

        // Search
        KeyCode::Char('/') => {
            state.mode = ViewMode::Search;
            state.search.input_active = true;
            state.search.input_buf.clear();
        }
        KeyCode::Char('n') if state.search.has_results() => {
            state.search.next();
            scroll_to_match(&state.search, &mut state.offset, viewport, max_offset);
        }
        KeyCode::Char('N') if state.search.has_results() => {
            state.search.prev();
            scroll_to_match(&state.search, &mut state.offset, viewport, max_offset);
        }

        // TOC
        KeyCode::Char('o') => {
            if !state.toc_entries.is_empty() {
                state.toc_selected = 0;
                state.toc_scroll = 0;
                // Try to select the heading closest to current offset
                for (i, entry) in state.toc_entries.iter().enumerate() {
                    if entry.line_idx <= state.offset {
                        state.toc_selected = i;
                    }
                }
                // Ensure scroll shows the selected entry
                let viewport = state.viewport();
                let count = state.toc_entries.len();
                let box_h = (count + 2).min(viewport.saturating_sub(4));
                let visible_entries = box_h.saturating_sub(2);
                if visible_entries > 0 && state.toc_selected >= visible_entries {
                    state.toc_scroll = state.toc_selected - visible_entries + 1;
                }
                state.mode = ViewMode::Toc;
            }
        }

        // Link picker
        KeyCode::Char('f') => {
            if !state.link_entries.is_empty() {
                state.link_input.clear();
                state.mode = ViewMode::LinkPicker;
            }
        }

        // Fuzzy heading search
        KeyCode::Char(':') => {
            if !state.toc_entries.is_empty() {
                state.fuzzy_input.clear();
                state.fuzzy_selected = 0;
                state.fuzzy_scroll = 0;
                state.mode = ViewMode::FuzzyHeading;
            }
        }

        // Copy section (y) or full document (Y)
        KeyCode::Char('y') => {
            let text = state.visible_section_text();
            if copy_to_clipboard(&text).is_ok() {
                state.status_msg = Some("Section copied".into());
            }
        }
        KeyCode::Char('Y') => {
            let text = state.full_text();
            if copy_to_clipboard(&text).is_ok() {
                state.status_msg = Some("Document copied".into());
            }
        }

        // Code block copy
        KeyCode::Char('c') => {
            if let Some(block_id) = state.find_code_block_at_offset()
                && let Some(block) = state.doc_info.code_blocks.get(block_id)
                && copy_to_clipboard(&block.content).is_ok()
            {
                state.status_msg = Some("Code block copied".into());
            }
        }

        // Heading jumps: [ prev, ] next
        KeyCode::Char('[') => {
            let headings = state.heading_lines();
            if let Some(&target) = headings.iter().rev().find(|&&h| h < state.offset) {
                state.offset = target.min(max_offset);
            }
        }
        KeyCode::Char(']') => {
            let headings = state.heading_lines();
            if let Some(&target) = headings.iter().find(|&&h| h > state.offset) {
                state.offset = target.min(max_offset);
            }
        }

        // File switching
        KeyCode::Tab => {
            if state.files.len() > 1 {
                let next = (state.current_file_idx + 1) % state.files.len();
                state.switch_file(next);
            }
        }
        KeyCode::BackTab => {
            if state.files.len() > 1 {
                let prev = if state.current_file_idx == 0 {
                    state.files.len() - 1
                } else {
                    state.current_file_idx - 1
                };
                state.switch_file(prev);
            }
        }

        // Navigation
        KeyCode::Down | KeyCode::Char('j') => {
            state.offset = (state.offset + 1).min(max_offset);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.offset = state.offset.saturating_sub(1);
        }
        KeyCode::Char(' ') | KeyCode::PageDown => {
            state.offset = (state.offset + viewport).min(max_offset);
        }
        KeyCode::Char('d') if mods.contains(KeyModifiers::CONTROL) => {
            state.offset = (state.offset + viewport / 2).min(max_offset);
        }
        KeyCode::Char('u') if mods.contains(KeyModifiers::CONTROL) => {
            state.offset = state.offset.saturating_sub(viewport / 2);
        }
        KeyCode::Char('b') | KeyCode::PageUp => {
            state.offset = state.offset.saturating_sub(viewport);
        }
        KeyCode::Char('g') | KeyCode::Home => {
            state.offset = 0;
        }
        KeyCode::Char('G') | KeyCode::End => {
            state.offset = max_offset;
        }
        _ => {}
    }
    false
}

fn handle_slide_keys(state: &mut ViewerState, code: KeyCode) -> bool {
    let num_slides = state.slide_boundaries.len().max(1);
    match code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Right
        | KeyCode::Char(' ')
        | KeyCode::Char('l')
        | KeyCode::Char('j')
        | KeyCode::Down
        | KeyCode::PageDown => {
            if state.current_slide + 1 < num_slides {
                state.current_slide += 1;
            }
        }
        KeyCode::Left
        | KeyCode::Char('h')
        | KeyCode::Char('k')
        | KeyCode::Up
        | KeyCode::PageUp
        | KeyCode::Char('b') => {
            state.current_slide = state.current_slide.saturating_sub(1);
        }
        KeyCode::Char('g') | KeyCode::Home => {
            state.current_slide = 0;
        }
        KeyCode::Char('G') | KeyCode::End => {
            state.current_slide = num_slides.saturating_sub(1);
        }
        KeyCode::Char('t') => {
            state.theme = state.theme.toggle();
            state.rebuild();
        }
        _ => {}
    }
    false
}

fn handle_search(state: &mut ViewerState, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            state.search.input_active = false;
            state.search.input_buf.clear();
            state.mode = ViewMode::Normal;
        }
        KeyCode::Enter => {
            state.search.input_active = false;
            state.search.execute(&state.wrapped);
            state.search.jump_nearest(state.offset);
            let viewport = state.viewport();
            let max_offset = state.max_offset();
            scroll_to_match(&state.search, &mut state.offset, viewport, max_offset);
            state.mode = ViewMode::Normal;
        }
        KeyCode::Backspace => {
            state.search.input_buf.pop();
        }
        KeyCode::Char(c) => {
            state.search.input_buf.push(c);
        }
        _ => {}
    }
}

fn handle_toc(state: &mut ViewerState, code: KeyCode) {
    let count = state.toc_entries.len();
    if count == 0 {
        state.mode = ViewMode::Normal;
        return;
    }

    let viewport = state.viewport();
    let box_h = (count + 2).min(viewport.saturating_sub(4).max(3));
    let visible_entries = box_h.saturating_sub(2).max(1);

    match code {
        KeyCode::Esc | KeyCode::Char('o') | KeyCode::Char('q') => {
            state.mode = ViewMode::Normal;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.toc_selected = state.toc_selected.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.toc_selected + 1 < count {
                state.toc_selected += 1;
            }
        }
        KeyCode::PageUp => {
            state.toc_selected = state.toc_selected.saturating_sub(visible_entries);
        }
        KeyCode::PageDown => {
            state.toc_selected = (state.toc_selected + visible_entries).min(count - 1);
        }
        KeyCode::Home | KeyCode::Char('g') => {
            state.toc_selected = 0;
        }
        KeyCode::End | KeyCode::Char('G') => {
            state.toc_selected = count.saturating_sub(1);
        }
        KeyCode::Enter => {
            let target = state.toc_entries[state.toc_selected].line_idx;
            let max = state.max_offset();
            state.offset = target.min(max);
            state.mode = ViewMode::Normal;
        }
        _ => {}
    }

    // Update scroll to keep selection visible
    if state.toc_selected >= state.toc_scroll + visible_entries {
        state.toc_scroll = state.toc_selected - visible_entries + 1;
    } else if state.toc_selected < state.toc_scroll {
        state.toc_scroll = state.toc_selected;
    }
}

fn handle_link_picker(state: &mut ViewerState, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            state.mode = ViewMode::Normal;
        }
        KeyCode::Char(c) if c.is_ascii_digit() => {
            state.link_input.push(c);
        }
        KeyCode::Backspace => {
            state.link_input.pop();
        }
        KeyCode::Enter => {
            if let Ok(num) = state.link_input.parse::<usize>()
                && num >= 1
                && num <= state.link_entries.len()
            {
                let url = state.link_entries[num - 1].url.clone();
                let _ = open::that(&url);
                state.status_msg = Some(format!("Opened: {}", url));
            }
            state.mode = ViewMode::Normal;
        }
        _ => {}
    }
}

fn handle_fuzzy(state: &mut ViewerState, code: KeyCode, mods: KeyModifiers) {
    let viewport = state.viewport();
    let max_visible = viewport.saturating_sub(6).max(1);

    // Ctrl+n / Ctrl+p for navigation without conflicting with typing
    let is_nav_down = code == KeyCode::Down
        || code == KeyCode::PageDown
        || (code == KeyCode::Char('n') && mods.contains(KeyModifiers::CONTROL));
    let is_nav_up = code == KeyCode::Up
        || code == KeyCode::PageUp
        || (code == KeyCode::Char('p') && mods.contains(KeyModifiers::CONTROL));

    if is_nav_up {
        let step = if code == KeyCode::PageUp {
            max_visible
        } else {
            1
        };
        state.fuzzy_selected = state.fuzzy_selected.saturating_sub(step);
    } else if is_nav_down {
        let step = if code == KeyCode::PageDown {
            max_visible
        } else {
            1
        };
        state.fuzzy_selected += step;
        // Will be clamped below
    } else {
        match code {
            KeyCode::Esc => {
                state.mode = ViewMode::Normal;
                return;
            }
            KeyCode::Char(c) => {
                state.fuzzy_input.push(c);
                state.fuzzy_selected = 0;
                state.fuzzy_scroll = 0;
            }
            KeyCode::Backspace => {
                state.fuzzy_input.pop();
                state.fuzzy_selected = 0;
                state.fuzzy_scroll = 0;
            }
            KeyCode::Enter => {
                let filtered = fuzzy_filter(&state.toc_entries, &state.fuzzy_input);
                if let Some(entry) = filtered.get(state.fuzzy_selected) {
                    let target = entry.line_idx;
                    let max = state.max_offset();
                    state.offset = target.min(max);
                }
                state.mode = ViewMode::Normal;
                return;
            }
            _ => {}
        }
    }

    // Clamp selected to filtered results and update scroll
    let count = fuzzy_filter(&state.toc_entries, &state.fuzzy_input).len();
    if count == 0 {
        state.fuzzy_selected = 0;
        state.fuzzy_scroll = 0;
    } else {
        state.fuzzy_selected = state.fuzzy_selected.min(count - 1);
        let visible = count.min(max_visible);
        if state.fuzzy_selected >= state.fuzzy_scroll + visible {
            state.fuzzy_scroll = state.fuzzy_selected - visible + 1;
        } else if state.fuzzy_selected < state.fuzzy_scroll {
            state.fuzzy_scroll = state.fuzzy_selected;
        }
    }
}

fn fuzzy_filter(entries: &[TocEntry], query: &str) -> Vec<TocEntry> {
    if query.is_empty() {
        return entries.to_vec();
    }
    let q = query.to_lowercase();
    entries
        .iter()
        .filter(|e| e.text.to_lowercase().contains(&q))
        .cloned()
        .collect()
}

// ── Search state ────────────────────────────────────────────────────────────

struct SearchMatch {
    line: usize,
    start: usize,
    end: usize,
}

struct SearchState {
    query: String,
    input_buf: String,
    matches: Vec<SearchMatch>,
    current_idx: usize,
    input_active: bool,
    use_regex: bool,
}

impl SearchState {
    fn new() -> Self {
        Self {
            query: String::new(),
            input_buf: String::new(),
            matches: Vec::new(),
            current_idx: 0,
            input_active: false,
            use_regex: false,
        }
    }

    fn execute(&mut self, lines: &[Line]) {
        self.query = self.input_buf.clone();
        // Auto-detect regex: if the query contains regex metacharacters
        self.use_regex = self.query.contains('\\')
            || self.query.contains('[')
            || self.query.contains('(')
            || self.query.contains('+')
            || self.query.contains('*')
            || self.query.contains('?')
            || self.query.contains('^')
            || self.query.contains('$')
            || self.query.contains('|');

        if self.use_regex
            && let Ok(re) = regex::RegexBuilder::new(&self.query)
                .case_insensitive(true)
                .build()
        {
            self.find_matches_regex(lines, &re);
            return;
        }
        self.find_matches_literal(lines);
    }

    fn find_matches(&mut self, lines: &[Line]) {
        if self.query.is_empty() {
            return;
        }
        if self.use_regex
            && let Ok(re) = regex::RegexBuilder::new(&self.query)
                .case_insensitive(true)
                .build()
        {
            self.find_matches_regex(lines, &re);
            return;
        }
        self.find_matches_literal(lines);
    }

    fn find_matches_literal(&mut self, lines: &[Line]) {
        self.matches.clear();
        self.current_idx = 0;
        if self.query.is_empty() {
            return;
        }
        let query_lower = self.query.to_lowercase();
        let qbyte_len = query_lower.len();
        let qchar_len = query_lower.chars().count();
        for (line_idx, line) in lines.iter().enumerate() {
            let text: String = line.spans.iter().map(|s| s.text.as_str()).collect();
            let text_lower = text.to_lowercase();
            let mut pos = 0;
            while pos < text_lower.len() {
                if let Some(found) = text_lower[pos..].find(&query_lower) {
                    let byte_start = pos + found;
                    let char_start = text_lower[..byte_start].chars().count();
                    self.matches.push(SearchMatch {
                        line: line_idx,
                        start: char_start,
                        end: char_start + qchar_len,
                    });
                    pos = byte_start + qbyte_len;
                } else {
                    break;
                }
            }
        }
    }

    fn find_matches_regex(&mut self, lines: &[Line], re: &regex::Regex) {
        self.matches.clear();
        self.current_idx = 0;
        for (line_idx, line) in lines.iter().enumerate() {
            let text: String = line.spans.iter().map(|s| s.text.as_str()).collect();
            for mat in re.find_iter(&text) {
                let char_start = text[..mat.start()].chars().count();
                let char_end = text[..mat.end()].chars().count();
                if char_start < char_end {
                    self.matches.push(SearchMatch {
                        line: line_idx,
                        start: char_start,
                        end: char_end,
                    });
                }
            }
        }
    }

    fn jump_nearest(&mut self, viewport_offset: usize) {
        if let Some(idx) = self.matches.iter().position(|m| m.line >= viewport_offset) {
            self.current_idx = idx;
        } else if !self.matches.is_empty() {
            self.current_idx = 0;
        }
    }

    fn next(&mut self) {
        if !self.matches.is_empty() {
            self.current_idx = (self.current_idx + 1) % self.matches.len();
        }
    }

    fn prev(&mut self) {
        if !self.matches.is_empty() {
            self.current_idx = self
                .current_idx
                .checked_sub(1)
                .unwrap_or(self.matches.len() - 1);
        }
    }

    fn current_line(&self) -> Option<usize> {
        self.matches.get(self.current_idx).map(|m| m.line)
    }

    fn has_results(&self) -> bool {
        !self.query.is_empty()
    }

    fn clear(&mut self) {
        self.query.clear();
        self.matches.clear();
        self.current_idx = 0;
    }

    fn highlights_for_line(&self, line_idx: usize) -> Vec<(usize, usize, bool)> {
        self.matches
            .iter()
            .enumerate()
            .filter(|(_, m)| m.line == line_idx)
            .map(|(i, m)| (m.start, m.end, i == self.current_idx))
            .collect()
    }
}

fn scroll_to_match(search: &SearchState, offset: &mut usize, viewport: usize, max_offset: usize) {
    if let Some(target) = search.current_line()
        && (target < *offset || target >= *offset + viewport)
    {
        *offset = target.saturating_sub(viewport / 3).min(max_offset);
    }
}

// ── Clipboard ───────────────────────────────────────────────────────────────

fn copy_to_clipboard(text: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut child = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Try xclip, then xsel
        if let Ok(mut child) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()?;
            return Ok(());
        }
        if let Ok(mut child) = std::process::Command::new("xsel")
            .arg("--clipboard")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()?;
            return Ok(());
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "No clipboard tool found",
        ))
    }
}

// ── Rendering ───────────────────────────────────────────────────────────────

fn render_frame(stdout: &mut io::Stdout, state: &ViewerState) -> io::Result<()> {
    let width = state.cols as usize;
    let viewport = state.viewport();
    let content_width = width.saturating_sub(4);
    let theme = &state.theme;

    // Determine which lines to show
    let (_display_lines, _display_offset) = if state.slide_mode {
        let start = state
            .slide_boundaries
            .get(state.current_slide)
            .copied()
            .unwrap_or(0);
        let end = state
            .slide_boundaries
            .get(state.current_slide + 1)
            .copied()
            .unwrap_or(state.wrapped.len());
        let slice = &state.wrapped[start..end.min(state.wrapped.len())];
        (slice, start)
    } else {
        (state.wrapped.as_slice(), state.offset)
    };

    // Scrollbar
    let total = state.wrapped.len();
    let has_scrollbar = !state.slide_mode && total > viewport && viewport > 0;
    let (thumb_start, thumb_end) = if has_scrollbar {
        let thumb_size = (viewport * viewport / total).max(1).min(viewport);
        let max_off = total.saturating_sub(viewport);
        let track_range = viewport.saturating_sub(thumb_size);
        let pos = if max_off > 0 && track_range > 0 {
            state.offset * track_range / max_off
        } else {
            0
        };
        (pos, (pos + thumb_size).min(viewport))
    } else {
        (0, 0)
    };

    // Title
    let file_label = if state.files.len() > 1 {
        format!(
            " {} [{}/{}] ",
            state.filename,
            state.current_file_idx + 1,
            state.files.len()
        )
    } else {
        format!(" {} ", state.filename)
    };
    let file_label_len = file_label.chars().count();
    let top_fill = width.saturating_sub(3 + file_label_len);

    queue!(
        stdout,
        MoveTo(0, 0),
        SetBackgroundColor(theme.bg),
        SetForegroundColor(theme.border),
        Print("╭─"),
        SetForegroundColor(theme.title),
        Print(&file_label),
        SetForegroundColor(theme.border),
        Print(format!("{}╮", "─".repeat(top_fill))),
        SetAttribute(Attribute::Reset),
    )?;

    // Content
    for row in 0..viewport {
        queue!(stdout, MoveTo(0, (row + 1) as u16))?;
        queue!(
            stdout,
            SetBackgroundColor(theme.bg),
            SetForegroundColor(theme.border),
            Print("│ "),
            SetAttribute(Attribute::Reset),
            SetBackgroundColor(theme.bg),
        )?;

        let line_idx = if state.slide_mode {
            let start = state
                .slide_boundaries
                .get(state.current_slide)
                .copied()
                .unwrap_or(0);
            start + row
        } else {
            state.offset + row
        };

        if let Some(line) = state.wrapped.get(line_idx) {
            let highlights = if !state.slide_mode {
                state.search.highlights_for_line(line_idx)
            } else {
                vec![]
            };
            let highlighted;
            let spans: &[StyledSpan] = if highlights.is_empty() {
                &line.spans
            } else {
                highlighted = apply_search_highlights(&line.spans, &highlights, theme);
                &highlighted
            };

            let mut col = 0;
            for span in spans {
                write_span(stdout, span, Some(theme.bg))?;
                col += span.text.chars().count();
            }
            if col < content_width {
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
                        SetAttribute(Attribute::Reset),
                        SetBackgroundColor(theme.bg),
                    )?;
                } else {
                    queue!(stdout, Print(" ".repeat(content_width - col)))?;
                }
            }
        } else {
            queue!(stdout, Print(" ".repeat(content_width)))?;
        }

        // Scrollbar / right border
        if has_scrollbar && row >= thumb_start && row < thumb_end {
            queue!(
                stdout,
                SetForegroundColor(theme.scrollbar_thumb),
                Print(" ┃"),
                SetAttribute(Attribute::Reset),
            )?;
        } else {
            let bar_color = if has_scrollbar {
                theme.scrollbar_track
            } else {
                theme.border
            };
            queue!(
                stdout,
                SetForegroundColor(bar_color),
                Print(" │"),
                SetAttribute(Attribute::Reset),
            )?;
        }
    }

    // Status bar
    render_status_bar(stdout, state)?;

    // Overlays (rendered on top)
    match state.mode {
        ViewMode::Toc => render_toc_overlay(stdout, state)?,
        ViewMode::LinkPicker => render_link_picker_overlay(stdout, state)?,
        ViewMode::FuzzyHeading => render_fuzzy_overlay(stdout, state)?,
        _ => {}
    }

    stdout.flush()
}

fn render_status_bar(stdout: &mut io::Stdout, state: &ViewerState) -> io::Result<()> {
    let width = state.cols as usize;
    let viewport = state.viewport();
    let theme = &state.theme;

    if state.slide_mode {
        let num_slides = state.slide_boundaries.len().max(1);
        let slide_label = format!(" Slide {}/{} ", state.current_slide + 1, num_slides);
        let slide_len = slide_label.chars().count();
        let hint = " ←/→ navigate · t theme ";
        let hint_len = hint.chars().count();
        let needed = 4 + slide_len + hint_len;
        let (show_hint, fill) = if width > needed {
            (true, width - needed)
        } else {
            (false, width.saturating_sub(4 + slide_len))
        };

        queue!(
            stdout,
            MoveTo(0, (viewport + 1) as u16),
            SetBackgroundColor(theme.bg),
            SetForegroundColor(theme.border),
            Print("╰─"),
        )?;
        if show_hint {
            queue!(stdout, SetForegroundColor(theme.help_hint), Print(hint))?;
        }
        queue!(
            stdout,
            SetForegroundColor(theme.border),
            Print("─".repeat(fill)),
            SetForegroundColor(theme.slide_indicator),
            Print(&slide_label),
            SetForegroundColor(theme.border),
            Print("─╯"),
            SetAttribute(Attribute::Reset),
        )?;
        return Ok(());
    }

    if state.mode == ViewMode::Search {
        let search_label = format!(" /{}█ ", state.search.input_buf);
        let search_label_len = search_label.chars().count();
        let fill = width.saturating_sub(3 + search_label_len);
        queue!(
            stdout,
            MoveTo(0, (viewport + 1) as u16),
            SetBackgroundColor(theme.bg),
            SetForegroundColor(theme.border),
            Print("╰─"),
            SetForegroundColor(theme.search_prompt),
            Print(&search_label),
            SetForegroundColor(theme.border),
            Print("─".repeat(fill)),
            Print("╯"),
            SetAttribute(Attribute::Reset),
        )?;
        return Ok(());
    }

    if let Some(ref msg) = state.status_msg {
        let msg_label = format!(" {} ", msg);
        let msg_len = msg_label.chars().count();
        let fill = width.saturating_sub(3 + msg_len);
        queue!(
            stdout,
            MoveTo(0, (viewport + 1) as u16),
            SetBackgroundColor(theme.bg),
            SetForegroundColor(theme.border),
            Print("╰─"),
            SetForegroundColor(theme.search_prompt),
            Print(&msg_label),
            SetForegroundColor(theme.border),
            Print("─".repeat(fill)),
            Print("╯"),
            SetAttribute(Attribute::Reset),
        )?;
        return Ok(());
    }

    if state.search.has_results() {
        let position = format_position(&state.wrapped, state.offset, viewport);
        let pos_label = format!(" {} ", position);
        let pos_label_len = pos_label.chars().count();

        let search_info = if state.search.matches.is_empty() {
            " no match ".to_string()
        } else {
            format!(
                " {}/{} ",
                state.search.current_idx + 1,
                state.search.matches.len()
            )
        };
        let search_info_len = search_info.chars().count();
        let search_info_fg = if state.search.matches.is_empty() {
            theme.search_no_match
        } else {
            theme.search_prompt
        };
        let fill = width.saturating_sub(4 + search_info_len + pos_label_len);

        queue!(
            stdout,
            MoveTo(0, (viewport + 1) as u16),
            SetBackgroundColor(theme.bg),
            SetForegroundColor(theme.border),
            Print("╰─"),
            SetForegroundColor(search_info_fg),
            Print(&search_info),
            SetForegroundColor(theme.border),
            Print("─".repeat(fill)),
            SetForegroundColor(theme.position),
            Print(&pos_label),
            SetForegroundColor(theme.border),
            Print("─╯"),
            SetAttribute(Attribute::Reset),
        )?;
        return Ok(());
    }

    // Normal position bar
    let position = format_position(&state.wrapped, state.offset, viewport);
    let pos_label = format!(" {} ", position);
    let pos_len = pos_label.chars().count();

    let hint = " / search · o toc · f links · t theme ";
    let hint_len = hint.chars().count();
    let needed = 4 + hint_len + pos_len;
    let (show_hint, fill) = if width > needed {
        (true, width - needed)
    } else {
        (false, width.saturating_sub(4 + pos_len))
    };

    queue!(
        stdout,
        MoveTo(0, (viewport + 1) as u16),
        SetBackgroundColor(theme.bg),
        SetForegroundColor(theme.border),
        Print("╰─"),
    )?;
    if show_hint {
        queue!(stdout, SetForegroundColor(theme.help_hint), Print(hint))?;
    }
    queue!(
        stdout,
        SetForegroundColor(theme.border),
        Print("─".repeat(fill)),
        SetForegroundColor(theme.position),
        Print(&pos_label),
        SetForegroundColor(theme.border),
        Print("─╯"),
        SetAttribute(Attribute::Reset),
    )
}

// ── Overlay rendering ───────────────────────────────────────────────────────

fn render_toc_overlay(stdout: &mut io::Stdout, state: &ViewerState) -> io::Result<()> {
    let theme = &state.theme;
    let entries = &state.toc_entries;
    let width = state.cols as usize;
    let viewport = state.viewport();

    let box_w = (width * 2 / 3).max(30).min(width.saturating_sub(6));
    let box_h = (entries.len() + 2).min(viewport.saturating_sub(4).max(3));
    let visible_entries = box_h.saturating_sub(2).max(1);
    let x_off = (width.saturating_sub(box_w)) / 2;
    let y_off = (viewport.saturating_sub(box_h)) / 2 + 1;

    let scroll = state.toc_scroll;

    // Title with count
    let title = format!(
        " Table of Contents ({}/{}) ",
        state.toc_selected + 1,
        entries.len()
    );
    let title_len = title.chars().count();
    let top_dashes = box_w.saturating_sub(3 + title_len);

    queue!(
        stdout,
        MoveTo(x_off as u16, y_off as u16),
        SetBackgroundColor(theme.overlay_bg),
        SetForegroundColor(theme.overlay_border),
        Print("╭─"),
        SetForegroundColor(theme.overlay_text),
        Print(&title),
        SetForegroundColor(theme.overlay_border),
        Print(format!("{}╮", "─".repeat(top_dashes))),
    )?;

    for i in 0..visible_entries {
        let entry_idx = scroll + i;
        queue!(
            stdout,
            MoveTo(x_off as u16, (y_off + 1 + i) as u16),
            SetBackgroundColor(theme.overlay_bg),
            SetForegroundColor(theme.overlay_border),
            Print("│"),
        )?;

        if let Some(entry) = entries.get(entry_idx) {
            let is_selected = entry_idx == state.toc_selected;
            let level_tag = format!("H{}", entry.level);
            let indent = ((entry.level as usize).saturating_sub(1)) * 2;
            let prefix = " ".repeat(indent + 1);
            let marker = if is_selected { "▸ " } else { "  " };
            let text = &entry.text;
            // Account for level tag: " H1 " = 4 chars
            let tag_len = level_tag.len() + 2; // space + tag + space
            let available = box_w.saturating_sub(3 + indent + 2 + tag_len);
            let display: String = if text.chars().count() > available {
                text.chars()
                    .take(available.saturating_sub(1))
                    .collect::<String>()
                    + "…"
            } else {
                text.clone()
            };
            let content_len =
                prefix.chars().count() + marker.chars().count() + display.chars().count() + tag_len;
            let padding = box_w.saturating_sub(2).saturating_sub(content_len);

            if is_selected {
                queue!(
                    stdout,
                    SetBackgroundColor(theme.overlay_selected_bg),
                    SetForegroundColor(theme.overlay_selected_fg),
                )?;
            } else {
                queue!(
                    stdout,
                    SetBackgroundColor(theme.overlay_bg),
                    SetForegroundColor(theme.overlay_text),
                )?;
            }
            queue!(
                stdout,
                Print(&prefix),
                Print(marker),
                Print(&display),
                Print(" ".repeat(padding)),
            )?;
            // Level tag (muted)
            if is_selected {
                queue!(
                    stdout,
                    SetForegroundColor(theme.overlay_muted),
                    Print(format!(" {} ", level_tag)),
                    SetBackgroundColor(theme.overlay_bg),
                    SetForegroundColor(theme.overlay_border),
                    Print("│"),
                )?;
            } else {
                queue!(
                    stdout,
                    SetForegroundColor(theme.overlay_muted),
                    Print(format!(" {} ", level_tag)),
                    SetForegroundColor(theme.overlay_border),
                    Print("│"),
                )?;
            }
        } else {
            queue!(
                stdout,
                SetBackgroundColor(theme.overlay_bg),
                Print(" ".repeat(box_w.saturating_sub(2))),
                SetForegroundColor(theme.overlay_border),
                Print("│"),
            )?;
        }
    }

    // Scroll indicators
    let has_above = scroll > 0;
    let has_below = scroll + visible_entries < entries.len();
    let scroll_hint = match (has_above, has_below) {
        (true, true) => " ▲▼ ",
        (true, false) => " ▲ ",
        (false, true) => " ▼ ",
        (false, false) => "",
    };

    let footer = " j/k ↑↓ navigate · Enter jump · Esc close ";
    let footer_len = footer.chars().count() + scroll_hint.chars().count();
    let bot_dashes = box_w.saturating_sub(3 + footer_len);

    queue!(
        stdout,
        MoveTo(x_off as u16, (y_off + 1 + visible_entries) as u16),
        SetBackgroundColor(theme.overlay_bg),
        SetForegroundColor(theme.overlay_border),
        Print("╰─"),
        SetForegroundColor(theme.overlay_muted),
        Print(footer),
    )?;
    if !scroll_hint.is_empty() {
        queue!(
            stdout,
            SetForegroundColor(theme.overlay_text),
            Print(scroll_hint),
        )?;
    }
    queue!(
        stdout,
        SetForegroundColor(theme.overlay_border),
        Print(format!("{}╯", "─".repeat(bot_dashes))),
        SetAttribute(Attribute::Reset),
    )?;

    Ok(())
}

fn render_link_picker_overlay(stdout: &mut io::Stdout, state: &ViewerState) -> io::Result<()> {
    let theme = &state.theme;
    let entries = &state.link_entries;
    let width = state.cols as usize;
    let viewport = state.viewport();

    let box_w = (width * 2 / 3).max(30).min(width.saturating_sub(6));
    let max_entries = viewport.saturating_sub(6);
    let shown = entries.len().min(max_entries);
    let box_h = shown + 3; // title + entries + input + bottom
    let x_off = (width.saturating_sub(box_w)) / 2;
    let y_off = (viewport.saturating_sub(box_h)) / 2 + 1;

    let title = " Links ";
    let title_len = title.chars().count();
    let top_dashes = box_w.saturating_sub(3 + title_len);

    queue!(
        stdout,
        MoveTo(x_off as u16, y_off as u16),
        SetBackgroundColor(theme.overlay_bg),
        SetForegroundColor(theme.overlay_border),
        Print("╭─"),
        SetForegroundColor(theme.overlay_text),
        Print(title),
        SetForegroundColor(theme.overlay_border),
        Print(format!("{}╮", "─".repeat(top_dashes))),
    )?;

    for (i, entry) in entries.iter().enumerate().take(shown) {
        let num = format!(" {:>2}. ", i + 1);
        let available = box_w.saturating_sub(2 + num.chars().count());
        let url_display: String = if entry.url.chars().count() > available {
            entry
                .url
                .chars()
                .take(available.saturating_sub(1))
                .collect::<String>()
                + "…"
        } else {
            entry.url.clone()
        };
        let padding = box_w.saturating_sub(2 + num.chars().count() + url_display.chars().count());

        queue!(
            stdout,
            MoveTo(x_off as u16, (y_off + 1 + i) as u16),
            SetBackgroundColor(theme.overlay_bg),
            SetForegroundColor(theme.overlay_border),
            Print("│"),
            SetForegroundColor(theme.overlay_selected_fg),
            Print(&num),
            SetForegroundColor(theme.overlay_text),
            Print(&url_display),
            Print(" ".repeat(padding)),
            SetForegroundColor(theme.overlay_border),
            Print("│"),
        )?;
    }

    // Input row
    let input_display = format!(" #{} █ ", state.link_input);
    let input_padding = box_w.saturating_sub(2 + input_display.chars().count());
    queue!(
        stdout,
        MoveTo(x_off as u16, (y_off + 1 + shown) as u16),
        SetBackgroundColor(theme.overlay_bg),
        SetForegroundColor(theme.overlay_border),
        Print("│"),
        SetForegroundColor(theme.search_prompt),
        Print(&input_display),
        SetBackgroundColor(theme.overlay_bg),
        Print(" ".repeat(input_padding)),
        SetForegroundColor(theme.overlay_border),
        Print("│"),
    )?;

    let footer = " type number · Enter open · Esc close ";
    let footer_len = footer.chars().count();
    let bot_dashes = box_w.saturating_sub(3 + footer_len);

    queue!(
        stdout,
        MoveTo(x_off as u16, (y_off + 2 + shown) as u16),
        SetBackgroundColor(theme.overlay_bg),
        SetForegroundColor(theme.overlay_border),
        Print("╰─"),
        SetForegroundColor(theme.overlay_muted),
        Print(footer),
        SetForegroundColor(theme.overlay_border),
        Print(format!("{}╯", "─".repeat(bot_dashes))),
        SetAttribute(Attribute::Reset),
    )?;

    Ok(())
}

fn render_fuzzy_overlay(stdout: &mut io::Stdout, state: &ViewerState) -> io::Result<()> {
    let theme = &state.theme;
    let width = state.cols as usize;
    let viewport = state.viewport();

    let filtered = fuzzy_filter(&state.toc_entries, &state.fuzzy_input);
    let total = filtered.len();

    let box_w = (width * 2 / 3).max(30).min(width.saturating_sub(6));
    let max_entries = viewport.saturating_sub(6).max(1);
    // Show at least 1 row for "no results" message
    let visible = if total == 0 {
        1
    } else {
        total.min(max_entries)
    };
    let box_h = visible + 3; // input row + entries + bottom
    let x_off = (width.saturating_sub(box_w)) / 2;
    let y_off = (viewport.saturating_sub(box_h)) / 2 + 1;

    let scroll = state.fuzzy_scroll;

    // Input row with match count
    let count_label = if state.fuzzy_input.is_empty() {
        format!("{} headings", total)
    } else if total == 0 {
        "no match".to_string()
    } else {
        format!("{}/{} ", state.fuzzy_selected + 1, total)
    };
    let input_display = format!(" > {}█ ", state.fuzzy_input);
    // Truncate input display if it would overflow the box
    let input_display = if input_display.chars().count() > box_w.saturating_sub(6) {
        let max_input_len = box_w.saturating_sub(10); // leave room for borders + count
        let suffix: String = state
            .fuzzy_input
            .chars()
            .rev()
            .take(max_input_len.saturating_sub(5))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!(" > …{}█ ", suffix)
    } else {
        input_display
    };
    let input_len = input_display.chars().count();
    let count_len = count_label.chars().count();
    let top_dashes = box_w.saturating_sub(3 + input_len + count_len + 1);

    queue!(
        stdout,
        MoveTo(x_off as u16, y_off as u16),
        SetBackgroundColor(theme.overlay_bg),
        SetForegroundColor(theme.overlay_border),
        Print("╭─"),
        SetForegroundColor(theme.search_prompt),
        Print(&input_display),
        SetForegroundColor(theme.overlay_border),
        Print("─".repeat(top_dashes)),
        SetForegroundColor(theme.overlay_muted),
        Print(format!(" {}", count_label)),
        SetForegroundColor(theme.overlay_border),
        Print("╮"),
    )?;

    if total == 0 {
        // "No results" row
        let msg = "  No matching headings";
        let msg_len = msg.chars().count();
        let padding = box_w.saturating_sub(2 + msg_len);
        queue!(
            stdout,
            MoveTo(x_off as u16, (y_off + 1) as u16),
            SetBackgroundColor(theme.overlay_bg),
            SetForegroundColor(theme.overlay_border),
            Print("│"),
            SetForegroundColor(theme.overlay_muted),
            Print(msg),
            Print(" ".repeat(padding)),
            SetForegroundColor(theme.overlay_border),
            Print("│"),
        )?;
    } else {
        for i in 0..visible {
            let entry_idx = scroll + i;
            queue!(
                stdout,
                MoveTo(x_off as u16, (y_off + 1 + i) as u16),
                SetBackgroundColor(theme.overlay_bg),
                SetForegroundColor(theme.overlay_border),
                Print("│"),
            )?;

            if let Some(entry) = filtered.get(entry_idx) {
                let is_selected = entry_idx == state.fuzzy_selected;
                let level_tag = format!("H{}", entry.level);
                let indent = ((entry.level as usize).saturating_sub(1)) * 2;
                let prefix = " ".repeat(indent + 1);
                let marker = if is_selected { "▸ " } else { "  " };
                let tag_len = level_tag.len() + 2;
                let available = box_w.saturating_sub(3 + indent + 2 + tag_len);
                let display: String = if entry.text.chars().count() > available {
                    entry
                        .text
                        .chars()
                        .take(available.saturating_sub(1))
                        .collect::<String>()
                        + "…"
                } else {
                    entry.text.clone()
                };
                let content_len = prefix.chars().count()
                    + marker.chars().count()
                    + display.chars().count()
                    + tag_len;
                let padding = box_w.saturating_sub(2).saturating_sub(content_len);

                if is_selected {
                    queue!(
                        stdout,
                        SetBackgroundColor(theme.overlay_selected_bg),
                        SetForegroundColor(theme.overlay_selected_fg),
                    )?;
                } else {
                    queue!(
                        stdout,
                        SetBackgroundColor(theme.overlay_bg),
                        SetForegroundColor(theme.overlay_text),
                    )?;
                }

                queue!(
                    stdout,
                    Print(&prefix),
                    Print(marker),
                    Print(&display),
                    Print(" ".repeat(padding)),
                )?;
                // Level tag
                if is_selected {
                    queue!(
                        stdout,
                        SetForegroundColor(theme.overlay_muted),
                        Print(format!(" {} ", level_tag)),
                        SetBackgroundColor(theme.overlay_bg),
                        SetForegroundColor(theme.overlay_border),
                        Print("│"),
                    )?;
                } else {
                    queue!(
                        stdout,
                        SetForegroundColor(theme.overlay_muted),
                        Print(format!(" {} ", level_tag)),
                        SetForegroundColor(theme.overlay_border),
                        Print("│"),
                    )?;
                }
            } else {
                queue!(
                    stdout,
                    SetBackgroundColor(theme.overlay_bg),
                    Print(" ".repeat(box_w.saturating_sub(2))),
                    SetForegroundColor(theme.overlay_border),
                    Print("│"),
                )?;
            }
        }
    }

    // Scroll indicators
    let has_above = scroll > 0;
    let has_below = total > 0 && scroll + visible < total;
    let scroll_hint = match (has_above, has_below) {
        (true, true) => " ▲▼ ",
        (true, false) => " ▲ ",
        (false, true) => " ▼ ",
        (false, false) => "",
    };

    let footer = " type to filter · ↑↓ select · Enter jump · Esc ";
    let footer_len = footer.chars().count() + scroll_hint.chars().count();
    let bot_dashes = box_w.saturating_sub(3 + footer_len);

    queue!(
        stdout,
        MoveTo(x_off as u16, (y_off + 1 + visible) as u16),
        SetBackgroundColor(theme.overlay_bg),
        SetForegroundColor(theme.overlay_border),
        Print("╰─"),
        SetForegroundColor(theme.overlay_muted),
        Print(footer),
    )?;
    if !scroll_hint.is_empty() {
        queue!(
            stdout,
            SetForegroundColor(theme.overlay_text),
            Print(scroll_hint),
        )?;
    }
    queue!(
        stdout,
        SetForegroundColor(theme.overlay_border),
        Print(format!("{}╯", "─".repeat(bot_dashes))),
        SetAttribute(Attribute::Reset),
    )?;

    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn format_position(lines: &[Line], offset: usize, viewport: usize) -> String {
    if lines.len() <= viewport {
        "All".to_string()
    } else if offset == 0 {
        "Top".to_string()
    } else if offset >= lines.len().saturating_sub(viewport) {
        "Bot".to_string()
    } else {
        let pct = (offset + viewport) * 100 / lines.len();
        format!("{}%", pct)
    }
}

fn apply_search_highlights(
    spans: &[StyledSpan],
    highlights: &[(usize, usize, bool)],
    theme: &Theme,
) -> Vec<StyledSpan> {
    let match_bg = theme.search_match_bg;
    let current_bg = theme.search_current_bg;
    let current_fg = theme.search_current_fg;

    let mut result = Vec::new();
    let mut char_offset = 0;

    for span in spans {
        let chars: Vec<char> = span.text.chars().collect();
        let span_len = chars.len();
        let span_start = char_offset;
        let span_end = char_offset + span_len;

        let mut cuts = vec![0usize, span_len];
        for &(hs, he, _) in highlights {
            if hs > span_start && hs < span_end {
                cuts.push(hs - span_start);
            }
            if he > span_start && he < span_end {
                cuts.push(he - span_start);
            }
        }
        cuts.sort();
        cuts.dedup();

        for pair in cuts.windows(2) {
            let (local_start, local_end) = (pair[0], pair[1]);
            if local_start >= local_end {
                continue;
            }

            let text: String = chars[local_start..local_end].iter().collect();
            let abs_pos = span_start + local_start;

            let highlight = highlights
                .iter()
                .find(|(hs, he, _)| abs_pos >= *hs && abs_pos < *he);

            let mut style = span.style.clone();
            if let Some(&(_, _, is_current)) = highlight {
                if is_current {
                    style.bg = Some(current_bg);
                    style.fg = Some(current_fg);
                    style.bold = true;
                } else {
                    style.bg = Some(match_bg);
                }
            }

            result.push(StyledSpan { text, style });
        }

        char_offset = span_end;
    }

    result
}

fn write_span(
    stdout: &mut io::Stdout,
    span: &StyledSpan,
    restore_bg: Option<Color>,
) -> io::Result<()> {
    let s = &span.style;

    // OSC 8 hyperlink start
    if let Some(ref url) = s.link_url {
        queue!(stdout, Print(format!("\x1b]8;;{}\x1b\\", url)))?;
    }

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

    // OSC 8 hyperlink end
    if s.link_url.is_some() {
        queue!(stdout, Print("\x1b]8;;\x1b\\"))?;
    }

    if let Some(bg) = restore_bg {
        queue!(stdout, SetBackgroundColor(bg))?;
    }
    Ok(())
}
