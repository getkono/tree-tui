//! Rendering: dispatch by screen state and lay out the top-level regions.

mod detail;
mod footer;
mod header;
mod help;
mod loading;
pub mod preview;
mod syntax;
pub mod theme;
mod tree_view;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::{App, Mode, Screen};

/// Minimum body width/height before the preview pane is shown; below either, it
/// folds away so the tree keeps the room.
const PREVIEW_MIN_WIDTH: u16 = 100;
const PREVIEW_MIN_HEIGHT: u16 = 20;
/// Share of the body width the preview pane takes when shown.
const PREVIEW_PCT: u16 = 40;
/// Fixed width of the detail panel when shown.
const DETAIL_WIDTH: u16 = 36;

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

    if app.show_help {
        help::render(frame, area);
    }
}

fn render_loaded(frame: &mut Frame, app: &mut App, area: Rect) {
    // Copy/borrow scalar state out before borrowing `screen` mutably.
    let root = app.root.clone();
    let root_label = app.root_label.clone();
    let editing = app.mode == Mode::Filter;
    let picker = app.picker.as_ref();
    let Screen::Loaded(loaded) = &mut app.screen else {
        return;
    };
    let loaded = loaded.as_mut();

    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    header::render(frame, &root_label, loaded, header_area);

    // Right-side panes appear as room allows: the detail panel takes a fixed
    // column; the preview pane takes a share of the width, folding away on a
    // narrow or short terminal.
    let show_preview = loaded.show_preview
        && body_area.width >= PREVIEW_MIN_WIDTH
        && body_area.height >= PREVIEW_MIN_HEIGHT;

    let mut constraints = vec![Constraint::Min(0)];
    if loaded.show_detail {
        constraints.push(Constraint::Length(DETAIL_WIDTH));
    }
    if show_preview {
        constraints.push(Constraint::Percentage(PREVIEW_PCT));
    }
    let chunks = Layout::horizontal(constraints).split(body_area);

    let tree_area = chunks[0];
    let mut next = 1;
    if loaded.show_detail {
        detail::render(frame, loaded, chunks[next]);
        next += 1;
    }
    let preview_area = show_preview.then(|| chunks[next]);

    if loaded.visible.is_empty() {
        render_empty(frame, &loaded.filter, tree_area);
    } else {
        tree_view::render(frame, loaded, tree_area);
    }

    if let Some(preview_area) = preview_area {
        loaded.ensure_preview(&root, picker);
        preview::render(frame, loaded, preview_area);
    }

    let computing = loaded.active_computing().then_some(loaded.active_lens);
    footer::render(
        frame,
        loaded.active_lens,
        loaded.sort_key,
        loaded.sort_dir,
        loaded.hide_zeros,
        &loaded.filter,
        editing,
        computing,
        footer_area,
    );
}

