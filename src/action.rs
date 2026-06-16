//! Keyboard actions and the key → action mapping.
//!
//! Mapping is kept separate from the `App::update` reducer so the dispatch
//! table stays legible and the reducer stays focused on state transitions.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A semantic action produced from a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    Up,
    Down,
    First,
    Last,
    PageUp,
    PageDown,
    /// Expand a collapsed dir, or descend into an expanded one.
    Expand,
    /// Activate the selection: expand/descend a dir, or open a file in $EDITOR.
    Open,
    /// Collapse an expanded dir, or move to the parent.
    Collapse,
    /// Toggle expansion of the selected dir.
    Toggle,
    ExpandAll,
    CollapseAll,
    CycleSort,
    ReverseSort,
    /// Switch to the next available lens.
    CycleLens,
    /// Jump directly to a lens by 1-based index (digit keys).
    JumpLens(u8),
    /// Toggle hiding rows that are zero under the active lens.
    ToggleZeros,
}

/// Translate a key press into an [`Action`].
pub fn map_key(key: KeyEvent) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('d') if ctrl => Action::PageDown,
        KeyCode::Char('u') if ctrl => Action::PageUp,
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::Char('k') | KeyCode::Up => Action::Up,
        KeyCode::Char('g') | KeyCode::Home => Action::First,
        KeyCode::Char('G') | KeyCode::End => Action::Last,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::Char('l') | KeyCode::Right => Action::Expand,
        KeyCode::Enter => Action::Open,
        KeyCode::Char('h') | KeyCode::Left => Action::Collapse,
        KeyCode::Char(' ') => Action::Toggle,
        KeyCode::Char('E') => Action::ExpandAll,
        KeyCode::Char('C') => Action::CollapseAll,
        KeyCode::Char('s') => Action::CycleSort,
        KeyCode::Char('r') => Action::ReverseSort,
        KeyCode::Char('m') => Action::CycleLens,
        KeyCode::Char(c @ '1'..='9') => Action::JumpLens(c as u8 - b'0'),
        KeyCode::Char('z') => Action::ToggleZeros,
        _ => Action::None,
    }
}
