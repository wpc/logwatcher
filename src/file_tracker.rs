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
    pub process_cmd: Option<String>,
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
                process_cmd: None,
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
            process_cmd: None,
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

/// Look up the full command line of the process that has a file open.
/// Uses `lsof -F p` to get the PID, then reads `/proc/<pid>/cmdline`.
/// Falls back to command name from `lsof -F c` if /proc is unavailable.
/// Truncates to 60 chars with "..." if too long.
pub fn lookup_process(path: &Path) -> Option<String> {
    let output = std::process::Command::new("lsof")
        .arg("-F")
        .arg("pc")
        .arg(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut pid: Option<&str> = None;
    let mut cmd_name: Option<String> = None;

    for line in stdout.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = Some(p);
        } else if let Some(c) = line.strip_prefix('c') {
            cmd_name = Some(c.to_string());
        }
        if pid.is_some() && cmd_name.is_some() {
            break;
        }
    }

    // Try /proc/<pid>/cmdline for full command line
    let full_cmd = pid.and_then(|p| {
        let cmdline_path = format!("/proc/{}/cmdline", p);
        std::fs::read(&cmdline_path).ok().map(|bytes| {
            let s = bytes.iter()
                .map(|&b| if b == 0 { b' ' } else { b })
                .collect::<Vec<u8>>();
            String::from_utf8_lossy(&s).trim().to_string()
        })
    });

    let result = full_cmd.or(cmd_name)?;
    if result.is_empty() {
        return None;
    }
    Some(truncate_cmd(&result, 60))
}

fn truncate_cmd(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker(n: usize) -> FileTracker {
        FileTracker::new(n, PathBuf::from("/tmp"))
    }

    #[test]
    fn assigns_to_empty_slots() {
        let mut t = tracker(3);
        let idx = t.file_modified(PathBuf::from("/tmp/a.txt"), vec!["hello".into()], 6);
        assert_eq!(idx, 0);
        assert!(t.panels[0].is_some());
        assert_eq!(t.panels[0].as_ref().unwrap().display_name, "a.txt");

        let idx2 = t.file_modified(PathBuf::from("/tmp/b.txt"), vec![], 0);
        assert_eq!(idx2, 1);
    }

    #[test]
    fn updates_existing_file_in_place() {
        let mut t = tracker(2);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec!["v1".into()], 3);
        let idx = t.file_modified(PathBuf::from("/tmp/a.txt"), vec!["v2".into()], 3);
        assert_eq!(idx, 0);
        assert_eq!(t.panels[0].as_ref().unwrap().lines, vec!["v2"]);
        // Should not have used a second slot
        assert!(t.panels[1].is_none());
    }

    #[test]
    fn evicts_oldest_when_full() {
        let mut t = tracker(2);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec!["a".into()], 2);
        // Make a.txt older by modifying b.txt after
        std::thread::sleep(std::time::Duration::from_millis(10));
        t.file_modified(PathBuf::from("/tmp/b.txt"), vec!["b".into()], 2);

        // Panel is full, c.txt should evict a.txt (oldest)
        std::thread::sleep(std::time::Duration::from_millis(10));
        let idx = t.file_modified(PathBuf::from("/tmp/c.txt"), vec!["c".into()], 2);
        assert_eq!(idx, 0); // a.txt was in slot 0
        assert_eq!(t.panels[0].as_ref().unwrap().display_name, "c.txt");
        assert!(t.panel_index(&PathBuf::from("/tmp/a.txt")).is_none());
    }

    #[test]
    fn evicts_deleted_files_first() {
        let mut t = tracker(2);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec!["a".into()], 2);
        std::thread::sleep(std::time::Duration::from_millis(10));
        t.file_modified(PathBuf::from("/tmp/b.txt"), vec!["b".into()], 2);

        // Mark b.txt as deleted — it should be evicted even though it's newer
        t.file_deleted(&PathBuf::from("/tmp/b.txt"));

        std::thread::sleep(std::time::Duration::from_millis(10));
        let idx = t.file_modified(PathBuf::from("/tmp/c.txt"), vec!["c".into()], 2);
        assert_eq!(idx, 1); // b.txt was in slot 1
        assert_eq!(t.panels[1].as_ref().unwrap().display_name, "c.txt");
    }

    #[test]
    fn panel_index_lookup() {
        let mut t = tracker(2);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec![], 0);
        assert_eq!(t.panel_index(&PathBuf::from("/tmp/a.txt")), Some(0));
        assert_eq!(t.panel_index(&PathBuf::from("/tmp/nope.txt")), None);
    }

    #[test]
    fn append_lines_caps_buffer() {
        let mut t = tracker(1);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec!["start".into()], 6);

        // Append more than MAX_LINES_PER_FILE
        let big: Vec<String> = (0..600).map(|i| format!("line {}", i)).collect();
        t.append_lines(0, big, 9999);

        let tracked = t.panels[0].as_ref().unwrap();
        assert_eq!(tracked.lines.len(), 500); // MAX_LINES_PER_FILE
        assert_eq!(tracked.lines.last().unwrap(), "line 599");
    }

    #[test]
    fn file_deleted_marks_flag() {
        let mut t = tracker(1);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec![], 0);
        assert!(!t.panels[0].as_ref().unwrap().is_deleted);

        t.file_deleted(&PathBuf::from("/tmp/a.txt"));
        assert!(t.panels[0].as_ref().unwrap().is_deleted);
    }

    #[test]
    fn gc_stale_clears_old_deleted() {
        let mut t = tracker(1);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec![], 0);
        t.file_deleted(&PathBuf::from("/tmp/a.txt"));

        // With zero timeout, should clear immediately
        t.gc_stale(std::time::Duration::ZERO);
        assert!(t.panels[0].is_none());
        assert!(t.panel_index(&PathBuf::from("/tmp/a.txt")).is_none());
    }

    #[test]
    fn gc_stale_keeps_non_deleted() {
        let mut t = tracker(1);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec![], 0);

        t.gc_stale(std::time::Duration::ZERO);
        assert!(t.panels[0].is_some()); // not deleted, should remain
    }

    #[test]
    fn display_name_strips_watch_root() {
        let mut t = FileTracker::new(1, PathBuf::from("/home/user/logs"));
        t.file_modified(PathBuf::from("/home/user/logs/sub/output.txt"), vec![], 0);
        assert_eq!(t.panels[0].as_ref().unwrap().display_name, "sub/output.txt");
    }
}
