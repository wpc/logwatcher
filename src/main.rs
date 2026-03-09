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

    for path in entries.into_iter().take(app.max_panels) {
        match tail_reader::read_tail(&path, app.tail_lines) {
            Ok((lines, size)) => {
                app.tracker.file_modified(path, lines, size);
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
            }
            Err(_) => {
                app.tracker.file_deleted(&path.to_path_buf());
            }
        }
    } else {
        // New file
        match tail_reader::read_tail(path, app.tail_lines) {
            Ok((lines, size)) => {
                app.tracker.file_modified(path.to_path_buf(), lines, size);
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
