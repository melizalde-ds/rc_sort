# bashrc-sort

Keeps `~/.bashrc` and `~/.bash_profile` at their Fedora defaults (`/etc/skel`).
When an installer (or you) adds lines, they move into `~/.bashrc.d/`
(`aliases.sh`, `exports.sh`, `functions.sh`, `tools.sh`) within a couple of seconds,
and the next terminal tells you. Blocks it can't place are left alone and asked
about in your next terminal. Design: [docs/design.md](docs/design.md).

## Install

```bash
cargo install --path .
cp dist/zz-bashrc-sort.sh ~/.bashrc.d/
mkdir -p ~/.config/systemd/user && cp dist/bashrc-sort.service ~/.config/systemd/user/
systemctl --user daemon-reload && systemctl --user enable --now bashrc-sort
```

## Use

- `bashrc-sort once`: one pass by hand.
- `bashrc-sort tidy [file-or-dir]`: sort files already in `~/.bashrc.d` (default) or a file
  you name into the four files. Shows the plan, asks about unsure blocks, changes nothing
  until you answer `y`. Files emptied in `~/.bashrc.d` are deleted (backed up first). A file
  starting with `[[ $- == *i* ]] || return` keeps that meaning: its blocks are wrapped in
  `if [[ $- == *i* ]]; then … fi`. New terminals list files that still need sorting.
- Logs: `journalctl --user -u bashrc-sort`.
- Backups of every rewrite: `~/.local/state/bashrc-sort/backups/`.
- Uninstall: `systemctl --user disable --now bashrc-sort`, then delete
  `~/.config/systemd/user/bashrc-sort.service`, `~/.bashrc.d/zz-bashrc-sort.sh`
  and `~/.cargo/bin/bashrc-sort`.
