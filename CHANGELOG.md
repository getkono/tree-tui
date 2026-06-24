# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
