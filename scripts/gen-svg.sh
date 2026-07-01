#!/usr/bin/env bash
# Regenerate assets/tree.svg — a preview screenshot of the tree-tui TUI,
# rendered with charmbracelet/freeze.
#
# tree-tui is a full-screen TUI (alternate screen + raw mode), so — unlike a
# plain CLI — freeze can't read its stdout directly: a TUI positions text with
# cursor-movement escapes that freeze's ANSI parser can't replay (and
# `freeze --execute` hits the same wall, collapsing everything onto huge lines).
# Instead we run the built binary in a detached tmux session, let it settle,
# drive a few keystrokes to reach a nice demo state, then scrape the finished
# screen with `tmux capture-pane -pe` — which flattens the grid into plain rows
# plus SGR colour — and pipe THAT into freeze.
#
# tree-tui paints in truecolour (Catppuccin Mocha), which freeze reproduces
# faithfully, so there is no palette remapping to do (contrast the 16-colour
# ANSI case); we just set the window background to Mocha's base (#1e1e2e).
#
# Idempotent: re-runs overwrite assets/tree.svg in place. The screenshot points
# tree at this repo, so its contents track the repo.
#
# Requirements:
#   - freeze (https://github.com/charmbracelet/freeze)
#       Provisioned by `mise install` (declared in mise.toml). To install it
#       manually, see https://github.com/charmbracelet/freeze#installation
#   - tmux — a system package (no prebuilt binaries, so not provisioned by mise).
#       e.g. `dnf install tmux`, `apt install tmux`, `brew install tmux`.
#
# Usage:
#   mise run svg
#   bash scripts/gen-svg.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Fixed terminal size: wide/tall enough for the multi-pane layout — the preview
# pane needs body width >= 100 and height >= 20 (see src/ui/mod.rs).
COLS=120
ROWS=34

# --- Prerequisites ---
for tool in freeze tmux; do
    if ! command -v "$tool" &>/dev/null; then
        echo "Error: $tool is not installed." >&2
        if [ "$tool" = "freeze" ]; then
            echo "  Install: mise install" >&2
            echo "  Or: https://github.com/charmbracelet/freeze#installation" >&2
        else
            echo "  Install tmux from your system package manager (e.g. 'dnf install tmux')." >&2
        fi
        exit 1
    fi
done

# --- Build the binary so the screenshot reflects current output ---
echo "Building tree (release)..."
cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml"
BIN="$REPO_ROOT/target/release/tree"

mkdir -p "$REPO_ROOT/assets"
OUT="$REPO_ROOT/assets/tree.svg"
TMP_DIR="$(mktemp -d)"
SESS="tree-svg-$$"
cleanup() {
    tmux kill-session -t "$SESS" 2>/dev/null || true
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

# --- Launch tree in a detached tmux session ---
# `-f /dev/null` ignores the user's tmux.conf for a reproducible pane; the two
# option sets make tmux keep truecolour so `capture-pane -e` emits 38;2;r;g;b
# (rather than quantising to 256 colours).
tmux -f /dev/null new-session -d -s "$SESS" -x "$COLS" -y "$ROWS"
tmux set-option -t "$SESS" -g default-terminal "tmux-256color"
tmux set-option -t "$SESS" -ga terminal-features ",*:RGB"
# Point tree at the repo itself: reproducible for any contributor, and — being a
# git repo — it lights up all four lenses. `exec` replaces the shell so the pane
# is pure TUI; COLORTERM asks the TUI for truecolour output.
tmux send-keys -t "$SESS" "cd '$REPO_ROOT' && COLORTERM=truecolor exec '$BIN' ." Enter

# --- Wait for the walk + first (code) lens layer to finish ---
# The footer hint ("q quit") shows only once loaded and not mid-compute, so its
# presence (with no "scanning…/computing…") means the frame is settled. Poll
# instead of a fixed sleep so it's robust on slow machines; give up after ~16s.
echo "Waiting for the TUI to settle..."
ready=0
for _ in $(seq 1 80); do
    sleep 0.2
    frame="$(tmux capture-pane -t "$SESS" -p 2>/dev/null || true)"
    if printf '%s' "$frame" | grep -q 'q quit' \
        && ! printf '%s' "$frame" | grep -qiE 'scanning|computing'; then
        ready=1
        break
    fi
done
if [ "$ready" -ne 1 ]; then
    echo "Error: the TUI did not finish loading within the timeout." >&2
    exit 1
fi

# --- Drive a short, deterministic demo state ---
# Jump to the top row (src/, the largest subtree under the code lens), expand
# it, then move onto its biggest source file so the preview pane renders
# syntax-highlighted code alongside the tree. Keys per src/action.rs & src/app.rs.
tmux send-keys -t "$SESS" g
sleep 0.15
tmux send-keys -t "$SESS" l
sleep 0.25
tmux send-keys -t "$SESS" j j
sleep 0.35

# --- Scrape the finished screen (plain rows + SGR colour) ---
FRAME_TXT="$TMP_DIR/frame.txt"
tmux capture-pane -t "$SESS" -p -e > "$FRAME_TXT"

# --- Render the SVG (no palette remap: tree-tui already emits truecolour) ---
echo "Rendering $OUT ..."
freeze \
    --output "$OUT" \
    --background "#1e1e2e" \
    --font.family "JetBrains Mono" \
    --window \
    --border.radius 8 \
    --padding 20 \
    --margin 20 \
    <"$FRAME_TXT"

echo "Wrote $OUT"
