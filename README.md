# rc-sort

[![crates.io](https://img.shields.io/crates/v/rc-sort.svg)](https://crates.io/crates/rc-sort)
[![CI](https://github.com/miguel-elizalde/rc_sort/actions/workflows/ci.yml/badge.svg)](https://github.com/miguel-elizalde/rc_sort/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Installers keep appending to your `~/.bashrc`. rc-sort moves what they add into
`~/.bashrc.d/` within a couple of seconds, so your rc files stay at the distro default
and your setup stays organized. Fish is supported too.

## Example

An installer appends this to `~/.bash_profile`:

```bash
# bun
export BUN_INSTALL="$HOME/.bun"
export PATH=$BUN_INSTALL/bin:$PATH
```

A moment later the block is in `~/.bashrc.d/tools.sh`, `~/.bash_profile` is back to
its default, and your next terminal says:

```
rc-sort: moved bun block from ~/.bash_profile to tools.sh
```

If rc-sort can't tell where a block goes, it leaves it alone and asks you in your next terminal.

## Install

Needs Linux with a systemd user session.

```bash
cargo install --locked rc-sort
rc-sort tidy      # optional: sort what's already in ~/.bashrc.d
rc-sort install   # adds the shell hook and starts the service
```

`rc-sort install` must run from `~/.cargo/bin/rc-sort`, so a custom `CARGO_HOME` or
`--root` won't work. Run it again after every upgrade.

## Where blocks go

| Block | Bash (`~/.bashrc.d/`) | Fish (`~/.config/fish/`) |
|---|---|---|
| Functions | `functions.sh` | `functions/<name>.fish` |
| Aliases | `aliases.sh` | `conf.d/aliases.fish` |
| PATH changes, tool setup, `eval "$(...)"`, `source` | `tools.sh` | `conf.d/tools.fish` |
| Other exports | `exports.sh` | `conf.d/exports.fish` |
| Anything else | asked about | asked about |

If a block is already in the target file, the copy is removed instead of added twice.

## Commands

| Command | What it does |
|---|---|
| `rc-sort install` | Writes the shell hook and systemd user unit, then starts the service |
| `rc-sort uninstall` | Stops the service and removes the hook and unit. Your files and backups stay |
| `rc-sort tidy [--plain] [path]` | Sorts files already in `~/.bashrc.d` and `conf.d`, or a file you name. Opens a picker in a terminal (press `?` for keys) |
| `rc-sort once` | Runs one sorting pass by hand |
| `rc-sort completions <shell>` | Prints a completion script for bash, zsh, fish, elvish or powershell |

The service runs `watch`, and the shell hook runs `prompt`. You don't need to run either one yourself.

## Safety

- Every rewrite is backed up first, to `~/.local/state/rc-sort/backups/`.
- It only asks questions in interactive terminals, never in scripts or `ssh host cmd`.
- A block with a syntax error, or one that is only half of an `if` or a loop, stays where it is.
- `tidy` changes nothing until you confirm.

## Supported setups

- **Bash:** only where `/etc/skel/.bashrc` loads `~/.bashrc.d` (Fedora does). On distros where it doesn't (Arch, CachyOS), `~/.bashrc` is left alone and `install` tells you.
- **Fish:** `config.fish` is compared to `/etc/skel/.config/fish/config.fish`, or to fish's first-run template if the distro ships none. A block that redefines something your distro's fish config also defines (CachyOS) is asked about instead, because that config loads later and would win.
- **Fish plugin managers** (fisher, oh-my-fish): their `conf.d` files show up as not sorted. Keep them with `k` in `tidy`.

## Logs and uninstall

```bash
journalctl --user -u rc-sort
rc-sort uninstall && cargo uninstall rc-sort
```

## License

MIT
