# bashrc-sort Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust systemd user service that keeps `~/.bashrc` and `~/.bash_profile` at their `/etc/skel` defaults by moving added blocks into `~/.bashrc.d/{aliases,exports,functions,tools}.sh`, and asks in the next terminal about blocks it can't classify.

**Architecture:** One crate, library + thin binary. Pure pipeline `diff → blocks → classify` with no I/O, then `sort` does the file work under a lock. Three subcommands: `watch` (service), `prompt` (shell hook), `once` (one pass). All paths come from a `Paths` struct so tests run against a temp `HOME` and temp skel dir.

**Tech Stack:** Rust (cargo 1.99, edition 2024), `notify` + `notify-debouncer-mini`, `similar`, `serde` + `serde_json`, `anyhow` for errors (with `.context()` naming the file). std `File::lock` for locking. No other crates.

**Spec:** `docs/design.md`

## Global Constraints

- Watched files: only `~/.bashrc` and `~/.bash_profile`. Baseline: `/etc/skel/.bashrc`, `/etc/skel/.bash_profile`, read at runtime.
- Only **added** lines are handled. Changed or deleted default lines are never moved or reported.
- Classification: first match wins (functions → aliases → tools → exports). Unsure only when no rule matches.
- Every rewrite of a watched file is backed up first to `~/.local/state/bashrc-sort/backups/<file>.<timestamp>`. Backups are never pruned.
- All writes are atomic (temp file in the same dir + rename).
- State dir `~/.local/state/bashrc-sort/`: `pending.json`, `messages`, `kept`, `backups/`, `lock`. Read-modify-write of state or rc files happens under an exclusive lock on `lock`.
- Move message format, exactly: `bashrc-sort: moved <name> block from ~/<file> to <target>.sh`.
- Prompt question, exactly: `[t]ools [e]xports [a]liases [f]unctions [k]eep in place`.
- Prompts only when the shell is interactive AND stdin is a TTY. `prompt` exits in <50 ms when nothing is pending. A missing binary must not break the shell.
- Never move or edit anything in `~/.bashrc.d/` except appending to the four target files (created if missing).
- Debounce ~1 s. One pass on startup.
- Install: `cargo install --path .` → `~/.cargo/bin/bashrc-sort`.

## Review Focus

1. **Edited default line** (e.g. user changes the `if [ -d ~/.bashrc.d ]` line, or the existing comment edits): must never count as added, or moving it out breaks the shell. → Task 1 test `changed_default_line_is_not_added`.
2. **Installer re-adds a tool you already have, with different text**: ask, don't append or drop. → Task 5 test `same_tool_different_text_is_asked`.
3. **Crash between appending to the target and rewriting the watched file**: next pass must clean the watched file without a second copy in the target. → Task 5 test `block_already_in_target_is_not_duplicated`.
4. **Unclosed construct** (`if` with no `fi`): must not be moved (it would break `tools.sh`). → Task 2 test `unclosed_if_is_unbalanced` + Task 3 test `unbalanced_block_is_unsure`.
5. **Pending block edited or removed before the user answers**: answering must not move stale text or error; drop it from pending. → Task 6 test `stale_pending_is_dropped`.
6. **Target file without a trailing newline**: appended block must not glue onto its last line. → Task 5 test `append_to_file_without_trailing_newline`.

---

## File Structure

```
Cargo.toml
src/lib.rs        pub mod declarations only
src/main.rs       arg dispatch: watch | prompt | once; builds Paths from $HOME + /etc/skel
src/diff.rs       added_lines(): which lines of the current file are additions
src/blocks.rs     split_blocks(): group added lines into Blocks
src/classify.rs   Category + classify()
src/state.rs      Paths, lock, atomic_write, backup, hash, State (pending/kept/messages)
src/sort.rs       run_once(), move_block(): the only code that edits rc files
src/prompt.rs     run_prompt(): shows messages, asks about pending blocks
src/watch.rs      run_watch(): inotify on $HOME, debounce, call run_once
dist/zz-bashrc-sort.sh       shell hook
dist/bashrc-sort.service     systemd user unit
tests/e2e.rs      temp-HOME end-to-end tests
```

Unit tests live in `#[cfg(test)] mod tests` inside each `src/*.rs`.

---

### Task 1: Scaffold + diff against default

**Files:**
- Create: `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/diff.rs`

**Interfaces:**
- Produces: `diff::added_lines(default: &str, current: &str) -> Vec<usize>`, 0-based indexes into `current.lines()`, ascending.

