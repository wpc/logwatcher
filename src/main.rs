mod app;
mod cli;
mod event;
mod file_tracker;
mod llm;
mod tail_reader;
mod tui;
mod ui;

use anyhow::Result;
use clap::Parser;
use cli::Args;
use app::App;
use crossterm::event::KeyCode;
use event::{AppEvent, EventHandler};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let dir = validate_dir(&args.dir)?;

    tui::install_panic_hook();
    let mut terminal = tui::init()?;

    let result = run(&args, &dir, &mut terminal).await;

    tui::restore()?;
    result
}

fn validate_dir(dir: &std::path::Path) -> Result<std::path::PathBuf> {
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if !canonical.is_dir() {
        anyhow::bail!("'{}' is not a directory", canonical.display());
    }
    Ok(canonical)
}

async fn run(args: &Args, dir: &std::path::Path, terminal: &mut tui::Tui) -> Result<()> {
    let mut app = App::new(args);

    initial_scan(&mut app, args)?;

    let (mut events, event_tx) = EventHandler::new(
        std::time::Duration::from_millis(args.tick_rate_ms),
        dir.to_path_buf(),
        args.glob.clone(),
    )?;

    // Spawn LLM summaries for initially scanned panels
    if let Some(ref url) = args.llm_api_url {
        for panel in &app.tracker.panels {
            if let Some(ref cmd) = panel.process_cmd {
                llm::spawn_summary(
                    event_tx.clone(),
                    panel.path.clone(),
                    cmd.clone(),
                    url.clone(),
                    args.llm_log_file.clone(),
                );
            }
        }
    }

    terminal.draw(|f| ui::render(f, &app))?;

    loop {
        if let Some(event) = events.next().await {
            dispatch_event(&mut app, event, &event_tx, &args.llm_api_url, &args.llm_log_file);
            terminal.draw(|f| ui::render(f, &app))?;
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn initial_scan(app: &mut App, args: &Args) -> Result<()> {
    let dir = args.dir.canonicalize().unwrap_or_else(|_| args.dir.clone());

    let mut entries = walkdir(&dir, &args.glob)?;

    entries.sort_by(|a, b| {
        let ma = std::fs::metadata(a).and_then(|m| m.modified()).ok();
        let mb = std::fs::metadata(b).and_then(|m| m.modified()).ok();
        mb.cmp(&ma) // most recent first
    });

    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(args.scan_back_minutes * 60);

    for path in entries.into_iter()
        .filter(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .map(|mtime| mtime > cutoff)
                .unwrap_or(false)
        })
        .take(app.max_panels)
    {
        match tail_reader::read_tail(&path, app.tail_lines) {
            Ok((lines, size)) => {
                let idx = app.tracker.file_modified(path.clone(), lines, size);
                app.ensure_scroll_offset(idx);
                app.tracker.panels[idx].process_cmd = file_tracker::lookup_process(&path);
            }
            Err(_) => {}
        }
    }

    Ok(())
}

fn walkdir(dir: &std::path::Path, glob_pattern: &Option<String>) -> Result<Vec<std::path::PathBuf>> {
    let mut results = Vec::new();
    walkdir_recursive(dir, glob_pattern, &mut results)?;
    Ok(results)
}

fn walkdir_recursive(
    dir: &std::path::Path,
    glob_pattern: &Option<String>,
    results: &mut Vec<std::path::PathBuf>,
) -> Result<()> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walkdir_recursive(&path, glob_pattern, results)?;
        } else if path.is_file() {
            if let Some(ref pattern) = glob_pattern {
                if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                    if !glob_match::glob_match(pattern, filename) {
                        continue;
                    }
                } else {
                    continue;
                }
            }
            results.push(path);
        }
    }
    Ok(())
}

