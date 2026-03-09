use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use crate::app::App;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    if app.show_help {
        render_help(frame, area);
        return;
    }

    // Count active panels (non-None) for layout, but always allocate max_panels grid
    let n = app.max_panels;
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
    let panel = &app.tracker.panels[panel_idx];

    match panel {
        None => {
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" [{}] (waiting...) ", panel_idx + 1))
                .border_style(Style::default().fg(Color::DarkGray));
            frame.render_widget(block, area);
        }
        Some(tracked) => {
            let is_selected = panel_idx == app.selected_panel;

            let title_style = if tracked.is_deleted {
                Style::default().fg(Color::Red).add_modifier(Modifier::DIM)
            } else if is_selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Green)
            };

            let deleted_marker = if tracked.is_deleted { " [deleted]" } else { "" };
            let process_info = tracked.process_cmd.as_ref()
                .map(|cmd| format!(" ({})", cmd))
                .unwrap_or_default();
            let title = format!(" [{}] {}{}{} ", panel_idx + 1, tracked.display_name, process_info, deleted_marker);

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

            let visible_height = inner.height as usize;
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

            frame.render_widget(paragraph, inner);
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
}
