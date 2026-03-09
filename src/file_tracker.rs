use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

/// State for a single tracked file
pub struct TrackedFile {
    pub path: PathBuf,
    pub display_name: String,
    pub lines: Vec<String>,
    pub last_modified: Instant,
    pub last_size: u64,
    pub is_deleted: bool,
    pub process_cmd: Option<String>,
    pub file_mtime: SystemTime,
}

const MAX_LINES_PER_FILE: usize = 500;

/// Manages which files occupy which panel slots
pub struct FileTracker {
    pub panels: Vec<TrackedFile>,
    path_to_panel: HashMap<PathBuf, usize>,
    watch_root: PathBuf,
    max_panels: usize,
}

impl FileTracker {
    pub fn new(max_panels: usize, watch_root: PathBuf) -> Self {
        Self {
            panels: Vec::new(),
            path_to_panel: HashMap::new(),
            watch_root,
            max_panels,
        }
    }

    pub fn active_count(&self) -> usize {
        self.panels.len()
    }

    /// Look up the panel index for a given path.
    pub fn panel_index(&self, path: &Path) -> Option<usize> {
        self.path_to_panel.get(path).copied()
    }

    /// Called when a file is created or modified with initial content.
    /// Returns the panel index that was assigned.
    pub fn file_modified(&mut self, path: PathBuf, lines: Vec<String>, file_size: u64) -> usize {
        let mtime = file_mtime(&path);
        // If already tracked, update in place
        if let Some(&idx) = self.path_to_panel.get(&path) {
            let tracked = &mut self.panels[idx];
            tracked.lines = lines;
            tracked.last_modified = Instant::now();
            tracked.last_size = file_size;
            tracked.is_deleted = false;
            tracked.file_mtime = mtime;
            return idx;
        }

        let display_name = self.make_display_name(&path);
        let new_tracked = TrackedFile {
            path: path.clone(),
            display_name,
            lines,
            last_modified: Instant::now(),
            last_size: file_size,
            is_deleted: false,
            process_cmd: None,
            file_mtime: mtime,
        };

        // Room to grow — push a new panel
        if self.panels.len() < self.max_panels {
            let idx = self.panels.len();
            self.panels.push(new_tracked);
            self.path_to_panel.insert(path, idx);
            return idx;
        }

        // All slots full — evict
        let evict_idx = self.eviction_candidate();
        self.path_to_panel.remove(&self.panels[evict_idx].path.clone());
        self.panels[evict_idx] = new_tracked;
        self.path_to_panel.insert(path, evict_idx);
        evict_idx
    }

    /// Append new lines to an already-tracked panel.
    pub fn append_lines(&mut self, panel_idx: usize, new_lines: Vec<String>, new_size: u64) {
        let tracked = &mut self.panels[panel_idx];
        tracked.lines.extend(new_lines);
        // Cap the lines buffer
        if tracked.lines.len() > MAX_LINES_PER_FILE {
            let drain_count = tracked.lines.len() - MAX_LINES_PER_FILE;
            tracked.lines.drain(..drain_count);
        }
        tracked.last_modified = Instant::now();
        tracked.last_size = new_size;
        tracked.file_mtime = file_mtime(&tracked.path);
    }

    /// Called when a file is deleted.
    pub fn file_deleted(&mut self, path: &Path) {
        if let Some(&idx) = self.path_to_panel.get(path) {
            self.panels[idx].is_deleted = true;
        }
    }

    /// Remove panels for deleted files that have been stale longer than the timeout.
    /// Compacts the panels vec and updates path_to_panel indices.
    pub fn gc_stale(&mut self, timeout: std::time::Duration) {
        let now = Instant::now();
        let mut i = 0;
        while i < self.panels.len() {
            let should_remove = self.panels[i].is_deleted
                && now.duration_since(self.panels[i].last_modified) > timeout;
            if should_remove {
                self.path_to_panel.remove(&self.panels[i].path);
                self.panels.remove(i);
                // Shift down all indices >= i
                for val in self.path_to_panel.values_mut() {
                    if *val >= i {
                        *val = val.saturating_sub(1);
                    }
                }
                // Don't increment i — next element shifted into this position
            } else {
                i += 1;
            }
        }
    }

