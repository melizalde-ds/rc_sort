# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-18

### Added
- A watch service that moves blocks added to `~/.bashrc`, `~/.bash_profile` and fish `config.fish` into `~/.bashrc.d/` and fish `conf.d/` and `functions/`.
- Sorting into tools, exports, aliases and functions. Blocks it can't place are asked about in the next interactive terminal.
- Duplicate blocks are removed instead of added twice.
- `rc-sort tidy`: a full-screen picker, or `--plain` prompts, for sorting existing files.
- `rc-sort install` and `uninstall` for the shell hook and the systemd user unit.
- `rc-sort completions` for bash, zsh, fish, elvish and PowerShell.
- A backup of every rewrite in `~/.local/state/rc-sort/backups/`.

[Unreleased]: https://github.com/miguel-elizalde/rc_sort/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/miguel-elizalde/rc_sort/releases/tag/v0.1.0
