use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};
use crate::app::App;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    if app.show_help {
        render_help(frame, area);
        return;
    }

    let n = app.tracker.active_count();
    if n == 0 {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" logwatcher (no files) ")
            .border_style(Style::default().fg(Color::DarkGray));
        frame.render_widget(block, area);
        return;
    }

    let panel_areas = compute_grid(area, n);

    for (i, panel_area) in panel_areas.iter().enumerate() {
        render_panel(frame, app, i, *panel_area);
    }
}

/// Compute a grid of Rect areas for N panels.
/// Prefers horizontal splits for readability.
fn compute_grid(area: Rect, n: usize) -> Vec<Rect> {
    if n == 0 {
        return vec![];
    }
    if n == 1 {
        return vec![area];
    }
    if n == 2 {
        // Horizontal split: top/bottom
        let chunks = Layout::vertical([
            Constraint::Ratio(1, 2),
            Constraint::Ratio(1, 2),
        ]).split(area);
        return vec![chunks[0], chunks[1]];
    }

    // General grid: cols = ceil(sqrt(n)), rows = ceil(n/cols)
    let cols = (n as f64).sqrt().ceil() as usize;
    let rows = (n + cols - 1) / cols;

    let row_constraints: Vec<Constraint> = (0..rows)
        .map(|_| Constraint::Ratio(1, rows as u32))
        .collect();
    let row_areas = Layout::vertical(row_constraints).split(area);

    let mut panels = Vec::with_capacity(n);
    let mut panel_idx = 0;

    for (row_i, &row_area) in row_areas.iter().enumerate() {
        let panels_in_row = if row_i < rows - 1 {
            cols
        } else {
            n - panel_idx
        };
        let col_constraints: Vec<Constraint> = (0..panels_in_row)
            .map(|_| Constraint::Ratio(1, panels_in_row as u32))
            .collect();
        let col_areas = Layout::horizontal(col_constraints).split(row_area);

        for &col_area in col_areas.iter() {
            panels.push(col_area);
            panel_idx += 1;
            if panel_idx >= n {
                break;
            }
        }
    }

    panels
}

fn render_panel(frame: &mut Frame, app: &App, panel_idx: usize, area: Rect) {
    let tracked = &app.tracker.panels[panel_idx];
    let is_selected = panel_idx == app.selected_panel;

    let title_style = if tracked.is_deleted {
        Style::default().fg(Color::Red).add_modifier(Modifier::DIM)
    } else if is_selected {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };

    let deleted_marker = if tracked.is_deleted { " [deleted]" } else { "" };
    let title = format!(" [{}] {}{} ", panel_idx + 1, tracked.display_name, deleted_marker);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(title_style)
        .title_bottom(Line::from(format!(" {} ", format_elapsed_since(tracked.file_mtime)))
            .right_aligned()
            .style(Style::default().fg(Color::DarkGray)))
        .border_style(if is_selected {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        });

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split inner area: log content on top, optional 2-line command bar at bottom
    let has_bar = tracked.process_cmd.is_some() || tracked.process_summary.is_some();
    let (content_area, cmd_area) = if has_bar && inner.height > 2 {
        let chunks = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(2),
        ]).split(inner);
        (chunks[0], Some(chunks[1]))
    } else {
        (inner, None)
    };

    let visible_height = content_area.height as usize;
    if visible_height == 0 {
        return;
    }

    let total_lines = tracked.lines.len();
    let scroll = app.scroll_offsets.get(panel_idx).copied().unwrap_or(0);

    let end = if total_lines > scroll {
        total_lines - scroll
    } else {
        0
    };
    let start = if end > visible_height {
        end - visible_height
    } else {
        0
    };

    let visible_text = tracked.lines[start..end].join("\n");
    let paragraph = Paragraph::new(visible_text)
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, content_area);

    // Show scrollbar when content overflows
    if total_lines > visible_height {
        let position = if total_lines > scroll + visible_height {
            total_lines - scroll - visible_height
        } else {
            0
        };
        let mut scrollbar_state = ScrollbarState::new(total_lines)
            .position(position);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(Color::DarkGray)),
            content_area,
            &mut scrollbar_state,
        );
    }

    // Render command bar
    if let Some(cmd_rect) = cmd_area {
        let width = cmd_rect.width as usize;
        if let Some(ref summary) = tracked.process_summary {
            // Line 1: LLM summary (cyan), Line 2: truncated raw cmd (dark gray)
            let summary_line = Line::from(Span::styled(summary.clone(), Style::default().fg(Color::Cyan)));
            let cmd_line = if let Some(ref cmd) = tracked.process_cmd {
                let text = if cmd.len() > width.saturating_sub(3) {
                    format!("{}...", &cmd[..width.saturating_sub(3)])
                } else {
                    cmd.clone()
                };
                Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)))
            } else {
                Line::default()
            };
            let cmd_paragraph = Paragraph::new(vec![summary_line, cmd_line]);
            frame.render_widget(cmd_paragraph, cmd_rect);
        } else if let Some(ref cmd) = tracked.process_cmd {
            let max_chars = width * 2;
            let text = if cmd.len() > max_chars {
                format!("{}...", &cmd[..max_chars.saturating_sub(3)])
            } else {
                cmd.clone()
            };
            let cmd_paragraph = Paragraph::new(text)
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: false });
            frame.render_widget(cmd_paragraph, cmd_rect);
        }
    }
}

