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
            scroll_offsets: vec![0; args.max_panels],
            selected_panel: 0,
            show_help: false,
        }
    }
}
