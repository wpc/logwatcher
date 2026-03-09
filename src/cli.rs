use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "logwatcher", about = "Multi-file tail watcher with split terminal panels")]
pub struct Args {
    /// Directory to watch for file changes
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// Maximum number of panels (files tailed simultaneously)
    #[arg(short = 'n', long = "panels", default_value_t = 4)]
    pub max_panels: usize,

    /// Number of lines to read initially when opening a file
    #[arg(short, long, default_value_t = 50)]
    pub tail_lines: usize,

    /// Seconds of inactivity before a panel can be recycled
    #[arg(short, long, default_value_t = 30)]
    pub stale_seconds: u64,

    /// Tick rate in milliseconds for UI refresh
    #[arg(long, default_value_t = 250)]
    pub tick_rate_ms: u64,

    /// Glob pattern to filter files (e.g. "*.log", "*.txt")
    #[arg(short, long)]
    pub glob: Option<String>,
}