fn render_help(frame: &mut Frame, area: Rect) {
    let help_text = vec![
        Line::from(Span::styled("Logwatcher - Help", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("Navigation", Style::default().fg(Color::Yellow))),
        Line::from("  Tab / Shift+Tab    Cycle panels"),
        Line::from("  1-9                Select panel directly"),
        Line::from(""),
        Line::from(Span::styled("Scrolling", Style::default().fg(Color::Yellow))),
        Line::from("  j / Down           Scroll down (newer)"),
        Line::from("  k / Up             Scroll up (older)"),
        Line::from("  PgUp / PgDn        Scroll by page"),
        Line::from("  G / End            Jump to bottom (follow)"),
        Line::from("  g / Home           Jump to top"),
        Line::from(""),
        Line::from(Span::styled("General", Style::default().fg(Color::Yellow))),
        Line::from("  ?                  Toggle this help"),
        Line::from("  q / Esc            Quit"),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help ")
        .title_style(Style::default().fg(Color::Cyan))
        .border_style(Style::default().fg(Color::Cyan));

    let paragraph = Paragraph::new(help_text).block(block);
    // Center the help overlay
    let help_area = centered_rect(60, 70, area);
    frame.render_widget(ratatui::widgets::Clear, help_area);
    frame.render_widget(paragraph, help_area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ]).split(r);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ]).split(popup_layout[1])[1]
}