    /// Get the panel slot that is least recently modified (candidate for eviction).
    fn eviction_candidate(&self) -> usize {
        let mut best_idx = 0;
        let mut best_time = Instant::now();
        let mut found_deleted = false;

        for (i, tracked) in self.panels.iter().enumerate() {
            // Prefer evicting deleted files
            if tracked.is_deleted && !found_deleted {
                found_deleted = true;
                best_idx = i;
                best_time = tracked.last_modified;
            } else if tracked.is_deleted == found_deleted && tracked.last_modified < best_time {
                best_idx = i;
                best_time = tracked.last_modified;
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

/// Read the file's mtime from the filesystem. Falls back to now if unavailable.
fn file_mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| SystemTime::now())
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
        assert_eq!(t.active_count(), 1);
        assert_eq!(t.panels[0].display_name, "a.txt");

        let idx2 = t.file_modified(PathBuf::from("/tmp/b.txt"), vec![], 0);
        assert_eq!(idx2, 1);
        assert_eq!(t.active_count(), 2);
    }

    #[test]
    fn updates_existing_file_in_place() {
        let mut t = tracker(2);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec!["v1".into()], 3);
        let idx = t.file_modified(PathBuf::from("/tmp/a.txt"), vec!["v2".into()], 3);
        assert_eq!(idx, 0);
        assert_eq!(t.panels[0].lines, vec!["v2"]);
        // Should not have grown
        assert_eq!(t.active_count(), 1);
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
        assert_eq!(t.panels[0].display_name, "c.txt");
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
        assert_eq!(t.panels[1].display_name, "c.txt");
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

        let tracked = &t.panels[0];
        assert_eq!(tracked.lines.len(), 500); // MAX_LINES_PER_FILE
        assert_eq!(tracked.lines.last().unwrap(), "line 599");
    }

    #[test]
    fn file_deleted_marks_flag() {
        let mut t = tracker(1);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec![], 0);
        assert!(!t.panels[0].is_deleted);