fn handle_file_changed(
    app: &mut App,
    path: &std::path::Path,
    event_tx: &tokio::sync::mpsc::UnboundedSender<AppEvent>,
    llm_api_url: &Option<String>,
    llm_log_file: &Option<std::path::PathBuf>,
) {
    if let Some(panel_idx) = app.tracker.panel_index(path) {
        let last_size = app.tracker.panels[panel_idx].last_size;
        match tail_reader::read_new_content(path, last_size, app.tail_lines) {
            Ok((new_lines, new_size)) => {
                if !new_lines.is_empty() {
                    app.tracker.append_lines(panel_idx, new_lines, new_size);
                } else {
                    app.tracker.panels[panel_idx].last_size = new_size;
                    app.tracker.panels[panel_idx].last_modified = std::time::Instant::now();
                }
                // Lookup process if not yet known
                if app.tracker.panels[panel_idx].process_cmd.is_none() {
                    app.tracker.panels[panel_idx].process_cmd = file_tracker::lookup_process(path);
                    if let Some(ref url) = llm_api_url {
                        if let Some(ref cmd) = app.tracker.panels[panel_idx].process_cmd {
                            llm::spawn_summary(
                                event_tx.clone(),
                                path.to_path_buf(),
                                cmd.clone(),
                                url.clone(),
                                llm_log_file.clone(),
                            );
                        }
                    }
                }
            }
            Err(_) => {
                app.tracker.file_deleted(&path.to_path_buf());
            }
        }
    } else {
        // New file
        match tail_reader::read_tail(path, app.tail_lines) {
            Ok((lines, size)) => {
                let idx = app.tracker.file_modified(path.to_path_buf(), lines, size);
                app.ensure_scroll_offset(idx);
                app.tracker.panels[idx].process_cmd = file_tracker::lookup_process(path);
                if let Some(ref url) = llm_api_url {
                    if let Some(ref cmd) = app.tracker.panels[idx].process_cmd {
                        llm::spawn_summary(
                            event_tx.clone(),
                            path.to_path_buf(),
                            cmd.clone(),
                            url.clone(),
                            llm_log_file.clone(),
                        );
                    }
                }
            }
            Err(_) => {}
        }
    }
}

fn dispatch_event(
    app: &mut App,
    event: AppEvent,
    event_tx: &tokio::sync::mpsc::UnboundedSender<AppEvent>,
    llm_api_url: &Option<String>,
    llm_log_file: &Option<std::path::PathBuf>,
) {
    match event {
        AppEvent::Key(key) => {
            handle_key(app, key);
        }
        AppEvent::Resize => {
            // ratatui handles resize automatically on next draw
        }
        AppEvent::FileChanged(path) => {
            handle_file_changed(app, &path, event_tx, llm_api_url, llm_log_file);
        }
        AppEvent::FileDeleted(path) => {
            app.tracker.file_deleted(&path);
        }
        AppEvent::Tick => {
            app.tracker.gc_stale(app.stale_timeout);
            app.clamp_selected_panel();
        }
        AppEvent::ProcessSummaryReady { path, summary } => {
            if let Some(idx) = app.tracker.panel_index(&path) {
                app.tracker.panels[idx].process_summary = Some(summary);
            }
        }
    }
}

fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) {
    let active = app.tracker.active_count();
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
        }
        KeyCode::Tab => {
            if active > 0 {
                app.selected_panel = (app.selected_panel + 1) % active;
            }
        }
        KeyCode::BackTab => {
            if active > 0 {
                app.selected_panel = if app.selected_panel == 0 {
                    active - 1
                } else {
                    app.selected_panel - 1
                };
            }
        }
        KeyCode::Char(c @ '1'..='9') => {
            let idx = (c as usize) - ('1' as usize);
            if idx < active {
                app.selected_panel = idx;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if active == 0 { return; }
            app.ensure_scroll_offset(app.selected_panel);
            if let Some(offset) = app.scroll_offsets.get_mut(app.selected_panel) {
                *offset = offset.saturating_add(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if active == 0 { return; }
            app.ensure_scroll_offset(app.selected_panel);
            if let Some(offset) = app.scroll_offsets.get_mut(app.selected_panel) {
                *offset = offset.saturating_sub(1);
            }
        }
        KeyCode::PageUp => {
            if active == 0 { return; }
            app.ensure_scroll_offset(app.selected_panel);
            if let Some(offset) = app.scroll_offsets.get_mut(app.selected_panel) {
                *offset = offset.saturating_add(20);
            }
        }
        KeyCode::PageDown => {
            if active == 0 { return; }
            app.ensure_scroll_offset(app.selected_panel);
            if let Some(offset) = app.scroll_offsets.get_mut(app.selected_panel) {
                *offset = offset.saturating_sub(20);
            }
        }
        KeyCode::End | KeyCode::Char('G') => {
            if active == 0 { return; }
            app.ensure_scroll_offset(app.selected_panel);
            if let Some(offset) = app.scroll_offsets.get_mut(app.selected_panel) {
                *offset = 0;
            }
        }
        KeyCode::Home | KeyCode::Char('g') => {
            if active == 0 { return; }
            app.ensure_scroll_offset(app.selected_panel);
            if let Some(offset) = app.scroll_offsets.get_mut(app.selected_panel) {
                *offset = app.tracker.panels[app.selected_panel].lines.len();
            }
        }
        KeyCode::Char('?') => {
            app.show_help = !app.show_help;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn make_args(dir: std::path::PathBuf) -> Args {
        Args {
            dir,
            max_panels: 4,
            tail_lines: 50,
            stale_seconds: 30,
            tick_rate_ms: 250,
            glob: None,
            llm_api_url: None,
            llm_log_file: None,
            scan_back_minutes: 30,
        }
    }

    fn make_test_tx() -> tokio::sync::mpsc::UnboundedSender<AppEvent> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        tx
    }

    #[test]
    fn validate_dir_accepts_valid_directory() {
        let tmp = TempDir::new().unwrap();
        let result = validate_dir(tmp.path());
        assert!(result.is_ok());
    }

    #[test]
    fn validate_dir_rejects_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("not_a_dir.txt");
        std::fs::write(&file, "data").unwrap();
        let result = validate_dir(&file);
        assert!(result.is_err());
    }

    #[test]
    fn validate_dir_rejects_nonexistent() {
        let result = validate_dir(std::path::Path::new("/nonexistent/path/xyz"));
        assert!(result.is_err());
    }

    #[test]
    fn walkdir_finds_files_recursively() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
        std::fs::write(sub.join("b.txt"), "b").unwrap();

        let results = walkdir(tmp.path(), &None).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn walkdir_applies_glob_filter() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
        std::fs::write(tmp.path().join("b.log"), "b").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "c").unwrap();

        let results = walkdir(tmp.path(), &Some("*.log".to_string())).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].to_string_lossy().contains("b.log"));
    }

    #[test]
    fn walkdir_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let results = walkdir(tmp.path(), &None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn initial_scan_skips_old_files() {
        let tmp = TempDir::new().unwrap();

        // Create a file and backdate it to 2 hours ago
        let old_path = tmp.path().join("old.txt");
        std::fs::write(&old_path, "old content").unwrap();
        let two_hours_ago = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600),
        );
        filetime::set_file_mtime(&old_path, two_hours_ago).unwrap();

        // Create a recent file
        std::fs::write(tmp.path().join("new.txt"), "new content").unwrap();

        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        // Only the recent file should be loaded
        assert_eq!(app.tracker.active_count(), 1);
        assert!(app.tracker.panels[0].display_name.contains("new.txt"));
    }

    #[test]
    fn initial_scan_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        assert_eq!(app.tracker.active_count(), 0);
    }

    #[test]
    fn handle_key_quit() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        assert!(!app.should_quit);
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn handle_key_tab_cycles_panels() {
        let tmp = TempDir::new().unwrap();
        // Create 4 files so we have 4 active panels
        for name in &["a.txt", "b.txt", "c.txt", "d.txt"] {
            std::fs::write(tmp.path().join(name), "content").unwrap();
        }
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();
        assert_eq!(app.tracker.active_count(), 4);
        assert_eq!(app.selected_panel, 0);

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.selected_panel, 1);

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.selected_panel, 2);

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.selected_panel, 3);

        // Wraps around
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.selected_panel, 0);
    }

    #[test]
    fn handle_key_number_selects_panel() {
        let tmp = TempDir::new().unwrap();
        for name in &["a.txt", "b.txt", "c.txt", "d.txt"] {
            std::fs::write(tmp.path().join(name), "content").unwrap();
        }
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('3')));
        assert_eq!(app.selected_panel, 2);

        // Out of range ignored (only 4 panels active)
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('9')));
        assert_eq!(app.selected_panel, 2); // unchanged
    }

    #[test]
    fn handle_key_tab_noop_when_empty() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        assert_eq!(app.selected_panel, 0);

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.selected_panel, 0); // no panels, stays at 0
    }

    #[test]
    fn handle_key_scroll() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "content").unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        assert_eq!(app.scroll_offsets[0], 0);
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Up));
        assert_eq!(app.scroll_offsets[0], 1);
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Up));
        assert_eq!(app.scroll_offsets[0], 2);
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Down));
        assert_eq!(app.scroll_offsets[0], 1);

        // Down at 0 stays at 0
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Down));
        assert_eq!(app.scroll_offsets[0], 0);
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Down));
        assert_eq!(app.scroll_offsets[0], 0);
    }

    #[test]
    fn handle_key_help_toggle() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        assert!(!app.show_help);
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('?')));
        assert!(app.show_help);
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('?')));
        assert!(!app.show_help);
    }

    #[test]
    fn handle_file_changed_new_file() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, "hello\nworld\n").unwrap();

        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        handle_file_changed(&mut app, &file_path, &make_test_tx(), &None, &None);

        assert!(app.tracker.panel_index(&file_path).is_some());
        let idx = app.tracker.panel_index(&file_path).unwrap();
        let tracked = &app.tracker.panels[idx];
        assert_eq!(tracked.lines, vec!["hello", "world"]);
    }

    #[test]
    fn handle_file_changed_appends() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, "line1\n").unwrap();

        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        handle_file_changed(&mut app, &file_path, &make_test_tx(), &None, &None);
        let idx = app.tracker.panel_index(&file_path).unwrap();
        let size_after_first = app.tracker.panels[idx].last_size;

        // Append more content
        let mut f = std::fs::OpenOptions::new().append(true).open(&file_path).unwrap();
        f.write_all(b"line2\n").unwrap();
        f.flush().unwrap();

        handle_file_changed(&mut app, &file_path, &make_test_tx(), &None, &None);
        let tracked = &app.tracker.panels[idx];
        assert_eq!(tracked.lines, vec!["line1", "line2"]);
        assert!(tracked.last_size > size_after_first);
    }

    #[test]
    fn handle_key_esc_quits() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        assert!(!app.should_quit);
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Esc));
        assert!(app.should_quit);
    }

    #[test]
    fn handle_key_backtab_cycles_reverse() {
        let tmp = TempDir::new().unwrap();
        for name in &["a.txt", "b.txt", "c.txt"] {
            std::fs::write(tmp.path().join(name), "content").unwrap();
        }
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();
        assert_eq!(app.selected_panel, 0);

        // BackTab from 0 wraps to last
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::BackTab));
        assert_eq!(app.selected_panel, 2);

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::BackTab));
        assert_eq!(app.selected_panel, 1);

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::BackTab));
        assert_eq!(app.selected_panel, 0);
    }

    #[test]
    fn handle_key_page_scroll() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "content").unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::PageUp));
        assert_eq!(app.scroll_offsets[0], 20);

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::PageDown));
        assert_eq!(app.scroll_offsets[0], 0);
    }

    #[test]
    fn handle_key_end_resets_scroll() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "content").unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        // Scroll up then End should reset to 0 (follow mode)
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Up));
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Up));
        assert_eq!(app.scroll_offsets[0], 2);

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::End));
        assert_eq!(app.scroll_offsets[0], 0);
    }

    #[test]
    fn handle_key_home_scrolls_to_top() {
        let tmp = TempDir::new().unwrap();
        // Create a file with multiple lines
        let content: String = (0..20).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(tmp.path().join("a.txt"), &content).unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Home));
        // Should set scroll to total lines count
        assert_eq!(app.scroll_offsets[0], app.tracker.panels[0].lines.len());
    }

    #[test]
    fn handle_key_vim_bindings() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "content").unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        // 'k' scrolls up
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('k')));
        assert_eq!(app.scroll_offsets[0], 1);

        // 'j' scrolls down
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('j')));
        assert_eq!(app.scroll_offsets[0], 0);
    }

    #[test]
    fn handle_file_changed_deleted_file() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, "hello\n").unwrap();

        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        handle_file_changed(&mut app, &file_path, &make_test_tx(), &None, &None);
        assert_eq!(app.tracker.active_count(), 1);

        // Delete the file, then trigger a change event
        std::fs::remove_file(&file_path).unwrap();
        handle_file_changed(&mut app, &file_path, &make_test_tx(), &None, &None);

        // Should be marked deleted
        assert!(app.tracker.panels[0].is_deleted);
    }

    #[test]
    fn initial_scan_respects_max_panels() {
        let tmp = TempDir::new().unwrap();
        for i in 0..10 {
            std::fs::write(tmp.path().join(format!("file{}.txt", i)), "content").unwrap();
        }
        let mut args = make_args(tmp.path().to_path_buf());
        args.max_panels = 3;
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        assert_eq!(app.tracker.active_count(), 3);
    }

    #[test]
    fn clamp_selected_panel_after_gc() {
        let tmp = TempDir::new().unwrap();
        for name in &["a.txt", "b.txt", "c.txt"] {
            std::fs::write(tmp.path().join(name), "content").unwrap();
        }
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        // Select last panel
        app.selected_panel = 2;

        // Delete all files and gc
        for name in &["a.txt", "b.txt", "c.txt"] {
            app.tracker.file_deleted(&tmp.path().join(name));
        }
        app.tracker.gc_stale(std::time::Duration::ZERO);
        app.clamp_selected_panel();

        assert_eq!(app.tracker.active_count(), 0);
        assert_eq!(app.selected_panel, 0);
    }

    #[test]
    fn ensure_scroll_offset_grows() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        assert!(app.scroll_offsets.is_empty());
        app.ensure_scroll_offset(2);
        assert_eq!(app.scroll_offsets.len(), 3);
        assert_eq!(app.scroll_offsets, vec![0, 0, 0]);
    }

    #[test]
    fn handle_file_changed_grows_panels_dynamically() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        assert_eq!(app.tracker.active_count(), 0);

        // Add files one by one via handle_file_changed
        for i in 0..3 {
            let p = tmp.path().join(format!("f{}.txt", i));
            std::fs::write(&p, format!("content {}", i)).unwrap();
            handle_file_changed(&mut app, &p, &make_test_tx(), &None, &None);
        }

        assert_eq!(app.tracker.active_count(), 3);
        assert_eq!(app.scroll_offsets.len(), 3);
    }

    #[test]
    fn handle_file_changed_no_new_content() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, "hello\n").unwrap();

        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        handle_file_changed(&mut app, &file_path, &make_test_tx(), &None, &None);

        let idx = app.tracker.panel_index(&file_path).unwrap();
        let size_before = app.tracker.panels[idx].last_size;

        // Trigger change without modifying the file — no new content
        handle_file_changed(&mut app, &file_path, &make_test_tx(), &None, &None);
        assert_eq!(app.tracker.panels[idx].last_size, size_before);
    }

    #[test]
    fn clamp_selected_panel_noop_when_valid() {
        let tmp = TempDir::new().unwrap();
        for name in &["a.txt", "b.txt", "c.txt"] {
            std::fs::write(tmp.path().join(name), "content").unwrap();
        }
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        app.selected_panel = 1;
        app.clamp_selected_panel();
        // Already valid, should be unchanged
        assert_eq!(app.selected_panel, 1);
    }

    #[test]
    fn clamp_selected_panel_reduces_to_last() {
        let tmp = TempDir::new().unwrap();
        for name in &["a.txt", "b.txt", "c.txt"] {
            std::fs::write(tmp.path().join(name), "content").unwrap();
        }
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        // selected_panel beyond active count, but panels still exist
        app.selected_panel = 5;
        app.clamp_selected_panel();
        assert_eq!(app.selected_panel, 2); // clamped to last (count - 1)
    }

    #[test]
    fn handle_key_scroll_noop_when_empty() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        // All scroll keys should be no-ops when no panels
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Up));
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Down));
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::PageUp));
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::PageDown));
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Home));
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::End));
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('j')));
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('k')));
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('g')));
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('G')));
        assert!(app.scroll_offsets.is_empty());
    }

    #[test]
    fn handle_key_g_uppercase_resets_scroll() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "content").unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Up));
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Up));
        assert_eq!(app.scroll_offsets[0], 2);

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('G')));
        assert_eq!(app.scroll_offsets[0], 0);
    }

    #[test]
    fn handle_key_unknown_key_is_noop() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('x')));
        assert!(!app.should_quit);
        assert!(!app.show_help);
    }

    #[test]
    fn walkdir_skips_directories() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(tmp.path().join("file.txt"), "data").unwrap();

        let results = walkdir(tmp.path(), &None).unwrap();
        // Only the file, not the directory
        assert_eq!(results.len(), 1);
        assert!(results[0].to_string_lossy().contains("file.txt"));
    }

    #[test]
    fn dispatch_event_key_quit() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        let key = crossterm::event::KeyEvent::from(KeyCode::Char('q'));
        dispatch_event(&mut app, AppEvent::Key(key), &make_test_tx(), &None, &None);
        assert!(app.should_quit);
    }

    #[test]
    fn dispatch_event_resize() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        // Resize should not panic or change state
        dispatch_event(&mut app, AppEvent::Resize, &make_test_tx(), &None, &None);
        assert!(!app.should_quit);
    }

    #[test]
    fn dispatch_event_file_changed() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, "hello\n").unwrap();

        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        dispatch_event(&mut app, AppEvent::FileChanged(file_path.clone()), &make_test_tx(), &None, &None);
        assert_eq!(app.tracker.active_count(), 1);
    }

    #[test]
    fn dispatch_event_file_deleted() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, "hello\n").unwrap();

        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        dispatch_event(&mut app, AppEvent::FileChanged(file_path.clone()), &make_test_tx(), &None, &None);
        assert_eq!(app.tracker.active_count(), 1);

        dispatch_event(&mut app, AppEvent::FileDeleted(file_path), &make_test_tx(), &None, &None);
        assert!(app.tracker.panels[0].is_deleted);
    }

    #[test]
    fn dispatch_event_tick_gc() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, "hello\n").unwrap();

        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        app.stale_timeout = std::time::Duration::ZERO;

        dispatch_event(&mut app, AppEvent::FileChanged(file_path.clone()), &make_test_tx(), &None, &None);
        dispatch_event(&mut app, AppEvent::FileDeleted(file_path), &make_test_tx(), &None, &None);
        // Tick should gc the stale deleted panel
        dispatch_event(&mut app, AppEvent::Tick, &make_test_tx(), &None, &None);
        assert_eq!(app.tracker.active_count(), 0);
    }
}