fn format_elapsed_since(t: std::time::SystemTime) -> String {
    let elapsed = t.elapsed().unwrap_or_default();
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use crate::file_tracker::FileTracker;

    fn make_app(max_panels: usize) -> App {
        App {
            should_quit: false,
            tracker: FileTracker::new(max_panels, std::path::PathBuf::from("/tmp")),
            max_panels,
            tail_lines: 50,
            stale_timeout: Duration::from_secs(30),
            scroll_offsets: Vec::new(),
            selected_panel: 0,
            show_help: false,
        }
    }

    #[test]
    fn format_elapsed_seconds() {
        let t = SystemTime::now() - Duration::from_secs(5);
        let result = format_elapsed_since(t);
        assert!(result.ends_with("s ago"));
    }

    #[test]
    fn format_elapsed_minutes() {
        let t = SystemTime::now() - Duration::from_secs(120);
        assert_eq!(format_elapsed_since(t), "2m ago");
    }

    #[test]
    fn format_elapsed_hours() {
        let t = SystemTime::now() - Duration::from_secs(7200);
        assert_eq!(format_elapsed_since(t), "2h ago");
    }

    #[test]
    fn format_elapsed_just_now() {
        let t = SystemTime::now();
        assert_eq!(format_elapsed_since(t), "0s ago");
    }

    #[test]
    fn compute_grid_returns_correct_count() {
        let area = Rect::new(0, 0, 100, 50);
        assert_eq!(compute_grid(area, 0).len(), 0);
        assert_eq!(compute_grid(area, 1).len(), 1);
        assert_eq!(compute_grid(area, 2).len(), 2);
        assert_eq!(compute_grid(area, 4).len(), 4);
        assert_eq!(compute_grid(area, 6).len(), 6);
    }

    #[test]
    fn render_empty_shows_no_files() {
        let app = make_app(4);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();

        let buf = terminal.backend().buffer().clone();
        let content = buffer_to_string(&buf);
        assert!(content.contains("no files"));
    }

    #[test]
    fn render_one_panel() {
        let mut app = make_app(4);
        app.tracker.file_modified(
            std::path::PathBuf::from("/tmp/test.log"),
            vec!["hello world".into(), "second line".into()],
            24,
        );
        app.ensure_scroll_offset(0);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();

        let buf = terminal.backend().buffer().clone();
        let content = buffer_to_string(&buf);
        assert!(content.contains("test.log"));
        assert!(content.contains("hello world"));
    }

    #[test]
    fn render_two_panels() {
        let mut app = make_app(4);
        app.tracker.file_modified(
            std::path::PathBuf::from("/tmp/a.log"),
            vec!["aaa".into()],
            4,
        );
        app.tracker.file_modified(
            std::path::PathBuf::from("/tmp/b.log"),
            vec!["bbb".into()],
            4,
        );
        app.ensure_scroll_offset(1);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();

        let buf = terminal.backend().buffer().clone();
        let content = buffer_to_string(&buf);
        assert!(content.contains("a.log"));
        assert!(content.contains("b.log"));
    }

    #[test]
    fn render_selected_panel() {
        let mut app = make_app(4);
        app.tracker.file_modified(
            std::path::PathBuf::from("/tmp/a.log"),
            vec!["aaa".into()],
            4,
        );
        app.tracker.file_modified(
            std::path::PathBuf::from("/tmp/b.log"),
            vec!["bbb".into()],
            4,
        );
        app.ensure_scroll_offset(1);
        app.selected_panel = 1;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();

        // Should render without panic, panel 1 selected
        let buf = terminal.backend().buffer().clone();
        let content = buffer_to_string(&buf);
        assert!(content.contains("b.log"));
    }

    #[test]
    fn render_deleted_panel() {
        let mut app = make_app(4);
        app.tracker.file_modified(
            std::path::PathBuf::from("/tmp/gone.log"),
            vec!["old data".into()],
            9,
        );
        app.tracker.file_deleted(&std::path::PathBuf::from("/tmp/gone.log"));
        app.ensure_scroll_offset(0);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();

        let buf = terminal.backend().buffer().clone();
        let content = buffer_to_string(&buf);
        assert!(content.contains("deleted"));
    }

    #[test]
    fn render_panel_with_process_cmd() {
        let mut app = make_app(4);
        app.tracker.file_modified(
            std::path::PathBuf::from("/tmp/out.log"),
            vec!["output".into()],
            7,
        );
        app.tracker.panels[0].process_cmd = Some("tail -f /tmp/out.log".into());
        app.ensure_scroll_offset(0);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();

        let buf = terminal.backend().buffer().clone();
        let content = buffer_to_string(&buf);
        // Command should appear in the bottom bar, not parenthesized in title
        assert!(content.contains("tail -f /tmp/out.log"));
        assert!(!content.contains("(tail -f"));
    }

    #[test]
    fn render_panel_with_long_process_cmd() {
        let mut app = make_app(4);
        app.tracker.file_modified(
            std::path::PathBuf::from("/tmp/out.log"),
            vec!["output".into()],
            7,
        );
        // Command longer than 2 lines of 78 chars (inner width) = 156 chars
        let long_cmd = "a".repeat(200);
        app.tracker.panels[0].process_cmd = Some(long_cmd);
        app.ensure_scroll_offset(0);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();

        let buf = terminal.backend().buffer().clone();
        let content = buffer_to_string(&buf);
        assert!(content.contains("..."));
    }

    #[test]
    fn render_help_overlay() {
        let mut app = make_app(4);
        app.show_help = true;

        let backend = TestBackend::new(80, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();

        let buf = terminal.backend().buffer().clone();
        let content = buffer_to_string(&buf);
        assert!(content.contains("Help"));
        assert!(content.contains("Tab"));
        assert!(content.contains("Quit"));
    }

    #[test]
    fn render_with_scrollbar() {
        let mut app = make_app(4);
        // Create enough lines to exceed panel height
        let lines: Vec<String> = (0..100).map(|i| format!("line {}", i)).collect();
        app.tracker.file_modified(
            std::path::PathBuf::from("/tmp/big.log"),
            lines,
            5000,
        );
        app.ensure_scroll_offset(0);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        // Should render without panic including scrollbar
        terminal.draw(|f| render(f, &app)).unwrap();
    }

    #[test]
    fn render_scrolled_up_panel() {
        let mut app = make_app(4);
        let lines: Vec<String> = (0..100).map(|i| format!("line {}", i)).collect();
        app.tracker.file_modified(
            std::path::PathBuf::from("/tmp/big.log"),
            lines,
            5000,
        );
        app.ensure_scroll_offset(0);
        app.scroll_offsets[0] = 50; // scrolled up

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();

        let buf = terminal.backend().buffer().clone();
        let content = buffer_to_string(&buf);
        // Should show lines around position 50, not the latest
        assert!(!content.contains("line 99"));
    }

    #[test]
    fn centered_rect_fits_inside() {
        let outer = Rect::new(0, 0, 100, 50);
        let inner = centered_rect(60, 70, outer);
        assert!(inner.x >= outer.x);
        assert!(inner.y >= outer.y);
        assert!(inner.right() <= outer.right());
        assert!(inner.bottom() <= outer.bottom());
        assert!(inner.width > 0);
        assert!(inner.height > 0);
    }

    #[test]
    fn render_scroll_beyond_content() {
        let mut app = make_app(4);
        app.tracker.file_modified(
            std::path::PathBuf::from("/tmp/small.log"),
            vec!["line1".into(), "line2".into()],
            12,
        );
        app.ensure_scroll_offset(0);
        // Scroll way beyond total lines
        app.scroll_offsets[0] = 999;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        // Should not panic, just shows nothing
        terminal.draw(|f| render(f, &app)).unwrap();
    }

    #[test]
    fn render_scrollbar_at_top() {
        let mut app = make_app(4);
        let lines: Vec<String> = (0..100).map(|i| format!("line {}", i)).collect();
        app.tracker.file_modified(
            std::path::PathBuf::from("/tmp/big.log"),
            lines,
            5000,
        );
        app.ensure_scroll_offset(0);
        // Scroll to near the top — scroll + visible_height >= total_lines
        app.scroll_offsets[0] = 90;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();
    }

    /// Helper: flatten a ratatui Buffer into a single string for assertions
    fn buffer_to_string(buf: &ratatui::buffer::Buffer) -> String {
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push_str(buf.cell((x, y)).map_or(" ", |c| c.symbol()));
            }
        }
        s
    }
}
