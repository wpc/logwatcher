# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run

```bash
source "$HOME/.cargo/env"  # if cargo not in PATH
cargo build                # dev build
cargo build --release      # release build
cargo run -- /path/to/dir  # run watching a directory
cargo run -- /path/to/dir -n 6 -g "*.log"  # 6 panels, glob filter
```

No tests or linter configured yet.

## Architecture

Logwather is a Rust TUI app that tails the N most recently changed files in a directory, displaying each in its own terminal panel. Designed for monitoring ephemeral task outputs (e.g., Claude Code subagent logs).

### Event-driven async architecture

Three event sources merge into a single `mpsc::UnboundedReceiver<AppEvent>` channel consumed by the main loop in `main.rs`:

1. **Crossterm** (keyboard/resize) — via `EventStream` async stream
2. **notify** (filesystem create/modify/delete) — bridged from sync callback to async channel
3. **Tick timer** — periodic UI refresh and stale panel garbage collection

All three are spawned as tokio tasks in `event.rs::EventHandler::new()`. The main loop in `main.rs::run()` calls `events.next().await`, dispatches the event, then redraws.

### Panel management (`file_tracker.rs`)

`FileTracker` maintains a fixed-size `Vec<Option<TrackedFile>>` where index = panel position on screen. A `HashMap<PathBuf, usize>` provides O(1) lookup from file path to panel index. When all panels are full and a new file arrives, the least-recently-modified panel is evicted (deleted files evicted first). `gc_stale()` is called on each tick to clear deleted files past the stale timeout.

### Rendering (`ui.rs`)

`compute_grid()` dynamically computes panel layout: N=1 fullscreen, N=2 horizontal split (top/bottom for readability), N≥3 uses `ceil(sqrt(N))` columns. Each panel renders a scrollable view of its `TrackedFile.lines` buffer (capped at 500 lines in `file_tracker.rs::MAX_LINES_PER_FILE`).

### File I/O (`tail_reader.rs`)

`read_tail()` reads the last N lines on initial open. `read_new_content()` does incremental reads from `last_size` offset, detecting truncation (log rotation) by checking if `current_size < last_size` and re-reading the tail in that case.

### Key modules

- `main.rs` — orchestration: CLI parse, event loop, `initial_scan()`, `handle_file_changed()`, `handle_key()`
- `event.rs` — `AppEvent` enum, `EventHandler` merging all event sources
- `file_tracker.rs` — `TrackedFile`, `FileTracker` with LRU eviction
- `ui.rs` — grid layout computation, panel rendering, help overlay
- `tail_reader.rs` — file tailing with truncation detection
- `cli.rs` — clap `Args` struct
- `app.rs` — `App` state (wraps `FileTracker` + UI state)
- `tui.rs` — terminal init/restore/panic hook
