# bashrc-sort: design

## Why

Installers (bun, pnpm, cargo, fnm…) append setup blocks to `~/.bashrc` and
`~/.bash_profile`. Over time this caused duplicates (fnm ran 3×, bun added twice,
two pnpm blocks) and an unorganized config. The user reorganized everything into
`~/.bashrc.d/` and wants it to stay organized automatically, without having to
notice and clean up after every installer.

Success: an installer (or the user) edits a watched file; within a couple of seconds
the new lines live in the right `~/.bashrc.d/` file, the watched file is back to its
default, and the next terminal says what happened.

## Current state (2026-11-05)

- Fedora 43-ish on WSL2, systemd enabled (`systemctl --user` works), cargo 1.99.
- `~/.bashrc` = Fedora's `/etc/skel/.bashrc` (only a comment differs). It sources
  every file in `~/.bashrc.d/` in glob (alphabetical) order.
- `~/.bash_profile` = `/etc/skel/.bash_profile` (only comments differ).
- `~/.bashrc.d/`: `aliases.sh`, `exports.sh`, `functions.sh`, `tools.sh`.
- Edit aliases (`editrc`, `edittools`, …) live in `aliases.sh`; `apply` = `source ~/.bashrc`.

## Goals

1. Watch `~/.bashrc` and `~/.bash_profile` for any change, from installers or the user.
2. Find lines that are not part of the default (`/etc/skel/` copy of that file).
3. Group them into blocks and classify each block as aliases / exports / functions / tools.
4. Sure blocks: move to the target file in `~/.bashrc.d/` and remove from the watched file.
5. Unsure blocks: leave in place, and ask the user in their next interactive terminal.
6. Every action shows once as a one-line message in the next interactive terminal.
7. Runs as a systemd user service, started at login, restarted on failure.

## Non-goals

- Reorganizing the files inside `~/.bashrc.d/` (dedup, reordering, rewriting).
- Watching `~/.profile`, zsh/fish configs, or any other file.
- Reloading already-open shells. New terminals pick up the change; open ones use `apply`.
- Configurable rules, categories, or a config file.

## Anti-goals (failure even if it "works")

- A shell that fails to start, or loses PATH/tool setup, because of a move.
- Losing any line. Every rewrite is backed up first.
- Prompting in non-interactive shells (scripts, `ssh cmd`, scp, editors and agents
  that run `bash -lc`). Prompts only when the shell is interactive AND stdin is a TTY.
- Edit loops: the service's own writes must not trigger more work.
- Asking about the same block again after the user answered.

## Behavior

### Diff against the default
- Baseline for each file: `/etc/skel/.bashrc` and `/etc/skel/.bash_profile`, read at runtime.
- Only **added** lines are handled. Changed or deleted default lines (including the
  current comment differences) are left alone and never reported. Default lines the user
  moved elsewhere in the file are not additions either.
  Builder decides the diff method; a line-based LCS diff is enough.

### Blocks
- A block is a run of added lines with no blank line inside.
- Leading comment lines belong to the block below them (bun: `# bun` + 2 lines).
- Marker pairs like `# pnpm` … `# pnpm end` form one block even across blank lines.
- A multi-line construct (`if … fi`, `case … esac`, function body `{ … }`) is never split.

### Classification (first match wins)
1. Defines a function (`name() {` or `function name`) → `functions.sh`
2. Every non-comment line is an `alias` → `aliases.sh`
3. Changes PATH, runs a command (`$(…)` or backticks, e.g. `eval "$(…)"`), sources a file (`.` / `source`), or names a known
   tool (cargo, rustup, fnm, nvm, bun, pnpm, deno, pyenv, conda, sdkman, zoxide,
   starship, direnv) → `tools.sh`
4. Every non-comment line is `export VAR=…` (no PATH) → `exports.sh`
5. Anything else (no rule above matches) → **unsure**. Decided 2026-11-05: first match
   wins, so a block matching several rules goes to the first one.

