//! Rendering: dispatch by screen state and lay out the top-level regions.

pub mod fileview;
mod footer;
mod header;
mod help;
mod loading;
pub mod preview;
pub mod reader;
pub mod theme;
mod tree_view;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::{App, Focus, Mode, PaneRects, Screen};

/// Minimum body width/height before the preview pane is shown; below either, it
/// folds away so the tree keeps the room.
const PREVIEW_MIN_WIDTH: u16 = 100;
const PREVIEW_MIN_HEIGHT: u16 = 20;
/// Denominator of the tree/preview split.
///
/// Finer than the width it divides — no terminal has ten thousand columns — so
/// every column of the body is addressable and a width recorded by a drag comes
/// back as that same width. At one part in 100 a 400-column body would only
/// resolve to every 4th column, and at one part in 1000 the round trip starts
/// losing a column once the body passes 1000.
pub const SPLIT_SCALE: u16 = 10_000;
/// Share of the body width the preview pane takes before the divider is dragged.
pub const DEFAULT_SPLIT_SHARE: u16 = SPLIT_SCALE / 5 * 2; // 40%
/// Narrowest either body pane may be left by a divider drag: two borders, the
/// selection gutter, and `NAME_FLOOR`-ish of content — enough that a tree row
/// still reads as a name and the preview still fits a short line. The drag
/// clamps at this from both ends, so it can never dismiss a pane; that stays
/// `Tab` / `p`.
pub const PANE_MIN: u16 = 24;

/// Hold a preview width inside the range that leaves both panes usable.
///
/// On a body too narrow to give both panes `PANE_MIN` the upper bound wins, so
/// the result never exceeds the body — but that only arises below
/// `PREVIEW_MIN_WIDTH`, where the pane has already folded away and nothing asks.
pub fn clamp_preview_width(body_width: u16, want: u16) -> u16 {
    let ceiling = body_width.saturating_sub(PANE_MIN).min(body_width);
    want.clamp(PANE_MIN.min(ceiling), ceiling)
}

/// Width the preview pane takes in a `body` of the given width at `share`
/// thousandths.
///
/// The layout uses an explicit `Length` rather than a `Percentage` constraint so
/// this is the *same* arithmetic the drag handler inverts — routing it through
/// ratatui's constraint solver instead would leave the seam a cell off the
/// cursor.
pub fn preview_width(body_width: u16, share: u16) -> u16 {
    let scale = u32::from(SPLIT_SCALE);
    // Rounded, and rounded again by `width_to_share` on the way in, so that a
    // width recorded by a drag comes back as the same width. Truncating at
    // either end costs a column, which is the difference between the divider
    // sitting under the pointer and a cell away from it.
    let want = (u32::from(body_width) * u32::from(share) + scale / 2) / scale;
    clamp_preview_width(body_width, want as u16)
}

/// The share a preview of `width` occupies in `body_width`, in thousandths —
/// the inverse of [`preview_width`], used to record a dragged divider.
pub fn width_to_share(body_width: u16, width: u16) -> u16 {
    if body_width == 0 {
        return DEFAULT_SPLIT_SHARE;
    }
    let body = u32::from(body_width);
    // Round rather than truncate: `preview_width` floors on the way back, and
    // two floors in a row cost a column, which is the difference between the
    // divider landing under the pointer and a cell short of it.
    ((u32::from(width) * u32::from(SPLIT_SCALE) + body / 2) / body) as u16
}

/// Render the current frame for `app`.
pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if matches!(app.screen, Screen::Loaded(_)) {
        render_loaded(frame, app, area);
    } else if matches!(app.screen, Screen::Loading) {
        loading::render(frame, app, area);
    } else if let Screen::Reader(reader) = &mut app.screen {
        reader::render(frame, reader, area);
    } else if let Screen::Error(message) = &app.screen {
        render_error(frame, message, area);
    }

    if app.show_help {
        help::render(frame, area);
    }
}

