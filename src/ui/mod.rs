//! Rendering: dispatch by screen state and lay out the top-level regions.

mod footer;
mod header;
mod loading;
pub mod theme;
mod tree_view;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::{App, Screen};

/// Render the current frame for `app`.
pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if matches!(app.screen, Screen::Loaded(_)) {
        render_loaded(frame, app, area);
    } else if matches!(app.screen, Screen::Loading) {
        loading::render(frame, app, area);
    } else if let Screen::Error(message) = &app.screen {
        render_error(frame, message, area);
    }
}

fn render_loaded(frame: &mut Frame, app: &mut App, area: Rect) {
    // Copy scalar/owned view state out before borrowing `screen` mutably.
    let root_label = app.root_label.clone();
    let sort_key = app.sort_key;
    let sort_dir = app.sort_dir;
    let Screen::Loaded(loaded) = &mut app.screen else {
        return;
    };

    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    header::render(
        frame,
        &root_label,
        &loaded.tree,
        loaded.duration,
        loaded.inaccurate,
        header_area,
    );
    tree_view::render(frame, loaded, body_area);
    footer::render(frame, sort_key, sort_dir, footer_area);
}

fn render_error(frame: &mut Frame, message: &str, area: Rect) {
    let text = vec![
        Line::from(Span::styled(
            "Error",
            Style::default()
                .fg(theme::WARN)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw(message.to_string())),
        Line::from(Span::styled(
            "press q to quit",
            Style::default().fg(theme::MUTED),
        )),
    ];
    let [row] = Layout::vertical([Constraint::Length(5)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::horizontal([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(row);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .block(Block::bordered().border_style(Style::default().fg(theme::WARN))),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::model::build_tree;
    use crate::scan::ScanOutcome;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::{Path, PathBuf};
    use std::time::Duration;
    use tokei::{Language, LanguageType, Languages, Report};

    fn sample_app() -> App {
        let mk = |path: &str, code: usize, comments: usize, blanks: usize| {
            let mut r = Report::new(PathBuf::from(path));
            r.stats.code = code;
            r.stats.comments = comments;
            r.stats.blanks = blanks;
            r
        };
        let mut rust = Language::new();
        rust.reports = vec![
            mk("/proj/src/main.rs", 120, 10, 8),
            mk("/proj/src/app.rs", 60, 4, 3),
        ];
        let mut md = Language::new();
        md.reports = vec![mk("/proj/README.md", 20, 0, 6)];
        let mut languages = Languages::new();
        languages.insert(LanguageType::Rust, rust);
        languages.insert(LanguageType::Markdown, md);

        let tree = build_tree(&languages, Path::new("/proj"), "proj".into());
        let mut app = App::new(PathBuf::from("/proj"), "proj".into());
        app.on_scan(ScanOutcome {
            tree,
            duration: Duration::from_millis(12),
            inaccurate: false,
        });
        app
    }

    #[test]
    fn renders_loaded_tree_without_panicking() {
        let mut app = sample_app();
        let mut terminal = Terminal::new(TestBackend::new(96, 16)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", terminal.backend());
        // Run `cargo test -- --nocapture` and uncomment to eyeball the layout:
        // eprintln!("\n{view}");
        assert!(view.contains("tree-tui"));
        assert!(view.contains("lines"));
        assert!(view.contains("src"));
        assert!(view.contains("README.md"));
    }
}
