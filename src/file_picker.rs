use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub display: String,
}

#[derive(Clone, Debug)]
struct MatchEntry {
    entry_idx: usize,
    score: i64,
}

#[derive(Debug)]
pub struct FilePickerState {
    pub root: PathBuf,
    pub root_label: String,
    pub input: String,
    pub selected: usize,
    pub scroll: usize,
    entries: Vec<FileEntry>,
    matches: Vec<MatchEntry>,
    pub error: Option<String>,
}

impl FilePickerState {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let root_label = display_path(&root);
        let mut state = Self {
            root,
            root_label,
            input: String::new(),
            selected: 0,
            scroll: 0,
            entries: Vec::new(),
            matches: Vec::new(),
            error: None,
        };
        state.refresh();
        state
    }

    pub fn refresh(&mut self) {
        match discover_markdown_files(&self.root) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(e) => {
                self.entries.clear();
                self.error = Some(e.to_string());
            }
        }
        self.update_matches();
    }

    pub fn push(&mut self, c: char) {
        self.input.push(c);
        self.selected = 0;
        self.scroll = 0;
        self.update_matches();
    }

    pub fn backspace(&mut self) {
        self.input.pop();
        self.selected = 0;
        self.scroll = 0;
        self.update_matches();
    }

    pub fn clear(&mut self) {
        self.input.clear();
        self.selected = 0;
        self.scroll = 0;
        self.update_matches();
    }

    pub fn move_up(&mut self, step: usize, visible: usize) {
        self.selected = self.selected.saturating_sub(step);
        self.keep_selection_visible(visible);
    }

    pub fn move_down(&mut self, step: usize, visible: usize) {
        let count = self.match_count();
        if count == 0 {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = (self.selected + step).min(count - 1);
        self.keep_selection_visible(visible);
    }

    pub fn move_home(&mut self) {
        self.selected = 0;
        self.scroll = 0;
    }

    pub fn move_end(&mut self, visible: usize) {
        let count = self.match_count();
        if count == 0 {
            self.selected = 0;
            self.scroll = 0;
        } else {
            self.selected = count - 1;
            self.keep_selection_visible(visible);
        }
    }

    pub fn keep_selection_visible(&mut self, visible: usize) {
        let visible = visible.max(1);
        if self.selected >= self.scroll + visible {
            self.scroll = self.selected - visible + 1;
        } else if self.selected < self.scroll {
            self.scroll = self.selected;
        }
    }

    pub fn select_path(&mut self, path: &Path) -> bool {
        let wanted = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let Some(entry_idx) = self.entries.iter().position(|entry| {
            entry
                .path
                .canonicalize()
                .unwrap_or_else(|_| entry.path.clone())
                == wanted
        }) else {
            return false;
        };
        let Some(match_idx) = self.matches.iter().position(|m| m.entry_idx == entry_idx) else {
            return false;
        };
        self.selected = match_idx;
        true
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    pub fn total_count(&self) -> usize {
        self.entries.len()
    }

    pub fn visible_entries(&self, visible: usize) -> Vec<&FileEntry> {
        self.matches
            .iter()
            .skip(self.scroll)
            .take(visible)
            .filter_map(|m| self.entries.get(m.entry_idx))
            .collect()
    }

    pub fn entry_at_match(&self, match_idx: usize) -> Option<&FileEntry> {
        self.matches
            .get(match_idx)
            .and_then(|m| self.entries.get(m.entry_idx))
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        self.entry_at_match(self.selected)
            .map(|entry| entry.path.clone())
    }

    fn update_matches(&mut self) {
        let query = self.input.trim();
        self.matches.clear();
        if query.is_empty() {
            self.matches = (0..self.entries.len())
                .map(|entry_idx| MatchEntry {
                    entry_idx,
                    score: 0,
                })
                .collect();
        } else {
            self.matches = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(entry_idx, entry)| {
                    fuzzy_score(&entry.display, query).map(|score| MatchEntry { entry_idx, score })
                })
                .collect();
        }

        self.matches.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| {
                    self.entries[a.entry_idx]
                        .display
                        .chars()
                        .count()
                        .cmp(&self.entries[b.entry_idx].display.chars().count())
                })
                .then_with(|| {
                    self.entries[a.entry_idx]
                        .display
                        .cmp(&self.entries[b.entry_idx].display)
                })
        });

        let count = self.matches.len();
        if count == 0 {
            self.selected = 0;
            self.scroll = 0;
        } else {
            self.selected = self.selected.min(count - 1);
            self.scroll = self.scroll.min(count - 1);
        }
    }
}

pub fn discover_markdown_files(root: &Path) -> std::io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    visit_dir(root, root, &mut entries)?;
    entries.sort_by(|a, b| cmp_display(&a.display, &b.display));
    Ok(entries)
}

