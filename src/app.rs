use crate::cli::Args;
use crate::file_tracker::FileTracker;

pub struct App {
    pub should_quit: bool,
    pub tracker: FileTracker,
    pub max_panels: usize,
    pub tail_lines: usize,
    pub stale_timeout: std::time::Duration,
    pub scroll_offsets: Vec<usize>,
    pub selected_panel: usize,
    pub show_help: bool,
}

impl App {
    pub fn new(args: &Args) -> Self {
        let dir = args.dir.canonicalize().unwrap_or_else(|_| args.dir.clone());
        Self {
            should_quit: false,
            tracker: FileTracker::new(args.max_panels, dir),
            max_panels: args.max_panels,
            tail_lines: args.tail_lines,
            stale_timeout: std::time::Duration::from_secs(args.stale_seconds),
            scroll_offsets: Vec::new(),
            selected_panel: 0,
            show_help: false,
        }
    }

    /// Ensure scroll_offsets has an entry for `idx`, growing with 0s if needed.
    pub fn ensure_scroll_offset(&mut self, idx: usize) {
        if idx >= self.scroll_offsets.len() {
            self.scroll_offsets.resize(idx + 1, 0);
        }
    }

    /// Clamp selected_panel to valid range after panels shrink.
    pub fn clamp_selected_panel(&mut self) {
        let count = self.tracker.active_count();
        if count == 0 {
            self.selected_panel = 0;
        } else if self.selected_panel >= count {
            self.selected_panel = count - 1;
        }
    }
}
