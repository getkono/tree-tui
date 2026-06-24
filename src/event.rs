//! The async event loop: input, walk completion, lazy lens computation, ticks.

use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyEventKind, MouseButton, MouseEventKind};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::{mpsc, oneshot};

use crate::app::{App, Screen};
use crate::collect::{self, LayerResult};
use crate::scan::ScanOutcome;
use crate::ui::reader::Handoff;
use crate::{editor, pager, scan, tui, ui, watch};

/// How long the selection must rest before the preview's (synchronous) file
/// read + syntax highlight runs. A held key / wheel spin keeps moving the
/// selection and so keeps pushing this out, keeping highlight off nav frames.
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(80);

/// Drive the app until the user quits or input is exhausted.
///
/// Redraws happen only on change: a state-changing key, a resize, walk
/// completion, a computed lens layer, or — while busy — a spinner tick. The tick
/// is armed only while the app is busy (the initial walk or a computing lens), so
/// an idle, fully-loaded tree consumes no CPU.
pub async fn run(terminal: &mut DefaultTerminal, app: &mut App) -> color_eyre::Result<()> {
    let started = Instant::now();
    let mut scan_rx = scan::spawn(app.root.clone(), app.root_label.clone());
    // Background lens computations report their results over this channel. The
    // original sender lives for the whole loop, so the receiver never closes.
    let (lens_tx, mut lens_rx) = mpsc::unbounded_channel::<LayerResult>();
    // Filesystem watch (best-effort): a coalesced signal triggers a re-walk.
    // The binding keeps the debouncer alive for the loop's lifetime.
    let mut fs_watch = watch::spawn(&app.root);
    let mut rescan_rx: Option<oneshot::Receiver<ScanOutcome>> = None;
    let mut rescan_again = false;
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(120));
    // Debounced preview refresh: the node the preview should show once the
    // selection settles, and the deadline at which to load it.
    let mut preview_target = app.preview_target_id();
    let mut preview_deadline: Option<tokio::time::Instant> = None;

    terminal.draw(|frame| ui::render(frame, app))?;

    while !app.should_quit {
        let mut redraw = false;
        tokio::select! {
            // One terminal event per iteration — the standard crossterm/ratatui
            // pattern. `apply_event` reports whether anything changed on screen,
            // so idle events (mouse motion is not even tracked) cost no redraw.
            maybe_event = events.next() => match maybe_event {
                Some(Ok(event)) => redraw = apply_event(terminal, app, event)?,
                Some(Err(err)) => return Err(err.into()),
                None => app.should_quit = true,
            },
            result = &mut scan_rx, if matches!(app.screen, Screen::Loading) => {
                match result {
                    Ok(outcome) => app.on_scan(outcome),
                    Err(_) => app.on_scan_failed("scan task failed unexpectedly"),
                }
                redraw = true;
            },
            Some(result) = lens_rx.recv() => {
                app.on_layer(result);
                redraw = true;
            },
            // A debounced filesystem change: kick off a re-walk, coalescing with
            // any already in flight (no redraw — nothing visible changes yet).
            Some(()) = async { fs_watch.as_mut().unwrap().rx.recv().await }, if fs_watch.is_some() => {
                if rescan_rx.is_none() {
                    rescan_rx = Some(scan::spawn(app.root.clone(), app.root_label.clone()));
                } else {
                    rescan_again = true;
                }
            },
            // A re-walk finished: merge it (a no-op when nothing visible changed),
            // then start another if a change landed while it was running.
            outcome = async { rescan_rx.as_mut().unwrap().await }, if rescan_rx.is_some() => {
                rescan_rx = None;
                if let Ok(outcome) = outcome {
                    app.on_rescan(outcome);
                }
                if rescan_again {
                    rescan_again = false;
                    rescan_rx = Some(scan::spawn(app.root.clone(), app.root_label.clone()));
                }
                redraw = true;
            },
            _ = ticker.tick(), if app.is_busy() => {
                app.spinner = app.spinner.wrapping_add(1);
                app.elapsed = started.elapsed();
                redraw = true;
            },
            // The selection has rested long enough: load the preview now (the
            // synchronous read + highlight happens here, off the nav frames).
            _ = async { tokio::time::sleep_until(preview_deadline.unwrap()).await },
                if preview_deadline.is_some() =>
            {
                preview_deadline = None;
                app.refresh_preview();
                redraw = true;
            },
        }

        // (Re)arm the preview debounce when the selection moves to a new,
        // not-yet-cached node; while navigating it keeps getting pushed out, so
        // the highlight runs only once the selection settles (~one debounce).
        let target = app.preview_target_id();
        if target != preview_target {
            preview_target = target;
            preview_deadline = target.map(|_| tokio::time::Instant::now() + PREVIEW_DEBOUNCE);
        }

        // Drain a requested lens computation onto a blocking thread.
        if let Some(lens) = app.pending_compute.take() {
            let tx = lens_tx.clone();
            let root = app.root.clone();
            tokio::task::spawn_blocking(move || {
                let _ = tx.send(collect::compute(lens, &root));
            });
        }

        if app.should_quit {
            break;
        }
        if redraw {
            terminal.draw(|frame| ui::render(frame, app))?;
        }
    }
    Ok(())
}

/// Apply one terminal event to the app, handling any external handoff a key
/// triggers. Returns whether the screen needs repainting — events the UI
/// ignores (key releases, bare mouse buttons, focus/paste) change nothing.
fn apply_event(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    event: Event,
) -> color_eyre::Result<bool> {
    let redraw = match event {
        Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
            app.handle_key(key);
            drain_pending_edit(terminal, app)?;
            drain_reader_handoff(terminal, app)?;
            drain_pending_capture(app);
            true
        }
        // Wheel scrolls and a left click act on the pane under the cursor; one
        // step per event. Motion is not tracked, so nothing floods the loop.
        Event::Mouse(m) => match m.kind {
            MouseEventKind::ScrollDown => {
                app.handle_scroll(m.column, m.row, 1);
                true
            }
            MouseEventKind::ScrollUp => {
                app.handle_scroll(m.column, m.row, -1);
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                app.handle_click(m.column, m.row);
                true
            }
            _ => false,
        },
        Event::Resize(_, _) => true,
        // Key releases, focus changes, paste: nothing to repaint for.
        _ => false,
    };
    Ok(redraw)
}

/// Apply a queued mouse-capture flip from the release-capture toggle.
fn drain_pending_capture(app: &mut App) {
    if let Some(on) = app.pending_capture.take() {
        let _ = tui::set_mouse_capture(on);
    }
}

/// Suspend the TUI to run `$EDITOR`/`$PAGER` if the reader queued a handoff. The
/// reader stays open across it, so we return to the reader on the way back.
fn drain_reader_handoff(terminal: &mut DefaultTerminal, app: &mut App) -> color_eyre::Result<()> {
    if let Some(handoff) = app.take_reader_handoff() {
        match handoff {
            Handoff::EditAtLine { path, line } => {
                tui::suspended(terminal, || editor::open_at_line(&path, line))?
            }
            Handoff::Pager { path } => tui::suspended(terminal, || pager::open(&path))?,
        }
    }
    Ok(())
}

/// Suspend the TUI to run `$EDITOR` if a key queued an edit. Drained per key so
/// two edits in one burst each suspend and restore correctly.
fn drain_pending_edit(terminal: &mut DefaultTerminal, app: &mut App) -> color_eyre::Result<()> {
    if let Some(path) = app.pending_edit.take() {
        tui::suspended(terminal, || editor::open(&path))?;
    }
    Ok(())
}