pub fn fuzzy_score(text: &str, query: &str) -> Option<i64> {
    let text = normalize_for_match(text);
    let query = normalize_for_match(query);
    if query.is_empty() {
        return Some(0);
    }

    let text_chars: Vec<char> = text.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();
    if query_chars.len() > text_chars.len() {
        return None;
    }

    let mut best: Option<i64> = None;
    for start in 0..text_chars.len() {
        if text_chars[start] != query_chars[0] {
            continue;
        }
        let Some(positions) = greedy_positions_from(&text_chars, &query_chars, start) else {
            continue;
        };
        let score = score_positions(&text_chars, &query_chars, &positions);
        best = Some(best.map_or(score, |prev| prev.max(score)));
    }
    best
}

fn visit_dir(root: &Path, dir: &Path, entries: &mut Vec<FileEntry>) -> std::io::Result<()> {
    let mut children = Vec::new();
    for child in fs::read_dir(dir)? {
        let child = match child {
            Ok(child) => child,
            Err(_) => continue,
        };
        children.push(child);
    }
    children.sort_by(|a, b| {
        cmp_display(
            &a.file_name().to_string_lossy(),
            &b.file_name().to_string_lossy(),
        )
    });

    for child in children {
        let file_type = match child.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let path = child.path();
        if file_type.is_dir() {
            let _ = visit_dir(root, &path, entries);
        } else if file_type.is_file() && is_markdown_file(&path) {
            let display = path
                .strip_prefix(root)
                .map(display_path)
                .unwrap_or_else(|_| display_path(&path));
            entries.push(FileEntry { path, display });
        }
    }
    Ok(())
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

fn greedy_positions_from(text: &[char], query: &[char], start: usize) -> Option<Vec<usize>> {
    let mut positions = vec![start];
    let mut text_idx = start + 1;
    for &query_char in &query[1..] {
        let found = text[text_idx..]
            .iter()
            .position(|&text_char| text_char == query_char)?;
        let idx = text_idx + found;
        positions.push(idx);
        text_idx = idx + 1;
    }
    Some(positions)
}

fn score_positions(text: &[char], query: &[char], positions: &[usize]) -> i64 {
    let span = positions.last().unwrap() - positions[0] + 1;
    let gaps = span.saturating_sub(query.len());
    let contiguous_pairs = positions.windows(2).filter(|w| w[1] == w[0] + 1).count();
    let boundary_hits = positions
        .iter()
        .filter(|&&idx| idx == 0 || matches!(text[idx.saturating_sub(1)], '/' | '-' | '_' | ' '))
        .count();
    let text_string: String = text.iter().collect();
    let query_string: String = query.iter().collect();
    let contains_bonus = if text_string.contains(&query_string) {
        5000
    } else {
        0
    };
    let basename_bonus = text_string
        .rsplit_once('/')
        .filter(|(_, basename)| basename.contains(&query_string))
        .map(|_| 1500)
        .unwrap_or(0);

    20_000
        + contains_bonus
        + basename_bonus
        + (contiguous_pairs as i64 * 80)
        + (boundary_hits as i64 * 40)
        - (span as i64 * 35)
        - (gaps as i64 * 20)
        - (positions[0] as i64 * 3)
}

fn normalize_for_match(value: &str) -> String {
    value
        .replace('\\', "/")
        .chars()
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn display_path(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = path.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = path.strip_prefix("//?/") {
        rest.to_string()
    } else {
        path
    }
}

fn cmp_display(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn fuzzy_score_matches_across_path_segments() {
        assert!(fuzzy_score("hello/world/a.md", "hellrlda.md").is_some());
    }

    #[test]
    fn fuzzy_score_rejects_non_subsequence() {
        assert!(fuzzy_score("hello/world/a.md", "haz").is_none());
    }

    #[test]
    fn fuzzy_score_prefers_tighter_match() {
        let tight = fuzzy_score("hello/world/a.md", "world").unwrap();
        let loose = fuzzy_score("w/o/r/l/d.md", "world").unwrap();
        assert!(tight > loose);
    }

    #[test]
    fn discover_markdown_files_recurses_and_filters() {
        let root = temp_root("mdterm-picker-test");
        let nested = root.join("hello").join("world");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("a.md"), "# A").unwrap();
        fs::write(nested.join("b.txt"), "B").unwrap();
        fs::write(root.join("README.MD"), "# Readme").unwrap();

        let entries = discover_markdown_files(&root).unwrap();
        let displays: Vec<_> = entries.iter().map(|entry| entry.display.as_str()).collect();

        assert_eq!(displays, vec!["hello/world/a.md", "README.MD"]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn display_path_strips_windows_verbatim_prefix() {
        assert_eq!(
            display_path(Path::new(r"\\?\C:\Users\me\file.md")),
            "C:/Users/me/file.md"
        );
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\file.md")),
            "//server/share/file.md"
        );
    }

    fn temp_root(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}"))
    }
}
