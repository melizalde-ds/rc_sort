# bashrc-sort

Keeps `~/.bashrc` and `~/.bash_profile` at their Fedora defaults (`/etc/skel`).
When an installer (or you) adds lines, they move into `~/.bashrc.d/`
(`aliases.sh`, `exports.sh`, `functions.sh`, `tools.sh`) within a couple of seconds,
and the next terminal tells you. Blocks it can't place are left alone and asked
about in your next terminal. Design: [docs/design.md](docs/design.md).

## Install

```bash
cargo install --path .
bashrc-sort tidy      # optional: sort what's already in ~/.bashrc.d first
bashrc-sort install   # writes the hook + systemd unit, starts the service
```

`install` writes `~/.bashrc.d/zz-bashrc-sort.sh` and
`~/.config/systemd/user/bashrc-sort.service` (both embedded from `dist/`), then runs
`daemon-reload`, `enable` and `restart`. Run it again after upgrading.

## Use

- `bashrc-sort once`: one pass by hand.
- `bashrc-sort tidy [--plain] [file-or-dir]`: sort files already in `~/.bashrc.d` (default) or a
  file you name into the four files. In a terminal it opens a box below the prompt: every block,
  grouped by file, with where it goes and a preview; `Tab` jumps to the next unsure block,
  `t/e/a/f` send the selected block to tools/exports/aliases/functions, `k` keeps it in place
  (never asked again), `l` leaves it for later, `r` restores the suggestion, `y` asks to confirm,
  `q` quits without changes, `?` lists all keys. Colors follow your terminal theme; on a
  true-color terminal it uses its own palette (`BASHRC_SORT_THEME=ansi|dark|light` to choose). Piped, or with `--plain`, it prints the plan and asks about unsure
  blocks one by one, then `Apply? [y/N]`. Either way nothing changes, kept blocks included,
  until you confirm. Files emptied in `~/.bashrc.d` are deleted (backed up first). A file
  starting with `[[ $- == *i* ]] || return` keeps that meaning: its blocks are wrapped in
  `if [[ $- == *i* ]]; then … fi`. New terminals list files that still need sorting.
- Logs: `journalctl --user -u bashrc-sort`.
- Backups of every rewrite: `~/.local/state/bashrc-sort/backups/`.
- Uninstall: `bashrc-sort uninstall` stops and disables the service and removes the hook
  and unit. Your `~/.bashrc.d` files and the backups stay. Then `cargo uninstall bashrc-sort`.
