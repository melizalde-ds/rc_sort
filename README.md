# rc-sort

Keeps `~/.bashrc` and `~/.bash_profile` at their Fedora defaults (`/etc/skel`).
When an installer (or you) adds lines, they move into `~/.bashrc.d/`
(`aliases.sh`, `exports.sh`, `functions.sh`, `tools.sh`) within a couple of seconds,
and the next terminal tells you. Blocks it can't place are left alone and asked
about in your next terminal. Design: [docs/design.md](docs/design.md).

Bash is only sorted where the default `/etc/skel/.bashrc` loads `~/.bashrc.d` (Fedora does).
Where it doesn't (Arch, CachyOS), `~/.bashrc` is left alone and `install` says so.

Fish works the same way for `~/.config/fish/config.fish`. Its default is
`/etc/skel/.config/fish/config.fish`, or fish's own first-run template where the distro ships
none. Added blocks move to `conf.d/aliases.fish`, `conf.d/exports.fish` and `conf.d/tools.fish`,
and each function to its own `functions/<name>.fish`. A block that defines an alias or function
the distro config also defines (CachyOS's `cachyos-config.fish`) is asked about instead: that
config loads after `conf.d`, so it would win. `install` adds `conf.d/zz-rc-sort.fish` when
`~/.config/fish` exists. Files a fish plugin manager (fisher, oh-my-fish) writes into `conf.d`
show up as not sorted; keep them (`k` in `tidy`), since the manager rewrites them on update.

## Install

Linux only: needs a systemd user session. `rc-sort install` expects the binary at
`~/.cargo/bin/rc-sort`, so it won't work with a custom `CARGO_HOME` or `--root`.

```bash
cargo install --locked rc-sort
rc-sort tidy      # optional: sort what's already in ~/.bashrc.d first
rc-sort install   # writes the hook + systemd unit, starts the service
```

`install` writes `~/.bashrc.d/zz-rc-sort.sh` and
`~/.config/systemd/user/rc-sort.service` (both embedded from `dist/`), then runs
`daemon-reload`, `enable` and `restart`. Run it again after upgrading.

## Use

- `rc-sort once`: one pass by hand.
- `rc-sort completions <bash|zsh|fish|elvish|powershell>`: print a tab-completion script, e.g.
  `rc-sort completions fish > ~/.config/fish/completions/rc-sort.fish`.
- `rc-sort tidy [--plain] [file-or-dir]`: sort files already in `~/.bashrc.d` and `~/.config/fish/conf.d` (default) or a
  file you name into the four files. In a terminal it opens a box below the prompt: every block,
  grouped by file, with where it goes and a preview; `Tab` jumps to the next unsure block,
  `t/e/a/f` send the selected block to tools/exports/aliases/functions, `k` keeps it in place
  (never asked again), `l` leaves it for later, `r` restores the suggestion, `y` asks to confirm,
  `q` quits without changes, `?` lists all keys. Colors follow your terminal theme. Piped, or with `--plain`, it prints the plan and asks about unsure
  blocks one by one, then `Apply? [y/N]`. Either way nothing changes, kept blocks included,
  until you confirm. Files emptied in `~/.bashrc.d` are deleted (backed up first). A file
  starting with `[[ $- == *i* ]] || return` keeps that meaning: its blocks are wrapped in
  `if [[ $- == *i* ]]; then … fi`. New terminals list files that still need sorting.
- Logs: `journalctl --user -u rc-sort`.
- Backups of every rewrite: `~/.local/state/rc-sort/backups/`.
- Uninstall: `rc-sort uninstall` stops and disables the service and removes the hook
  and unit. Your `~/.bashrc.d` files and the backups stay. Then `cargo uninstall rc-sort`.