fn render_empty(frame: &mut Frame, filter: &str, area: Rect) {
    let message = if filter.is_empty() {
        "No files found here."
    } else {
        "No matches for this filter."
    };
    let block = Block::bordered()
        .border_style(Style::default().fg(theme::MUTED))
        .title(" tree ");
    let paragraph = Paragraph::new(vec![
        Line::default(),
        Line::from(Span::styled(message, Style::default().fg(theme::MUTED))),
    ])
    .alignment(Alignment::Center)
    .block(block);
    frame.render_widget(paragraph, area);
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
    use crate::app::{App, Screen};
    use crate::collect::LayerResult;
    use crate::model::{CodeData, CodeNum, Lens, build_skeleton};
    use crate::scan::ScanOutcome;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokei::LanguageType;

    fn code_data(lang: LanguageType, code: usize) -> CodeData {
        let num = CodeNum {
            code,
            comments: 0,
            blanks: 0,
        };
        let mut data = CodeData {
            num,
            primary_lang: Some(lang),
            ..Default::default()
        };
        data.langs.insert(lang, num);
        data
    }

    /// A loaded app for `/proj` with the code layer already computed.
    fn sample_app() -> App {
        let files = vec![
            (PathBuf::from("src/main.rs"), 4000),
            (PathBuf::from("src/app.rs"), 2000),
            (PathBuf::from("README.md"), 800),
        ];
        let dirs = vec![PathBuf::from("src")];
        let tree = build_skeleton(&files, &dirs, "proj".into());
        let mut app = App::new(PathBuf::from("/proj"), "proj".into());
        app.on_scan(ScanOutcome {
            tree,
            duration: Duration::from_millis(12),
            repo: false,
        });

        let mut layer = HashMap::new();
        layer.insert(
            PathBuf::from("src/main.rs"),
            code_data(LanguageType::Rust, 120),
        );
        layer.insert(
            PathBuf::from("src/app.rs"),
            code_data(LanguageType::Rust, 60),
        );
        layer.insert(
            PathBuf::from("README.md"),
            code_data(LanguageType::Markdown, 20),
        );
        app.on_layer(LayerResult::Code {
            files: layer,
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
        assert!(view.contains("tree"));
        assert!(view.contains("lines")); // code lens primary column
        assert!(view.contains("src"));
        assert!(view.contains("README.md"));
        assert!(view.contains("Markdown"));
    }

    #[test]
    fn renders_a_sole_subdir_chain_as_one_concatenated_row() {
        // `src/main/java` is a chain of sole sub-directories: it must render as a
        // single concatenated row, never as separate `main` / `java` rows.
        let files = vec![(PathBuf::from("src/main/java/App.java"), 100)];
        let dirs = vec![
            PathBuf::from("src"),
            PathBuf::from("src/main"),
            PathBuf::from("src/main/java"),
        ];
        let tree = build_skeleton(&files, &dirs, "proj".into());
        let mut app = App::new(PathBuf::from("/proj"), "proj".into());
        app.on_scan(ScanOutcome {
            tree,
            duration: Duration::ZERO,
            repo: false,
        });

        let mut terminal = Terminal::new(TestBackend::new(96, 16)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", terminal.backend());
        assert!(
            view.contains("src/main/java"),
            "chain not concatenated:\n{view}"
        );
    }

    #[test]
    fn renders_size_lens_with_human_bytes() {
        let mut app = sample_app();
        app.update(crate::action::Action::JumpLens(2)); // size lens
        if let Screen::Loaded(loaded) = &app.screen {
            assert_eq!(loaded.active_lens, Lens::Size);
        }
        let mut terminal = Terminal::new(TestBackend::new(96, 16)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", terminal.backend());
        assert!(view.contains("size")); // size lens primary column
        assert!(view.contains("KB")); // human-readable bytes
    }

    #[test]
    fn preview_pane_shows_when_wide_and_folds_when_narrow() {
        let mut app = sample_app();

        // Wide and tall: the preview pane is shown (the selected dir renders a
        // short note, but the bordered " preview " title is present).
        let mut wide = Terminal::new(TestBackend::new(120, 30)).unwrap();
        wide.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", wide.backend());
        assert!(
            view.contains("preview"),
            "preview missing when wide:\n{view}"
        );

        // Narrow: it folds away so the tree keeps the room.
        let mut narrow = Terminal::new(TestBackend::new(80, 16)).unwrap();
        narrow.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", narrow.backend());
        assert!(!view.contains("preview"), "preview should fold:\n{view}");

        // Toggled off: no pane even on a wide terminal.
        if let Screen::Loaded(loaded) = &mut app.screen {
            loaded.show_preview = false;
        }
        wide.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", wide.backend());
        assert!(!view.contains("preview"), "preview should be off:\n{view}");
    }

    #[test]
    fn renders_detail_panel_and_help_overlay() {
        let mut app = sample_app();
        if let Screen::Loaded(loaded) = &mut app.screen {
            loaded.show_detail = true;
        }
        let mut terminal = Terminal::new(TestBackend::new(110, 18)).unwrap();

        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let detail = format!("{}", terminal.backend());
        assert!(detail.contains("detail"));
        assert!(detail.contains("languages"));
        assert!(detail.contains("Rust"));

        app.show_help = true;
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let help = format!("{}", terminal.backend());
        assert!(help.contains("keybindings"));
        assert!(help.contains("quit"));
    }
}