- [ ] **Step 1: Scaffold.** `cargo init --name bashrc-sort` (keep existing README/LICENSE). Add deps: `notify`, `notify-debouncer-mini`, `similar`, `serde` (feature `derive`), `serde_json`, latest versions compatible with each other. `main.rs` prints usage `bashrc-sort watch|prompt|once` and exits 2 for now.

- [ ] **Step 2: Write failing tests in `src/diff.rs`**

```rust
const SKEL: &str = "# .bash_profile\n\n# Get the aliases and functions\nif [ -f ~/.bashrc ]; then\n\t. ~/.bashrc\nfi\n\n# User specific environment and startup programs\n";

#[test] fn appended_lines_are_added() {
    let cur = format!("{SKEL}\n# bun\nexport BUN_INSTALL=\"$HOME/.bun\"\n");
    assert_eq!(added_lines(SKEL, &cur), vec![8, 9, 10]); // blank + 2 lines
}
#[test] fn identical_file_has_no_additions() { assert!(added_lines(SKEL, SKEL).is_empty()); }
#[test] fn changed_default_line_is_not_added() {
    let cur = SKEL.replace("if [ -f ~/.bashrc ]; then", "if [ -f \"$HOME/.bashrc\" ]; then");
    assert!(added_lines(SKEL, &cur).is_empty());
}
#[test] fn append_after_deleted_default_lines_is_added() {
    // real ~/.bash_profile: last 2 skel lines deleted, then installer appends
    let base: String = SKEL.lines().take(6).map(|l| format!("{l}\n")).collect();
    let cur = format!("{base}# bun\nexport PATH=\"$HOME/.bun/bin:$PATH\"\n");
    assert_eq!(added_lines(SKEL, &cur), vec![6, 7]);
}
```

- [ ] **Step 3: Run** `cargo test diff::` → FAIL (function missing).

- [ ] **Step 4: Implement `added_lines`.** Use `similar::TextDiff::from_lines`, walk `ops()`. `Insert` → all new lines are added. `Replace` → a new line is a *changed default line* (skip) if `similar::TextDiff::from_chars(old, new).ratio() >= 0.6` against any old line in that same op; otherwise added. `Equal`/`Delete` → nothing.

- [ ] **Step 5: Run** `cargo test diff::` → PASS.

- [ ] **Step 6: Commit** `git add -A && git commit -m "feat: scaffold crate and diff added lines against skel"`

---

### Task 2: Split added lines into blocks

**Files:**
- Create: `src/blocks.rs`

**Interfaces:**
- Consumes: output of `diff::added_lines`.
- Produces:
  ```rust
  pub struct Block { pub lines: Vec<usize>, pub text: String, pub balanced: bool }
  pub fn split_blocks(current: &str, added: &[usize]) -> Vec<Block>
  ```
  `lines` = indexes into `current.lines()`; `text` = those lines joined with `\n`, no trailing newline; `balanced` = construct depth is 0 at block end.

- [ ] **Step 1: Write failing tests** (all inputs are a file where every line is added, so `added = 0..n`):
  - `bun_block_with_leading_comment`: `"# bun\nexport BUN_INSTALL=...\nexport PATH=...\n"` → 1 block, 3 lines.
  - `blank_line_splits_blocks`: `"alias a=x\n\nalias b=y\n"` → 2 blocks.
  - `pnpm_markers_join_across_blank`: `"# pnpm\nexport PNPM_HOME=...\n\ncase \":$PATH:\" in\n  *\":$PNPM_HOME:\"*) ;;\n  *) export PATH=...;;\nesac\n# pnpm end\n"` → 1 block containing both markers.
  - `fnm_if_not_split`: `"FNM_PATH=...\nif [ -d \"$FNM_PATH\" ]; then\n\n  eval \"$(fnm env)\"\nfi\n"` → 1 block, `balanced == true`.
  - `function_body_not_split`: `"mkcd() {\n  mkdir -p \"$1\"\n\n  cd \"$1\"\n}\n"` → 1 block.
  - `unclosed_if_is_unbalanced`: `"if true; then\n  echo hi\n"` → 1 block, `balanced == false`.
  - `one_line_if_is_balanced`: `"if [ -f ~/x ]; then . ~/x; fi\n\nalias a=b\n"` → 2 blocks, first `balanced == true`.
  - `comment_only_block_dropped`: `"# just a note\n"` → 0 blocks.
  - `non_added_line_splits`: current `"alias a=x\nDEFAULT\nalias b=y\n"`, added `[0, 2]` → 2 blocks.

