mod app;
mod cli;
mod event;
mod file_tracker;
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

    let dir = args.dir.canonicalize().unwrap_or_else(|_| args.dir.clone());
    if !dir.is_dir() {
        anyhow::bail!("'{}' is not a directory", dir.display());
    }

    tui::install_panic_hook();
    let mut terminal = tui::init()?;

    let result = run(&args, &mut terminal).await;

    tui::restore()?;
    result
}

async fn run(args: &Args, terminal: &mut tui::Tui) -> Result<()> {
    let mut app = App::new(args);

    initial_scan(&mut app, args)?;

    let dir = args.dir.canonicalize().unwrap_or_else(|_| args.dir.clone());
    let mut events = EventHandler::new(
        std::time::Duration::from_millis(args.tick_rate_ms),
        dir,
        args.glob.clone(),
    )?;

    terminal.draw(|f| ui::render(f, &app))?;

    loop {
        if let Some(event) = events.next().await {
            match event {
                AppEvent::Key(key) => {
                    handle_key(&mut app, key);
                }
                AppEvent::Resize(_, _) => {
                    // ratatui handles resize automatically on next draw
                }
                AppEvent::FileChanged(path) => {
                    handle_file_changed(&mut app, &path);
                }
                AppEvent::FileDeleted(path) => {
                    app.tracker.file_deleted(&path);
                }
                AppEvent::Tick => {
                    app.tracker.gc_stale(app.stale_timeout);
                }
            }

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

    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 60);

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
                if let Some(ref mut tracked) = app.tracker.panels[idx] {
                    tracked.process_cmd = file_tracker::lookup_process(&path);
                }
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

fn handle_file_changed(app: &mut App, path: &std::path::Path) {
    if let Some(panel_idx) = app.tracker.panel_index(path) {
        let last_size = app.tracker.panels[panel_idx]
            .as_ref()
            .map(|t| t.last_size)
            .unwrap_or(0);
        match tail_reader::read_new_content(path, last_size, app.tail_lines) {
            Ok((new_lines, new_size)) => {
                if !new_lines.is_empty() {
                    app.tracker.append_lines(panel_idx, new_lines, new_size);
                } else if let Some(ref mut tracked) = app.tracker.panels[panel_idx] {
                    tracked.last_size = new_size;
                    tracked.last_modified = std::time::Instant::now();
                }
                // Lookup process if not yet known
                if app.tracker.panels[panel_idx].as_ref().map_or(true, |t| t.process_cmd.is_none()) {
                    if let Some(ref mut tracked) = app.tracker.panels[panel_idx] {
                        tracked.process_cmd = file_tracker::lookup_process(path);
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
                if let Some(ref mut tracked) = app.tracker.panels[idx] {
                    tracked.process_cmd = file_tracker::lookup_process(path);
                }
            }
            Err(_) => {}
        }
    }
}

fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
        }
        KeyCode::Tab => {
            app.selected_panel = (app.selected_panel + 1) % app.max_panels;
        }
        KeyCode::BackTab => {
            app.selected_panel = if app.selected_panel == 0 {
                app.max_panels - 1
            } else {
                app.selected_panel - 1
            };
        }
        KeyCode::Char(c @ '1'..='9') => {
            let idx = (c as usize) - ('1' as usize);
            if idx < app.max_panels {
                app.selected_panel = idx;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(offset) = app.scroll_offsets.get_mut(app.selected_panel) {
                *offset = offset.saturating_add(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(offset) = app.scroll_offsets.get_mut(app.selected_panel) {
                *offset = offset.saturating_sub(1);
            }
        }
        KeyCode::PageUp => {
            if let Some(offset) = app.scroll_offsets.get_mut(app.selected_panel) {
                *offset = offset.saturating_add(20);
            }
        }
        KeyCode::PageDown => {
            if let Some(offset) = app.scroll_offsets.get_mut(app.selected_panel) {
                *offset = offset.saturating_sub(20);
            }
        }
        KeyCode::End | KeyCode::Char('G') => {
            if let Some(offset) = app.scroll_offsets.get_mut(app.selected_panel) {
                *offset = 0;
            }
        }
        KeyCode::Home | KeyCode::Char('g') => {
            if let Some(offset) = app.scroll_offsets.get_mut(app.selected_panel) {
                let total = app.tracker.panels[app.selected_panel]
                    .as_ref()
                    .map(|t| t.lines.len())
                    .unwrap_or(0);
                *offset = total;
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
        }
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
        let populated: Vec<_> = app.tracker.panels.iter().filter(|p| p.is_some()).collect();
        assert_eq!(populated.len(), 1);
        assert!(populated[0].as_ref().unwrap().display_name.contains("new.txt"));
    }

    #[test]
    fn initial_scan_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
        initial_scan(&mut app, &args).unwrap();

        assert!(app.tracker.panels.iter().all(|p| p.is_none()));
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
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);
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
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('3')));
        assert_eq!(app.selected_panel, 2);

        // Out of range ignored
        handle_key(&mut app, crossterm::event::KeyEvent::from(KeyCode::Char('9')));
        assert_eq!(app.selected_panel, 2); // unchanged
    }

    #[test]
    fn handle_key_scroll() {
        let tmp = TempDir::new().unwrap();
        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

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

        handle_file_changed(&mut app, &file_path);

        assert!(app.tracker.panel_index(&file_path).is_some());
        let idx = app.tracker.panel_index(&file_path).unwrap();
        let tracked = app.tracker.panels[idx].as_ref().unwrap();
        assert_eq!(tracked.lines, vec!["hello", "world"]);
    }

    #[test]
    fn handle_file_changed_appends() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, "line1\n").unwrap();

        let args = make_args(tmp.path().to_path_buf());
        let mut app = App::new(&args);

        handle_file_changed(&mut app, &file_path);
        let idx = app.tracker.panel_index(&file_path).unwrap();
        let size_after_first = app.tracker.panels[idx].as_ref().unwrap().last_size;

        // Append more content
        let mut f = std::fs::OpenOptions::new().append(true).open(&file_path).unwrap();
        f.write_all(b"line2\n").unwrap();
        f.flush().unwrap();

        handle_file_changed(&mut app, &file_path);
        let tracked = app.tracker.panels[idx].as_ref().unwrap();
        assert_eq!(tracked.lines, vec!["line1", "line2"]);
        assert!(tracked.last_size > size_after_first);
    }
}