fn render_loaded(frame: &mut Frame, app: &mut App, area: Rect) {
    // Copy/borrow scalar state out before borrowing `screen` mutably.
    let root_label = app.root_label.clone();
    let head_hash = app.head_hash.clone();
    let editing = app.mode == Mode::Filter;
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

    header::render(
        frame,
        &root_label,
        head_hash.as_deref(),
        loaded,
        header_area,
    );

    // The preview pane takes the dragged share of the width, folding away on a
    // narrow or short terminal so the tree keeps the room.
    let show_preview = loaded.show_preview
        && body_area.width >= PREVIEW_MIN_WIDTH
        && body_area.height >= PREVIEW_MIN_HEIGHT;

    let (tree_area, preview_area) = if show_preview {
        let width = preview_width(body_area.width, loaded.split_share);
        let [tree, preview] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(width)]).areas(body_area);
        (tree, Some(preview))
    } else {
        (body_area, None)
    };

    // Record pane rects for the next frame's mouse hit-testing, and keep focus
    // on the tree when the preview has folded away. The divider is the seam
    // where the tree's right border meets the preview's left one — two columns,
    // so the grab target is not a single-cell pixel hunt.
    loaded.panes = PaneRects {
        body: body_area,
        tree: tree_area,
        preview: preview_area,
        divider: preview_area.map(|_| {
            Rect::new(
                tree_area.right().saturating_sub(1),
                body_area.y,
                2,
                body_area.height,
            )
        }),
    };
    if preview_area.is_none() {
        if loaded.focus == Focus::Preview {
            loaded.focus = Focus::Tree;
        }
        // The pane the divider divides is gone, so any drag still held on it is
        // over. Without this, motion keeps rewriting the split against a seam
        // nobody can see, and the change only surfaces when the pane returns.
        loaded.split_drag = None;
    }

    if loaded.visible.is_empty() {
        render_empty(frame, &loaded.filter, tree_area);
    } else {
        tree_view::render(frame, loaded, tree_area);
    }

    if let Some(preview_area) = preview_area {
        // The preview content is loaded off the render path — debounced by the
        // event loop so a held key / wheel spin never pays a file read + syntax
        // highlight per frame (that synchronous cost is what made nav lurch).
        preview::render(frame, loaded, preview_area);
    } else {
        // The pane folded away: retract any Kitty reservation it left behind, or
        // the post-draw flush would paint its image over the widened tree.
        loaded.preview.state.clear_pending_image();
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
            head: None,
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
    fn renders_the_head_hash_left_of_the_loc_summary() {
        let mut app = sample_app();
        app.head_hash = Some("9ee4e1e".into());
        let mut terminal = Terminal::new(TestBackend::new(96, 16)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", terminal.backend());
        // The hash sits on the recap line, to the left of the "lines" total.
        let recap = view
            .lines()
            .find(|line| line.contains("lines"))
            .expect("the code lens recap line is rendered");
        let hash_at = recap.find("9ee4e1e").expect("the head hash is shown");
        let lines_at = recap.find("lines").unwrap();
        assert!(
            hash_at < lines_at,
            "hash must be left of the LOC summary:\n{view}"
        );
    }

    #[test]
    fn navigation_reuses_the_row_cache_but_content_changes_rebuild_it() {
        use crate::action::Action;
        let mut app = sample_app();
        let mut terminal = Terminal::new(TestBackend::new(96, 16)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();

        let rev = |app: &App| {
            let Screen::Loaded(l) = &app.screen else {
                panic!("not loaded");
            };
            l.rebuild_rev
        };
        let first = rev(&app);

        // The render populated a cache keyed to the current content, width, and
        // computing state.
        if let Screen::Loaded(l) = &app.screen {
            let c = l.row_cache.as_ref().expect("render populates the cache");
            assert_eq!(c.rev, first);
            assert_eq!(c.width, 96);
            assert_eq!(c.computing, l.active_computing());
        }

        // Pure navigation doesn't change content, so the rev is stable and the
        // next render reuses the cache.
        app.update(Action::Down);
        assert_eq!(rev(&app), first, "navigation must not rebuild content");
        terminal.draw(|frame| render(frame, &mut app)).unwrap();

        // A content change bumps the rev; the next render rebuilds to match.
        app.update(Action::CycleSort);
        let after = rev(&app);
        assert!(after > first, "sorting changes content");
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        if let Screen::Loaded(l) = &app.screen {
            assert_eq!(l.row_cache.as_ref().unwrap().rev, after);
        }
    }

    #[test]
    fn excluding_a_directory_updates_the_rendered_header_totals() {
        use crate::action::Action;
        let mut app = sample_app();
        // Select the src directory (4000 + 2000 bytes across 2 files).
        if let Screen::Loaded(loaded) = &mut app.screen {
            let idx = loaded
                .visible
                .iter()
                .position(|&id| loaded.tree.nodes[id].name == "src")
                .expect("src is visible");
            loaded.table_state.select(Some(idx));
        }
        app.update(Action::ToggleExclude);

        let mut terminal = Terminal::new(TestBackend::new(96, 16)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", terminal.backend());
        // Only README.md remains counted, so the header drops from 3 files to 1.
        assert!(view.contains("1 files"), "totals should adjust:\n{view}");
        assert!(
            !view.contains("3 files"),
            "excluded files must not be counted:\n{view}"
        );
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
            head: None,
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
    fn renders_focused_text_preview_with_line_numbers() {
        use crate::app::Focus;
        let mut app = sample_app();
        // Inject a text preview, focus the preview pane, and pin the cache key.
        // The renderer never loads a preview itself (see `Loaded::ensure_preview`),
        // but a matching key keeps the injected content current for any path that
        // does.
        if let Screen::Loaded(loaded) = &mut app.screen {
            let src = b"fn main() {}\nlet x = 1;\n";
            let doc = super::fileview::FileDoc::prepare(
                std::path::Path::new("x.rs"),
                src,
                src.len() as u64,
                &super::preview::preview_limits(),
            );
            loaded.preview = super::preview::Preview::from_doc(doc);
            loaded.preview_for = loaded
                .selected_id()
                .map(|id| loaded.tree.nodes[id].rel_path.clone());
            loaded.focus = Focus::Preview;
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", terminal.backend());
        assert!(view.contains("preview"), "preview title missing:\n{view}");
        assert!(view.contains("fn main"), "code text missing:\n{view}");
        // The line-number gutter renders "1" and "2" for the two lines.
        assert!(
            view.contains('1') && view.contains('2'),
            "gutter missing:\n{view}"
        );
    }

    #[test]
    fn renders_the_help_overlay() {
        let mut app = sample_app();
        app.show_help = true;
        let mut terminal = Terminal::new(TestBackend::new(110, 18)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let help = format!("{}", terminal.backend());
        assert!(help.contains("keybindings"));
        assert!(help.contains("quit"));
    }

    /// Everything the retired detail panel used to say about the selected node
    /// now rides in the row itself: its file count, its on-disk size, and its
    /// share of the tree under the active lens.
    #[test]
    fn wide_rows_carry_the_files_size_and_share_columns() {
        let mut app = sample_app();
        let mut terminal = Terminal::new(TestBackend::new(180, 16)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", terminal.backend());

        for header in ["files", "size", "share", "languages", "lines"] {
            assert!(view.contains(header), "missing {header} column:\n{view}");
        }

        // The values, not just the headers. src aggregates 2 files and 6000
        // bytes and holds 180 of the 200 lines counted; README.md is 1 file of
        // 800 bytes and the other 20 lines.
        let row = |needle: &str| {
            view.lines()
                .find(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("no {needle} row:\n{view}"))
                .to_string()
        };
        let src = row("src/");
        assert!(src.contains("5.9 KB"), "src size:\n{src}");
        assert!(src.contains(" 2 "), "src file count:\n{src}");
        assert!(src.contains("90.0%"), "src share:\n{src}");
        let readme = row("README.md");
        assert!(readme.contains("800 B"), "README size:\n{readme}");
        assert!(readme.contains(" 1 "), "README file count:\n{readme}");
        assert!(readme.contains("10.0%"), "README share:\n{readme}");
    }

    /// `share` measures against the *root's* total, not the parent's, so the
    /// column always reads against the figure in the header.
    #[test]
    fn share_is_measured_against_the_root_not_the_parent() {
        use crate::action::Action;
        let mut app = sample_app();
        app.update(Action::ExpandAll);
        let mut terminal = Terminal::new(TestBackend::new(180, 16)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", terminal.backend());

        // main.rs is 120 lines: 60.0% of the root's 200, but 66.7% of src's 180.
        let row = view
            .lines()
            .find(|line| line.contains("main.rs"))
            .unwrap_or_else(|| panic!("no main.rs row:\n{view}"));
        assert!(row.contains("60.0%"), "not the root's total:\n{row}");
        assert!(
            !row.contains("66.7%"),
            "measured against the parent:\n{row}"
        );
    }

    #[test]
    fn preview_width_is_proportional_and_never_starves_a_pane() {
        // Proportional in the ordinary case, and exact at the round numbers.
        assert_eq!(preview_width(200, DEFAULT_SPLIT_SHARE), 80);
        assert_eq!(preview_width(200, SPLIT_SCALE / 2), 100);

        // Clamped at both ends, so neither pane can be squeezed away.
        assert_eq!(preview_width(200, 0), PANE_MIN);
        assert_eq!(preview_width(200, SPLIT_SCALE), 200 - PANE_MIN);

        // A body too narrow to give both panes the minimum can't render the
        // preview at all, but the width must still fit inside it.
        for body in [0, 1, PANE_MIN, PANE_MIN * 2 - 1] {
            let width = preview_width(body, DEFAULT_SPLIT_SHARE);
            assert!(width <= body, "preview {width} overflows a body of {body}");
        }
    }

    /// The drag records a width as a share; rendering turns it back into a
    /// width. The round trip has to be exact, or the seam drifts from the
    /// pointer — and drifts further the wider the terminal, which is why the
    /// widths here run past any real one.
    #[test]
    fn a_width_survives_the_round_trip_through_a_share() {
        for body in [48u16, 100, 120, 160, 240, 400, 1000, 1001, 1600] {
            for width in PANE_MIN..=(body - PANE_MIN) {
                let back = preview_width(body, width_to_share(body, width));
                assert_eq!(back, width, "width {width} of {body} did not survive");
            }
        }
    }

    /// The supplementary columns must not cost the code lens its language
    /// breakdown at an ordinary width — see `Columns::choose`, which trades them
    /// away before the legend.
    #[test]
    fn the_language_legend_survives_the_supplementary_columns() {
        let mut app = sample_app();
        let mut terminal = Terminal::new(TestBackend::new(96, 16)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", terminal.backend());
        assert!(view.contains("languages"), "legend column dropped:\n{view}");
        assert!(view.contains("Markdown"), "legend content dropped:\n{view}");
    }

    /// The divider's position is state, not a constant: the recorded pane rects
    /// follow `split_share`, and the drag handle sits on the seam between them.
    #[test]
    fn the_split_share_places_the_panes_and_the_divider() {
        let mut app = sample_app();
        if let Screen::Loaded(loaded) = &mut app.screen {
            loaded.split_share = SPLIT_SCALE / 4;
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();

        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded");
        };
        let preview = loaded.panes.preview.expect("preview is on screen");
        assert_eq!(preview.width, 30, "a quarter of a 120-column body");
        assert_eq!(loaded.panes.tree.width, 90);
        let divider = loaded.panes.divider.expect("divider recorded");
        assert_eq!((divider.x, divider.width), (89, 2));
        assert_eq!(preview.x, divider.x + 1, "the seam abuts the preview");
    }

    /// A share at either extreme still leaves both panes usable, so a stored or
    /// resized split can never squeeze one away.
    #[test]
    fn the_rendered_split_clamps_both_panes_at_the_minimum() {
        for share in [0, 1, SPLIT_SCALE - 1, SPLIT_SCALE] {
            let mut app = sample_app();
            if let Screen::Loaded(loaded) = &mut app.screen {
                loaded.split_share = share;
            }
            let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
            let Screen::Loaded(loaded) = &app.screen else {
                panic!("not loaded");
            };
            let preview = loaded.panes.preview.expect("preview is on screen");
            assert!(preview.width >= PANE_MIN, "preview starved at {share}");
            assert!(
                loaded.panes.tree.width >= PANE_MIN,
                "tree starved at {share}"
            );
        }
    }

    /// An excluded row contributes nothing to the total the `share` column
    /// measures against, so it has no share to report — and its own full value
    /// over a total that no longer contains it would read past 100%.
    #[test]
    fn an_excluded_row_reports_no_share() {
        use crate::action::Action;
        let mut app = sample_app();
        if let Screen::Loaded(loaded) = &mut app.screen {
            let idx = loaded
                .visible
                .iter()
                .position(|&id| loaded.tree.nodes[id].name == "src")
                .expect("src is visible");
            loaded.table_state.select(Some(idx));
        }
        app.update(Action::ToggleExclude);
        // Expanded, so the excluded directory's *children* render too: they are
        // excluded only by inheritance and must be treated the same way.
        app.update(Action::ExpandAll);

        let mut terminal = Terminal::new(TestBackend::new(180, 16)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let view = format!("{}", terminal.backend());
        // src holds 180 of 200 lines; excluded, the root total drops to 20, so
        // a naive ratio prints 900.0% for src and 600.0% for the main.rs under
        // it — the case an explicit-boundary check would miss.
        assert!(!view.contains("900.0%"), "incoherent share:\n{view}");
        assert!(
            !view.contains("600.0%"),
            "inherited exclusion missed:\n{view}"
        );
        for row in ["src/", "main.rs"] {
            let line = view
                .lines()
                .find(|l| l.contains(row))
                .unwrap_or_else(|| panic!("no {row} row:\n{view}"));
            assert!(line.contains('—'), "{row} still reports a share:\n{line}");
        }
        // The rows still counted keep a real share of the reduced total.
        assert!(
            view.contains("100.0%"),
            "README.md is now all of it:\n{view}"
        );
    }

    /// The seam can fold away mid-drag, and the frame that folds it is the one
    /// that knows: it ends the drag rather than leaving it to the next event.
    #[test]
    fn folding_the_preview_away_ends_a_drag_held_on_its_divider() {
        let mut app = sample_app();
        let mut wide = Terminal::new(TestBackend::new(120, 30)).unwrap();
        wide.draw(|frame| render(frame, &mut app)).unwrap();
        if let Screen::Loaded(loaded) = &mut app.screen {
            loaded.split_drag = Some(0);
        }

        // Too narrow for the preview: the pane folds, and the drag with it.
        let mut narrow = Terminal::new(TestBackend::new(80, 30)).unwrap();
        narrow.draw(|frame| render(frame, &mut app)).unwrap();
        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded")
        };
        assert!(loaded.panes.preview.is_none(), "the preview should fold");
        assert_eq!(loaded.split_drag, None, "the drag outlived its divider");
    }
}
