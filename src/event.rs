//! The async event loop: input, walk completion, lazy lens computation, ticks.

use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::app::{App, Screen};
use crate::collect::{self, LayerResult};
use crate::{editor, scan, tui, ui};

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
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(120));

    terminal.draw(|frame| ui::render(frame, app))?;

    while !app.should_quit {
        let mut redraw = false;
        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    app.handle_key(key);
                    if let Some(path) = app.pending_open.take() {
                        tui::suspended(terminal, || editor::open(&path))?;
                    }
                    redraw = true;
                }
                Some(Ok(Event::Resize(_, _))) => redraw = true,
                Some(Ok(_)) => {}
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