        t.file_deleted(&PathBuf::from("/tmp/a.txt"));
        assert!(t.panels[0].is_deleted);
    }

    #[test]
    fn gc_stale_clears_old_deleted() {
        let mut t = tracker(1);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec![], 0);
        t.file_deleted(&PathBuf::from("/tmp/a.txt"));

        // With zero timeout, should clear immediately
        t.gc_stale(std::time::Duration::ZERO);
        assert_eq!(t.active_count(), 0);
        assert!(t.panel_index(&PathBuf::from("/tmp/a.txt")).is_none());
    }

    #[test]
    fn gc_stale_keeps_non_deleted() {
        let mut t = tracker(1);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec![], 0);

        t.gc_stale(std::time::Duration::ZERO);
        assert_eq!(t.active_count(), 1); // not deleted, should remain
    }

    #[test]
    fn display_name_strips_watch_root() {
        let mut t = FileTracker::new(1, PathBuf::from("/home/user/logs"));
        t.file_modified(PathBuf::from("/home/user/logs/sub/output.txt"), vec![], 0);
        assert_eq!(t.panels[0].display_name, "sub/output.txt");
    }

    #[test]
    fn truncate_cmd_short_unchanged() {
        assert_eq!(super::truncate_cmd("tail -f x.txt", 60), "tail -f x.txt");
    }

    #[test]
    fn truncate_cmd_exact_limit() {
        let s = "a".repeat(60);
        assert_eq!(super::truncate_cmd(&s, 60), s);
    }

    #[test]
    fn truncate_cmd_over_limit() {
        let s = "a".repeat(70);
        let result = super::truncate_cmd(&s, 60);
        assert_eq!(result.len(), 60);
        assert!(result.ends_with("..."));
        assert_eq!(&result[..57], &"a".repeat(57));
    }

    #[test]
    fn file_mtime_is_set_from_filesystem() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.txt");
        std::fs::write(&path, "hello").unwrap();

        let expected_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        let mut t = FileTracker::new(1, tmp.path().to_path_buf());
        t.file_modified(path, vec!["hello".into()], 5);

        let tracked = &t.panels[0];
        // file_mtime should be very close to the actual mtime
        let diff = tracked.file_mtime.duration_since(expected_mtime).unwrap_or_default();
        assert!(diff.as_millis() < 100);
    }

    #[test]
    fn file_mtime_updates_on_append() {
        use tempfile::TempDir;
        use std::io::Write;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.txt");
        std::fs::write(&path, "line1\n").unwrap();

        let mut t = FileTracker::new(1, tmp.path().to_path_buf());
        t.file_modified(path.clone(), vec!["line1".into()], 6);
        let mtime_before = t.panels[0].file_mtime;

        // Wait a bit and append
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"line2\n").unwrap();
        f.flush().unwrap();

        t.append_lines(0, vec!["line2".into()], 12);
        let mtime_after = t.panels[0].file_mtime;

        assert!(mtime_after > mtime_before);
    }

    #[test]
    fn gc_stale_compacts_indices() {
        let mut t = tracker(4);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec!["a".into()], 2);
        t.file_modified(PathBuf::from("/tmp/b.txt"), vec!["b".into()], 2);
        t.file_modified(PathBuf::from("/tmp/c.txt"), vec!["c".into()], 2);

        // Delete b.txt (index 1) and gc
        t.file_deleted(&PathBuf::from("/tmp/b.txt"));
        t.gc_stale(std::time::Duration::ZERO);

        assert_eq!(t.active_count(), 2);
        assert_eq!(t.panels[0].display_name, "a.txt");
        assert_eq!(t.panels[1].display_name, "c.txt");
        // Indices should be updated
        assert_eq!(t.panel_index(&PathBuf::from("/tmp/a.txt")), Some(0));
        assert_eq!(t.panel_index(&PathBuf::from("/tmp/c.txt")), Some(1));
    }

    #[test]
    fn dynamic_growth() {
        let mut t = tracker(3);
        assert_eq!(t.active_count(), 0);

        t.file_modified(PathBuf::from("/tmp/a.txt"), vec![], 0);
        assert_eq!(t.active_count(), 1);

        t.file_modified(PathBuf::from("/tmp/b.txt"), vec![], 0);
        assert_eq!(t.active_count(), 2);

        t.file_modified(PathBuf::from("/tmp/c.txt"), vec![], 0);
        assert_eq!(t.active_count(), 3);

        // At max — should evict, not grow
        t.file_modified(PathBuf::from("/tmp/d.txt"), vec![], 0);
        assert_eq!(t.active_count(), 3);
    }

    #[test]
    fn file_deleted_unknown_path_is_noop() {
        let mut t = tracker(2);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec![], 0);
        t.file_deleted(&PathBuf::from("/tmp/nonexistent.txt"));
        assert_eq!(t.active_count(), 1);
        assert!(!t.panels[0].is_deleted);
    }

    #[test]
    fn re_add_deleted_file_clears_deleted_flag() {
        let mut t = tracker(2);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec!["v1".into()], 3);
        t.file_deleted(&PathBuf::from("/tmp/a.txt"));
        assert!(t.panels[0].is_deleted);

        // Re-modify same file
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec!["v2".into()], 3);
        assert!(!t.panels[0].is_deleted);
        assert_eq!(t.panels[0].lines, vec!["v2"]);
    }

    #[test]
    fn gc_stale_respects_timeout() {
        let mut t = tracker(2);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec![], 0);
        t.file_deleted(&PathBuf::from("/tmp/a.txt"));

        // With a long timeout, should NOT be removed
        t.gc_stale(std::time::Duration::from_secs(3600));
        assert_eq!(t.active_count(), 1);
    }

    #[test]
    fn gc_stale_multiple_deletions() {
        let mut t = tracker(4);
        t.file_modified(PathBuf::from("/tmp/a.txt"), vec!["a".into()], 2);
        t.file_modified(PathBuf::from("/tmp/b.txt"), vec!["b".into()], 2);
        t.file_modified(PathBuf::from("/tmp/c.txt"), vec!["c".into()], 2);
        t.file_modified(PathBuf::from("/tmp/d.txt"), vec!["d".into()], 2);

        // Delete a and c (indices 0 and 2)
        t.file_deleted(&PathBuf::from("/tmp/a.txt"));
        t.file_deleted(&PathBuf::from("/tmp/c.txt"));
        t.gc_stale(std::time::Duration::ZERO);

        assert_eq!(t.active_count(), 2);
        assert_eq!(t.panels[0].display_name, "b.txt");
        assert_eq!(t.panels[1].display_name, "d.txt");
        assert_eq!(t.panel_index(&PathBuf::from("/tmp/b.txt")), Some(0));
        assert_eq!(t.panel_index(&PathBuf::from("/tmp/d.txt")), Some(1));
    }

    #[test]
    fn display_name_outside_watch_root() {
        let mut t = FileTracker::new(1, PathBuf::from("/home/user/logs"));
        t.file_modified(PathBuf::from("/other/path/file.txt"), vec![], 0);
        // Outside watch root — should use full path
        assert_eq!(t.panels[0].display_name, "/other/path/file.txt");
    }
}
