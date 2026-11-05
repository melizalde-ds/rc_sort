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
- Logs: `journalctl --user -u bashrc-sort`.
- Backups of every rewrite: `~/.local/state/bashrc-sort/backups/`.
- Uninstall: `systemctl --user disable --now bashrc-sort`, then delete
  `~/.config/systemd/user/bashrc-sort.service`, `~/.bashrc.d/zz-bashrc-sort.sh`
  and `~/.cargo/bin/bashrc-sort`.
