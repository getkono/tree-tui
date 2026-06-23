//! The async event loop: input, walk completion, lazy lens computation, ticks.

use std::time::{Duration, Instant};

use crossterm::event::EventStream;
use futures::{FutureExt, StreamExt};
use ratatui::DefaultTerminal;
use tokio::sync::{mpsc, oneshot};

use crate::app::{App, OpenMode, Screen};
use crate::batch::{self, Effect};
use crate::collect::{self, LayerResult};
use crate::scan::ScanOutcome;
use crate::{editor, pager, scan, tui, ui, watch};

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

    terminal.draw(|frame| ui::render(frame, app))?;

    while !app.should_quit {
        let mut redraw = false;
        tokio::select! {
            maybe_event = events.next() => {
                // Handle the first event, then drain everything else that is
                // immediately ready into one burst. Coalescing collapses wheel
                // and move runs so a flood becomes a single net change and a
                // single redraw (issue #22).
                let first = match maybe_event {
                    Some(Ok(ev)) => ev,
                    Some(Err(err)) => return Err(err.into()),
                    None => {
                        app.should_quit = true;
                        continue;
                    }
                };
                let mut burst = vec![first];
                while burst.len() < batch::MAX_BATCH {
                    match events.next().now_or_never() {
                        Some(Some(Ok(ev))) => burst.push(ev),
                        Some(Some(Err(err))) => return Err(err.into()),
                        Some(None) => {
                            app.should_quit = true;
                            break;
                        }
                        // Nothing more is ready right now: stop draining.
                        None => break,
                    }
                }
                for effect in batch::coalesce(burst) {
                    apply_effect(terminal, app, effect)?;
                    if app.should_quit {
                        break;
                    }
                }
                redraw = true;
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

/// Apply one coalesced [`Effect`] to the app, handling any external handoff it
/// triggers. The caller redraws once after the whole batch.
fn apply_effect(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    effect: Effect,
) -> color_eyre::Result<()> {
    match effect {
        Effect::Key(key) => {
            app.handle_key(key);
            drain_pending_open(terminal, app)?;
        }
        // Mouse handling lights up with the focusable preview pane (issue #23);
        // until mouse capture is enabled these effects never arrive.
        Effect::Scroll { .. } | Effect::MouseMoved { .. } => {}
        // The resize itself needs nothing; the caller's redraw repaints.
        Effect::Resize => {}
    }
    Ok(())
}

/// Suspend the TUI to run `$PAGER`/`$EDITOR` if a key queued an open. Drained
/// per key so two opens in one burst each suspend and restore correctly.
fn drain_pending_open(terminal: &mut DefaultTerminal, app: &mut App) -> color_eyre::Result<()> {
    if let Some((path, mode)) = app.pending_open.take() {
        match mode {
            OpenMode::View => tui::suspended(terminal, || pager::open(&path))?,
            OpenMode::Edit => tui::suspended(terminal, || editor::open(&path))?,
        }
    }
    Ok(())
}
