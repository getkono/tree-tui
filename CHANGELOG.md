# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/getkono/tree-tui/compare/v0.2.1...v0.3.0) - 2026-09-21

### Added

- *(ui)* resize the tree and preview by dragging the divider
- *(tui)* track mouse motion while a button is held
- *(keys)* bind Tab to the preview pane
- *(tree)* carry files, size, commits, and share in every row

### Fixed

- *(collect)* harden the tracked-file union and pin it with tests
- *(collect)* show every file git tracks, dot-entries included
- *(app)* keep a divider drag out of the full-screen reader
- *(ui)* make the divider's round trip exact at any terminal width

### Other

- *(collect)* compare the merged findings with multiplicity
- *(collect)* hold the fan-out claim across runs, not on each one
- *(collect)* pin the Drop-time merge without a scheduler
- Merge pull request #56 from getkono/dependabot/cargo/karet-pdf-0.6.1
- Merge pull request #57 from getkono/dependabot/cargo/karet-core-0.6.1
- Merge pull request #58 from getkono/dependabot/cargo/karet-filetype-0.6.1
- Merge pull request #61 from getkono/dependabot/cargo/tokei-15.0.0
- *(collect)* assert the walk actually fans out
- *(collect)* benchmark the sequential and parallel walks
- *(collect)* put the parallel merge under test
- *(collect)* walk the filesystem in parallel
- *(collect)* ground the walk's behaviour with property tests
- split the crate into a library and a thin binary
- state the walk's actual file set
- *(ui)* [**breaking**] retire the detail panel

## [0.2.1](https://github.com/getkono/tree-tui/compare/v0.2.0...v0.2.1) - 2026-09-12

### Other

- *(readme)* recommend mise as the first install method

## [0.2.0](https://github.com/getkono/tree-tui/compare/v0.1.3...v0.2.0) - 2026-08-31

### Added

- *(tree)* show a file-type icon on every row
- *(fileview)* [**breaking**] migrate to karet 0.6 and own the file-view dispatch

### Fixed

- *(event)* arm the preview debounce against the layout the frame used
- *(preview)* skip the freshness check for note-only previews
- *(fileview)* key the Kitty placement on the document's file stamp
- *(preview)* keep scroll and page across a content refresh
- *(preview)* refresh the preview when the file changes on disk
- *(cli)* default --icons to the tier every font can render
- *(tui)* send the Kitty delete escape only to a Kitty terminal
- *(fileview)* redraw the terminal image after the screen is handed away
- *(reader)* page by the lines actually shown, and settle the viewport before titling it

### Other

- *(app)* cover the direction the NodeId key actually aliased in
- record the preview-freshness design
- *(app)* key the preview cache on the previewed path
- *(deps)* update all dependencies to latest
- *(deps)* bump gix from 0.85 to 0.87
- *(fileview)* say what the placement key proves, and fix the renamed links
- *(fileview)* stop retransmitting an unchanged terminal image
- describe the file view tree-tui now owns

## [0.1.3](https://github.com/getkono/tree-tui/compare/v0.1.2...v0.1.3) - 2026-07-02

### Added

- replace syntect/ratatui-image with karet-fileview for previews
- show HEAD commit hash left of the header LOC summary
- model exclusion as exclude + include exceptions
- add exclude filter to adjust aggregate stats ([#37](https://github.com/getkono/tree-tui/pull/37))

### Fixed

- sort directories before files in the name sort order
- advertise the r (reverse sort) key in the footer hint

### Other

- add a TUI preview to the README
- add gen-svg script and wire up the svg task

## [0.1.2](https://github.com/getkono/tree-tui/compare/v0.1.1...v0.1.2) - 2026-06-24

### Added

- open files in a full-screen in-TUI reader instead of $PAGER
- focusable, scrollable preview pane with focus-follows-mouse
- watch the filesystem and refresh on change
- view files in $PAGER on Enter, edit on Shift+Enter

### Fixed

- smooth navigation, granular wheel, and interact-to-focus

### Other

- Merge pull request #8 from getkono/dependabot/cargo/gix-0.85.0
- cache tree rows and add interactive click selection
- batch and coalesce input events for smooth scrolling
- Merge branch 'master' into feat/file-watching

## [0.1.1](https://github.com/getkono/tree-tui/compare/v0.1.0...v0.1.1) - 2026-06-23

### Added

- *(release)* distribute via Homebrew tap
- concatenate sole-subdirectory chains into one row
- add swappable lenses with lazy-computed metrics

## [0.1.0](https://github.com/getkono/tree-tui/releases/tag/v0.1.0) - 2026-06-16

### Added

- *(tui)* open the selected file in $EDITOR on Enter
- add responsive language column with percentages
- *(panels)* detail panel, help overlay, and name filter
- *(tui)* interactive code-stats tree
- *(cli)* strict CLI, -V build info, and file logging

### Other

- *(release)* add crates.io metadata, release-plz, and dependabot config
- rename tree-tui to tree in the README
- rename user-facing "tree-tui" to "tree"
- enforce Conventional Commits with convco
- document tree-tui usage, keybindings, and install
- *(deps)* add tokei, name binary tree, add install task
- initialize project