- [ ] **Step 2: Run** `cargo test blocks::` → FAIL.

- [ ] **Step 3: Implement `split_blocks`.** Walk added lines in order. A block ends at a blank line or a gap in indexes, **unless** construct depth > 0 or an open marker is pending. Depth: per line (comment lines ignored), split into words on whitespace and `;`; each word `if|case|for|while|until` +1, each `fi|esac|done` −1 (so one-line `if …; fi` nets 0); a line ending in `{` +1, a line that is `}` (trimmed) −1. Marker: a comment line `# X` opens; it closes on `# X end`; if no close is found, fall back to normal splitting (don't swallow the file). Blank lines inside a block are kept in `text`. Drop blocks whose non-blank lines are all comments.

- [ ] **Step 4: Run** `cargo test blocks::` → PASS.

- [ ] **Step 5: Commit** `git commit -am "feat: split added lines into blocks"`

---

### Task 3: Classify blocks

**Files:**
- Create: `src/classify.rs`

**Interfaces:**
- Consumes: `blocks::Block`.
- Produces:
  ```rust
  #[derive(Clone, Copy, PartialEq, Debug)]
  pub enum Category { Aliases, Exports, Functions, Tools }
  impl Category {
      pub fn file_name(self) -> &'static str;      // "aliases.sh", ...
      pub fn from_key(c: char) -> Option<Self>;   // 't','e','a','f'
  }
  pub fn classify(b: &Block) -> Option<Category>  // None = unsure
  pub fn tools_named(text: &str) -> Vec<&'static str> // KNOWN_TOOLS words in text, deduped
  ```

- [ ] **Step 1: Write failing tests** (build `Block` via `split_blocks` on the sample text):

| test | input | expected |
|---|---|---|
| `bun_is_tools` | bun block (Task 2) | `Some(Tools)` |
| `pnpm_is_tools` | pnpm block | `Some(Tools)` |
| `fnm_is_tools` | fnm block | `Some(Tools)` |
| `cargo_env_is_tools` | `. "$HOME/.cargo/env"` | `Some(Tools)` |
| `guarded_source_is_tools` | `[ -s "$HOME/.foo/init.sh" ] && \. "$HOME/.foo/init.sh"` | `Some(Tools)` |
| `alias_is_aliases` | `alias zz="ls"` | `Some(Aliases)` |
| `function_is_functions` | `mkcd` block | `Some(Functions)` |
| `function_keyword_is_functions` | `function hi {\n echo hi\n}` | `Some(Functions)` |
| `function_with_path_is_functions` | `f() { :; }\nexport PATH="$HOME/x:$PATH"` | `Some(Functions)` (first match wins) |
| `plain_export_is_exports` | `export EDITOR=nano` | `Some(Exports)` |
| `mixed_alias_export_is_unsure` | `alias a=b\nexport C=d` | `None` |
| `unknown_command_is_unsure` | `ssh-agent -s > /dev/null` | `None` |
| `unbalanced_block_is_unsure` | `if true; then\n eval "$(fnm env)"` | `None` |

Plus `tools_named_finds_words`: `tools_named(". \"$HOME/.cargo/env\"\nexport PNPM_HOME=x") == ["cargo"]` (`PNPM_HOME` is one word, not `pnpm`; match is case-sensitive on whole words).

- [ ] **Step 2: Run** `cargo test classify::` → FAIL.

- [ ] **Step 3: Implement `classify`.** `!balanced` → `None` first. Then rules from the spec in order over non-comment, non-blank trimmed lines, with plain string checks (no regex crate):
  1. any line: starts with `function ` or matches `name()` / `name ()` where name is `[A-Za-z_][A-Za-z0-9_:-]*`.
  2. all lines start with `alias `.
  3. any line: contains `PATH=` as an assignment of `PATH` itself (`PATH=` at start, or after `export `), contains `eval "$(` / `eval $(` / `` eval "` ``, sources a file (a word `.`, `\.` or `source` at line start or after `&&`, `||`, `;`), or any word (split on chars not in `[A-Za-z0-9_-]`) is in `KNOWN_TOOLS = ["cargo","rustup","fnm","nvm","bun","pnpm","deno","pyenv","conda","sdkman","zoxide","starship","direnv"]`.
  4. all lines start with `export ` and contain `=`.

- [ ] **Step 4: Run** `cargo test classify::` → PASS.

- [ ] **Step 5: Commit** `git commit -am "feat: classify blocks"`

---

### Task 4: State, paths, locking, atomic writes

**Files:**
- Create: `src/state.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct Paths { pub home: PathBuf, pub skel: PathBuf }
  impl Paths {
      pub fn rc_dir(&self) -> PathBuf;     // home/.bashrc.d
      pub fn state_dir(&self) -> PathBuf;  // home/.local/state/bashrc-sort
  }
  pub const WATCHED: [&str; 2] = [".bashrc", ".bash_profile"];
  pub fn lock(p: &Paths) -> anyhow::Result<File>;              // creates state dir; File::lock(); unlocks on drop
  pub fn atomic_write(path: &Path, content: &str) -> anyhow::Result<()>; // resolve symlinks first (so a dotfiles symlink survives), then <path>.bashrc-sort.tmp + rename, keeps file mode
  pub fn backup(p: &Paths, file: &str, content: &str) -> anyhow::Result<()>; // backups/<file>.<unix_secs>[.<n>]
  pub fn hash(text: &str) -> u64;   // FNV-1a 64 over lines with trailing whitespace trimmed, joined by '\n', outer blank lines stripped
  #[derive(Serialize, Deserialize, Clone)]
  pub struct Pending { pub hash: u64, pub file: String, pub text: String, pub note: String } // note: "" or why it's unsure
  #[derive(Default)]
  pub struct State { pub pending: Vec<Pending>, pub kept: HashSet<u64>, pub messages: Vec<String> }
  impl State { pub fn load(p: &Paths) -> anyhow::Result<State>; pub fn save(&self, p: &Paths) -> anyhow::Result<()>; }
  ```
  Files: `pending.json` (JSON array), `kept` (one decimal hash per line), `messages` (one message per line). Missing files = empty. Caller holds the lock around load→save.

- [ ] **Step 1: Write failing tests** (temp home: `std::env::temp_dir().join(format!("bashrc-sort-test-{}-{name}", std::process::id()))`, wiped at start of each test):
  - `hash_ignores_trailing_whitespace_and_outer_blanks`: `hash("\nalias a=b  \n") == hash("alias a=b")`.
  - `state_roundtrip`: save State with 1 pending, 1 kept, 2 messages → load → equal.
  - `load_missing_is_empty`.
  - `atomic_write_replaces_and_keeps_mode`: write file with mode 0o600, `atomic_write` → content new, mode still 0o600, no `.tmp` left.
  - `atomic_write_through_symlink`: `link -> real`; `atomic_write(link)` → `link` is still a symlink, `real` has new content.
  - `backup_twice_same_second_keeps_both`: two backups → 2 files in `backups/`.

- [ ] **Step 2: Run** `cargo test state::` → FAIL.

- [ ] **Step 3: Implement** per the interface above.

- [ ] **Step 4: Run** `cargo test state::` → PASS.

- [ ] **Step 5: Commit** `git commit -am "feat: state files, lock, atomic write, backups"`

---

### Task 5: Sort pass (`once`) + end-to-end tests

**Files:**
- Create: `src/sort.rs`, `tests/e2e.rs`, `tests/fixtures/skel/.bashrc`, `tests/fixtures/skel/.bash_profile` (copies of `/etc/skel`)
- Modify: `src/main.rs` (wire `once`), `src/lib.rs` (add each new module as tasks land)

**Interfaces:**
- Consumes: `added_lines`, `split_blocks`, `classify`, `Category`, everything in `state`.
- Produces:
  ```rust
  pub fn run_once(p: &Paths) -> anyhow::Result<()>;   // takes the lock itself
  /// Re-reads `file`, finds the block whose hash == `h`, moves it to `cat`.
  /// Ok(false) if the block is no longer there. Caller holds the lock.
  pub fn move_block(p: &Paths, state: &mut State, file: &str, h: u64, cat: Category) -> anyhow::Result<bool>;
  pub fn block_name(b: &Block) -> String; // first line if comment: text after '#', trimmed; else first line, max 40 chars
  ```

- [ ] **Step 1: Write failing tests in `tests/e2e.rs`.** Helper `setup(name) -> Paths` writes the real-world skel files (copy contents of `/etc/skel/.bashrc` and `/etc/skel/.bash_profile` into `tests/fixtures/skel/` once and use those, so tests don't depend on the host), copies them as `home/.bashrc` / `home/.bash_profile`, creates `home/.bashrc.d/tools.sh` with `"# tools\n"`.
  - `bun_moves_to_tools_once`: append bun block to `.bash_profile`; `run_once` twice → `.bash_profile` == skel; `tools.sh` contains the block exactly once; `messages` == `["bashrc-sort: moved bun block from ~/.bash_profile to tools.sh"]`; 1 file in `backups/`.
  - `same_tool_different_text_is_asked`: `tools.sh` has the user's real bun block (with `case`); append the installer's bun block (`# bun` / `export BUN_INSTALL="$HOME/.bun"` / `export PATH=$BUN_INSTALL/bin:$PATH`) to `.bash_profile` → `.bash_profile` unchanged, `tools.sh` unchanged, 1 pending with `note == "tools.sh already has a bun block"`.
  - `block_already_in_target_is_not_duplicated`: put bun block in `tools.sh` and in `.bash_profile` → after `run_once`, `.bash_profile` == skel, block count in `tools.sh` == 1.
  - `append_to_file_without_trailing_newline`: `aliases.sh` = `"alias a=b"` (no `\n`); append `alias zz="ls"` to `.bashrc` → `aliases.sh` == `"alias a=b\n\nalias zz=\"ls\"\n"`.
  - `missing_target_is_created`: no `functions.sh`; add `mkcd` → file created.
  - `unsure_stays_and_is_pending_once`: append `ssh-agent -s > /dev/null` → `.bashrc` unchanged; `run_once` twice → `pending.len() == 1`; no message.
  - `kept_block_is_ignored`: put its hash in `kept` → no pending after `run_once`.
  - `missing_watched_file_is_skipped`: delete `.bash_profile` → `run_once` is `Ok`, file not created.
  - `edited_comment_lines_untouched`: `.bashrc` with the user's real changed comment (`# User specific config: aliases, exports, functions, tools`) → nothing moved, nothing pending.

- [ ] **Step 2: Run** `cargo test --test e2e` → FAIL.

- [ ] **Step 3: Implement `run_once`.** Under the lock, for each of `WATCHED`: skip if watched or skel file missing (log to stderr). Compute blocks. For each block: hash in `kept` or `pending` → skip; `classify` → `None` → push `Pending` (note `""`); `Some(cat)` → if the exact block is already in the target, just remove it from the watched file; else if `tools_named(block)` overlaps `tools_named(target)` → push `Pending` with note `"<target>.sh already has a <tool> block"`; else move. `move_block` (used when the user answers) skips the overlap check: the user decided. Do all sure moves for one file as: backup watched (once per pass per file) → append each block to its target (skip if the hash-normalized block text is already a substring of the normalized target; ensure target ends with `\n` before appending `"\n" + text + "\n"`) via `atomic_write` → remove moved block lines **plus added blank lines directly before each moved block** from the watched file and `atomic_write` it → push message. Save state. Target writes happen before the watched-file write, so a crash leaves a duplicate that the dedup clears, never a lost line.

- [ ] **Step 4: Implement `move_block`** with the same per-block logic (used by `prompt`), and wire `once` in `main.rs` (`Paths { home: $HOME, skel: "/etc/skel" }`; error → stderr, exit 1).

- [ ] **Step 5: Run** `cargo test` → all PASS.

- [ ] **Step 6: Commit** `git add -A && git commit -m "feat: sort pass and once subcommand"`

---

### Task 6: `prompt` subcommand + shell hook

**Files:**
- Create: `src/prompt.rs`, `dist/zz-bashrc-sort.sh`
- Modify: `src/main.rs` (wire `prompt`)

**Interfaces:**
- Consumes: `State`, `lock`, `move_block`, `Category::from_key`.
- Produces: `pub fn run_prompt(p: &Paths, input: &mut impl BufRead, out: &mut impl Write) -> anyhow::Result<()>`

- [ ] **Step 1: Write failing tests** (in `tests/e2e.rs`, input via `Cursor`, output `Vec<u8>`):
  - `prints_messages_once`: 1 message → output contains it; second call prints nothing.
  - `answer_t_moves_pending`: pending `ssh-agent` block, input `"t\n"` → block in `tools.sh`, gone from `.bashrc`, pending empty, output contains `bashrc-sort: moved ssh-agent -s > /dev/null block from ~/.bashrc to tools.sh` (printed right away, not queued).
  - `answer_k_keeps`: input `"k\n"` → hash in `kept`, pending empty, `.bashrc` unchanged.
  - `empty_answer_stays_pending`: input `"\n"` and input `""` (EOF, stands in for Ctrl-C) → still pending.
  - `stale_pending_is_dropped`: pending entry whose text is no longer in `.bashrc`, input `"t\n"` → pending empty, `tools.sh` unchanged, no error.
  - `nothing_pending_is_silent`: no state files → empty output.

- [ ] **Step 2: Run** `cargo test --test e2e prompt` → FAIL.

- [ ] **Step 3: Implement `run_prompt`.** Fast path: if `messages` and `pending.json` don't exist or are empty, return before taking the lock. Lock → load → print + clear messages → save → **unlock** (don't hold the lock while waiting for the user). For each pending block: print `bashrc-sort: unsure where this block from ~/<file> goes:` (or, when `note` is set, `bashrc-sort: <note>; this one from ~/<file> may be a duplicate:`), the block indented 4 spaces, then `[t]ools [e]xports [a]liases [f]unctions [k]eep in place: `. Read one line. Then lock → load → apply (`move_block` / add to `kept`; on `Ok(false)` just drop it) → remove from pending → save → unlock. Unknown key → re-ask once, then leave pending.

- [ ] **Step 4: Write `dist/zz-bashrc-sort.sh`**

```bash
# bashrc-sort hook: show moves, ask about unsure blocks. Interactive TTY only.
if [[ $- == *i* && -t 0 && -x "$HOME/.cargo/bin/bashrc-sort" ]]; then
    # trap so Ctrl-C in the prompt doesn't abort the rest of shell startup
    trap ':' INT
    "$HOME/.cargo/bin/bashrc-sort" prompt
    trap - INT
fi
```

- [ ] **Step 5: Run** `cargo test` → PASS. Then `cargo build --release && time ./target/release/bashrc-sort prompt` with empty state → `real` < 0.05s.

- [ ] **Step 6: Commit** `git add -A && git commit -m "feat: prompt subcommand and shell hook"`

---

### Task 7: `watch` subcommand, systemd unit, install

**Files:**
- Create: `src/watch.rs`, `dist/bashrc-sort.service`
- Modify: `src/main.rs` (wire `watch`), `README.md` (install steps)

**Interfaces:**
- Consumes: `run_once`, `WATCHED`.
- Produces: `pub fn run_watch(p: &Paths) -> anyhow::Result<()>` (returns only on watcher error).

- [ ] **Step 1: Implement `run_watch`.** `run_once` at startup. `notify_debouncer_mini::new_debouncer(Duration::from_secs(1), tx)`, watch `p.home` `NonRecursive`. On each debounced batch: if any event path's `file_name()` is in `WATCHED`, call `run_once`. Errors from `run_once` → `eprintln!` and continue (journal shows them); never exit on a pass error. Loop guard is built in: after a pass the file has no new blocks, and pending/kept are skipped by hash.

- [ ] **Step 2: Write `dist/bashrc-sort.service`**

```ini
[Unit]
Description=Keep ~/.bashrc and ~/.bash_profile sorted into ~/.bashrc.d

[Service]
ExecStart=%h/.cargo/bin/bashrc-sort watch
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
```

- [ ] **Step 3: Write install steps in `README.md`**

```bash
cargo install --path .
cp dist/zz-bashrc-sort.sh ~/.bashrc.d/
mkdir -p ~/.config/systemd/user && cp dist/bashrc-sort.service ~/.config/systemd/user/
systemctl --user daemon-reload && systemctl --user enable --now bashrc-sort
```

- [ ] **Step 4: Install and verify** (run the README steps), then:
  - `systemctl --user is-active bashrc-sort` → `active`
  - `echo 'alias zz="ls"' >> ~/.bashrc; sleep 3` → `diff /etc/skel/.bashrc ~/.bashrc` shows only the original comment line; `tail -2 ~/.bashrc.d/aliases.sh` shows `alias zz="ls"`; `cat ~/.local/state/bashrc-sort/messages` shows the move message.
  - `bash -ic exit` → prints the message once; running again prints nothing.
  - `bash -lc 'echo ok' </dev/null` → prints only `ok` (no prompt in non-interactive).
  - Append `ssh-agent -s > /dev/null` to `~/.bashrc`, open a terminal, press Ctrl-C at the question → shell still starts, `type cargo` still works, block still pending. Answer `k` next terminal, then remove the line from `~/.bashrc` by hand.
  - `journalctl --user -u bashrc-sort -n 20` → no errors, only one pass for the echo.
  - Remove `alias zz="ls"` from `aliases.sh` by hand afterwards.

- [ ] **Step 5: Commit** `git add -A && git commit -m "feat: watch service, systemd unit, install docs"`
