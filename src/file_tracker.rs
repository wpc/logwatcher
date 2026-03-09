use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// State for a single tracked file
pub struct TrackedFile {
    pub path: PathBuf,
    pub display_name: String,
    pub lines: Vec<String>,
    pub last_modified: Instant,
    pub last_size: u64,
    pub is_deleted: bool,
}

const MAX_LINES_PER_FILE: usize = 500;

/// Manages which files occupy which panel slots
pub struct FileTracker {
    pub panels: Vec<Option<TrackedFile>>,
    path_to_panel: HashMap<PathBuf, usize>,
    watch_root: PathBuf,
}

impl FileTracker {
    pub fn new(max_panels: usize, watch_root: PathBuf) -> Self {
        let mut panels = Vec::with_capacity(max_panels);
        for _ in 0..max_panels {
            panels.push(None);
        }
        Self {
            panels,
            path_to_panel: HashMap::new(),
            watch_root,
        }
    }

    /// Look up the panel index for a given path.
    pub fn panel_index(&self, path: &Path) -> Option<usize> {
        self.path_to_panel.get(path).copied()
    }

    /// Called when a file is created or modified with initial content.
    /// Returns the panel index that was assigned.
    pub fn file_modified(&mut self, path: PathBuf, lines: Vec<String>, file_size: u64) -> usize {
        // If already tracked, update in place
        if let Some(&idx) = self.path_to_panel.get(&path) {
            if let Some(ref mut tracked) = self.panels[idx] {
                tracked.lines = lines;
                tracked.last_modified = Instant::now();
                tracked.last_size = file_size;
                tracked.is_deleted = false;
            }
            return idx;
        }

        let display_name = self.make_display_name(&path);

        // Find an empty slot
        if let Some(idx) = self.panels.iter().position(|p| p.is_none()) {
            self.panels[idx] = Some(TrackedFile {
                path: path.clone(),
                display_name,
                lines,
                last_modified: Instant::now(),
                last_size: file_size,
                is_deleted: false,
            });
            self.path_to_panel.insert(path, idx);
            return idx;
        }

        // All slots full — evict
        let evict_idx = self.eviction_candidate();
        if let Some(ref old) = self.panels[evict_idx] {
            self.path_to_panel.remove(&old.path.clone());
        }
        self.panels[evict_idx] = Some(TrackedFile {
            path: path.clone(),
            display_name,
            lines,
            last_modified: Instant::now(),
            last_size: file_size,
            is_deleted: false,
        });
        self.path_to_panel.insert(path, evict_idx);
        evict_idx
    }

    /// Append new lines to an already-tracked panel.
    pub fn append_lines(&mut self, panel_idx: usize, new_lines: Vec<String>, new_size: u64) {
        if let Some(ref mut tracked) = self.panels[panel_idx] {
            tracked.lines.extend(new_lines);
            // Cap the lines buffer
            if tracked.lines.len() > MAX_LINES_PER_FILE {
                let drain_count = tracked.lines.len() - MAX_LINES_PER_FILE;
                tracked.lines.drain(..drain_count);
            }
            tracked.last_modified = Instant::now();
            tracked.last_size = new_size;
        }
    }

    /// Called when a file is deleted.
    pub fn file_deleted(&mut self, path: &Path) {
        if let Some(&idx) = self.path_to_panel.get(path) {
            if let Some(ref mut tracked) = self.panels[idx] {
                tracked.is_deleted = true;
            }
        }
    }

    /// Remove panels for deleted files that have been stale longer than the timeout.
    pub fn gc_stale(&mut self, timeout: std::time::Duration) {
        let now = Instant::now();
        for i in 0..self.panels.len() {
            let should_clear = if let Some(ref tracked) = self.panels[i] {
                tracked.is_deleted && now.duration_since(tracked.last_modified) > timeout
            } else {
                false
            };
            if should_clear {
                if let Some(ref tracked) = self.panels[i] {
                    self.path_to_panel.remove(&tracked.path);
                }
                self.panels[i] = None;
            }
        }
    }

    /// Get the panel slot that is least recently modified (candidate for eviction).
    fn eviction_candidate(&self) -> usize {
        let mut best_idx = 0;
        let mut best_time = Instant::now();
        let mut found_deleted = false;

        for (i, panel) in self.panels.iter().enumerate() {
            if let Some(ref tracked) = panel {
                // Prefer evicting deleted files
                if tracked.is_deleted && !found_deleted {
                    found_deleted = true;
                    best_idx = i;
                    best_time = tracked.last_modified;
                } else if tracked.is_deleted == found_deleted && tracked.last_modified < best_time {
                    best_idx = i;
                    best_time = tracked.last_modified;
                }
            } else {
                // Empty slot — shouldn't happen since we check first, but just in case
                return i;
            }
        }

        best_idx
    }

    fn make_display_name(&self, path: &Path) -> String {
        path.strip_prefix(&self.watch_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string()
    }
}