(Builder's call: exact regexes and known-tool list.)

### Moving a sure block
1. Back up the watched file to `~/.local/state/bashrc-sort/backups/<file>.<timestamp>`.
2. If the identical block (ignoring surrounding whitespace) is already in the target
   file, do not append it again (this is how duplicates died).
   Otherwise append it to the target, preceded by a blank line.
   If a tools block is not identical but names a known tool that tools.sh already names
   (e.g. the bun installer's block vs the user's hand-edited `# bun` block), treat it
   as **unsure** instead (decided 2026-11-05): leave it and ask, so neither a silent
   duplicate nor a silent drop happens.
3. Remove the block from the watched file. All writes are atomic (temp file + rename).
4. Record message: `bashrc-sort: moved bun block from ~/.bash_profile to tools.sh`.

### Unsure blocks
- Left in the watched file, untouched. Recorded as pending (keyed by content hash) so it
  is asked once, not on every event.
- Next interactive terminal shows the block and asks:
  `[t]ools [e]xports [a]liases [f]unctions [k]eep in place`.
  t/e/a/f → move it as above. k → remember the hash; never ask about it again.
  A block that is half of an if/loop is never moved; it is kept and the user is told.
- Empty input / Ctrl-C → stays pending; asked again next terminal.

### Shell hook
- `~/.bashrc.d/zz-bashrc-sort.sh` (sorts last). If interactive and TTY, runs
  `bashrc-sort prompt`, which prints pending messages, asks about pending blocks,
  then clears what it showed. It must exit fast (<50 ms) when there is nothing pending,
  and a missing binary must not break the shell.
- The service never moves its own hook file or anything in `~/.bashrc.d/`.

### Watching
- inotify on the home directory, filtered to the two file names (installers and
  editors often replace the file via rename, which a file-level watch would miss).
- Debounce ~1 s so a multi-step installer write is handled once.
- Loop guard: after the service rewrites a file, the file has no added lines, so the
  next event is a no-op. Pending/kept blocks are skipped by hash.
- Also runs one pass on startup, so edits made while the service was down get sorted.

## Decided by the user

- Rust. systemd user service.
- Any change gets sorted, installer or user.
- Unsure → ask in the next terminal; sure → move + one-line message.
- Project lives in `~/dev/bashrc-sort`.

## Claude's calls (user can override)

- Baseline = `/etc/skel` copies, not a snapshot.
- `~/.profile` is not watched (bash ignores it when `~/.bash_profile` exists).
- PATH changes count as tools, even for things like dotnet that currently sit in
  `exports.sh`. Existing files are not touched.
- State in `~/.local/state/bashrc-sort/`: `pending.json`, `messages`, `kept`, `backups/`.
  Backups are never pruned automatically.
- Single binary with subcommands: `watch` (service), `prompt` (hook), `once` (one pass,
  for testing). Installed with `cargo install --path .` to `~/.cargo/bin`.
- Crates: `notify` (+ debouncer), `similar` (diff), `serde_json`, `anyhow` (errors; user request 2026-11-05). Nothing else unless needed.
- Service and hook both touch the state files: every write is atomic, and both take
  an exclusive lock on `~/.local/state/bashrc-sort/lock` while reading-modifying-writing.
- A missing target file in `~/.bashrc.d/` is created.
- Editing with nano while the service runs: the line moves out on save; if nano saves
  again with the line, dedup (step 2 of Moving) removes it again without a duplicate.
- The hook file and systemd unit are written by the install steps in the plan,
  not by the binary.

## Left to the builder

Diff algorithm details, regexes, crate versions, unit file contents, error message
wording, code layout.

## Testing

- Unit tests: block splitting and classification with real samples (bun, pnpm block
  with `# pnpm end`, fnm with `if … fi`, cargo `. "$HOME/.cargo/env"`, an alias, a
  function, a plain export, an ambiguous block).
- End-to-end in a temp `HOME` with `once`: append the bun block to `.bash_profile`,
  assert it lands in `tools.sh` once and `.bash_profile` is back to default.
- Manual: after install, `echo 'alias zz="ls"' >> ~/.bashrc`, open a new terminal,
  see the message, check `aliases.sh`.
