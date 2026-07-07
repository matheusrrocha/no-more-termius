//! Pure pane logic for the dual-pane SFTP browser: entries, sorting,
//! fuzzy filtering, selection. No I/O except `read_local_dir`.

use std::path::{Path, PathBuf};

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ratatui::widgets::ListState;

pub const PARENT: &str = "..";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FsEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
}

impl FsEntry {
    pub fn is_parent(&self) -> bool {
        self.name == PARENT
    }

    pub fn is_hidden(&self) -> bool {
        !self.is_parent() && self.name.starts_with('.')
    }
}

pub struct PaneState {
    pub cwd: PathBuf,
    /// Full listing; `..` pinned at index 0 when cwd has a parent.
    pub entries: Vec<FsEntry>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub filter: String,
    pub loading: bool,
    pub list_state: ListState,
    matcher: SkimMatcherV2,
}

impl PaneState {
    pub fn new() -> Self {
        Self {
            cwd: PathBuf::from("/"),
            entries: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            filter: String::new(),
            loading: false,
            list_state: ListState::default(),
            matcher: SkimMatcherV2::default(),
        }
    }

    /// Install a fresh directory listing. `entries` need not be sorted.
    pub fn set_listing(&mut self, cwd: PathBuf, mut entries: Vec<FsEntry>, show_hidden: bool) {
        sort_entries(&mut entries);
        if let Some(parent) = cwd.parent() {
            entries.insert(
                0,
                FsEntry {
                    name: PARENT.into(),
                    path: parent.to_path_buf(),
                    is_dir: true,
                    is_symlink: false,
                    size: 0,
                },
            );
        }
        self.cwd = cwd;
        self.entries = entries;
        self.filter.clear();
        self.loading = false;
        self.apply_filter(show_hidden);
    }

    pub fn apply_filter(&mut self, show_hidden: bool) {
        if self.filter.is_empty() {
            self.filtered = (0..self.entries.len())
                .filter(|&i| show_hidden || !self.entries[i].is_hidden())
                .collect();
        } else {
            // `..` is excluded while filtering.
            let matcher = &self.matcher;
            let mut scored: Vec<(i64, usize)> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| !e.is_parent() && (show_hidden || !e.is_hidden()))
                .filter_map(|(i, e)| matcher.fuzzy_match(&e.name, &self.filter).map(|s| (s, i)))
                .collect();
            scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
            self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.selected = 0;
    }

    pub fn selected_entry(&self) -> Option<&FsEntry> {
        self.filtered.get(self.selected).map(|&i| &self.entries[i])
    }

    pub fn move_selection(&mut self, delta: i64) {
        if self.filtered.is_empty() {
            return;
        }
        let last = self.filtered.len() as i64 - 1;
        self.selected = (self.selected as i64 + delta).clamp(0, last) as usize;
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        self.selected = self.filtered.len().saturating_sub(1);
    }
}

pub fn sort_entries(entries: &mut [FsEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Valid file name for rename: non-empty, no path separators, not `.`/`..`.
pub fn is_valid_entry_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\0') && name != "." && name != ".."
}

/// Remote preview whitelist: small images, text-like files and PDFs.
pub fn remote_preview_supported(name: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        // images
        "png", "jpg", "jpeg", "gif", "bmp", "webp", "heic", "tiff", "svg", // text
        "txt", "md", "log", "json", "yaml", "yml", "toml", "conf", "cfg", "ini", "csv", "xml",
        "html", "css", "js", "ts", "sh", "py", "rb", "go", "rs", "sql", "env", // documents
        "pdf",
    ];
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Synchronous local listing — local dirs list instantly, no worker needed.
pub fn read_local_dir(dir: &Path) -> std::io::Result<Vec<FsEntry>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let is_symlink = entry
            .file_type()
            .map(|t| t.is_symlink())
            .unwrap_or(false);
        let meta = std::fs::metadata(&path); // follows symlinks
        let (is_dir, size) = match meta {
            Ok(m) => (m.is_dir(), m.len()),
            Err(_) => (false, 0), // broken symlink
        };
        out.push(FsEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path,
            is_dir,
            is_symlink,
            size,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> FsEntry {
        FsEntry {
            name: name.into(),
            path: PathBuf::from("/tmp").join(name),
            is_dir,
            is_symlink: false,
            size: 42,
        }
    }

    fn pane_with(names: &[(&str, bool)], show_hidden: bool) -> PaneState {
        let mut pane = PaneState::new();
        let entries = names.iter().map(|(n, d)| entry(n, *d)).collect();
        pane.set_listing(PathBuf::from("/tmp"), entries, show_hidden);
        pane
    }

    fn visible_names(pane: &PaneState) -> Vec<&str> {
        pane.filtered
            .iter()
            .map(|&i| pane.entries[i].name.as_str())
            .collect()
    }

    #[test]
    fn dirs_first_case_insensitive_with_parent_pinned() {
        let pane = pane_with(
            &[("zeta.txt", false), ("Alpha", true), ("beta", true), ("a.txt", false)],
            true,
        );
        assert_eq!(visible_names(&pane), vec!["..", "Alpha", "beta", "a.txt", "zeta.txt"]);
    }

    #[test]
    fn root_has_no_parent_entry() {
        let mut pane = PaneState::new();
        pane.set_listing(PathBuf::from("/"), vec![entry("etc", true)], true);
        assert_eq!(visible_names(&pane), vec!["etc"]);
    }

    #[test]
    fn hidden_toggle() {
        let mut pane = pane_with(&[(".secret", false), ("open.txt", false)], true);
        assert_eq!(visible_names(&pane), vec!["..", ".secret", "open.txt"]);
        pane.filter.clear();
        pane.apply_filter(false);
        assert_eq!(visible_names(&pane), vec!["..", "open.txt"]);
    }

    #[test]
    fn filter_excludes_parent_and_resets_selection() {
        let mut pane = pane_with(&[("deploy.log", false), ("readme.md", false)], true);
        pane.selected = 2;
        pane.filter = "dep".into();
        pane.apply_filter(true);
        assert_eq!(visible_names(&pane), vec!["deploy.log"]);
        assert_eq!(pane.selected, 0);
    }

    #[test]
    fn selection_clamps() {
        let mut pane = pane_with(&[("a", false), ("b", false)], true);
        pane.move_selection(-5);
        assert_eq!(pane.selected, 0);
        pane.move_selection(100);
        assert_eq!(pane.selected, 2); // .., a, b
    }

    #[test]
    fn remote_preview_whitelist() {
        assert!(remote_preview_supported("photo.PNG"));
        assert!(remote_preview_supported("notes.md"));
        assert!(remote_preview_supported("doc.pdf"));
        assert!(!remote_preview_supported("archive.tar.gz"));
        assert!(!remote_preview_supported("binary"));
        assert!(!remote_preview_supported("app.exe"));
    }

    #[test]
    fn entry_name_validation() {
        assert!(is_valid_entry_name("file.txt"));
        assert!(is_valid_entry_name(".hidden"));
        assert!(!is_valid_entry_name(""));
        assert!(!is_valid_entry_name("a/b"));
        assert!(!is_valid_entry_name("."));
        assert!(!is_valid_entry_name(".."));
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
