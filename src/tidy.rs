//! `rc-sort tidy [path]`: sorts files already in `~/.bashrc.d` and fish's `conf.d` (or a file or dir you
//! name) into aliases.sh / exports.sh / functions.sh / tools.sh. Interactive: shows the plan,
//! asks about unsure blocks, and changes nothing until you answer `y`.

use crate::blocks::{split_blocks, Block};
use crate::classify::{leaves_file, tools_named, Category};
use crate::prompt::{ask, Answer};
use crate::sort::{block_name, dup_in, place, rc_files, tool_clash, without, FISH_HOOK, HOOK};
use crate::state::{atomic_write, backup, hash, load_kept, lock, Paths, State};
use crate::shell::Shell;
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// How a file splits into blocks for sorting.
pub struct Scan {
    pub blocks: Vec<Block>,
    /// Line index of a leading `[[ $- == *i* ]] || return`: everything below it is
    /// interactive-only, so each moved block gets wrapped in `if [[ $- == *i* ]]`.
    pub guard: Option<usize>,
    /// Why the file can't be sorted automatically.
    pub problem: Option<String>,
}

/// Every line of `text` counts (unlike the watched files, there is no default to diff against).
pub fn scan_text(shell: Shell, text: &str) -> Scan {
    let lines: Vec<&str> = text.lines().collect();
    let first_code = lines.iter().position(|l| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with('#')
    });
    let guard = first_code.filter(|&i| is_interactive_guard(shell, lines[i]));
    let lines_in: Vec<usize> = (0..lines.len()).filter(|&i| Some(i) != guard).collect();
    let blocks = split_blocks(shell, text, &lines_in);
    let problem = blocks.iter().find(|b| leaves_file(b)).map(|b| {
        format!("it has a `return`/`exit` outside a function ({}); sort it by hand", block_name(b))
    });
    Scan { blocks, guard, problem }
}

/// `[[ $- == *i* ]] || return` or `[[ $- != *i* ]] && return` (quotes, `return 0`, `;` allowed).
fn is_interactive_guard(shell: Shell, line: &str) -> bool {
    let t: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.trim_end_matches(';').trim_end();
    let t = t.strip_suffix(" 0").unwrap_or(t);
    let t = t.replace("\"$-\"", "$-").replace("--is-interactive", "is-interactive");
    match shell {
        Shell::Bash => t == "[[ $- == *i* ]] || return" || t == "[[ $- != *i* ]] && return",
        Shell::Fish => t == "status is-interactive; or return" || t == "status is-interactive || return",
    }
}

/// Names of `~/.bashrc.d` files that still hold blocks to sort (kept blocks don't count).
/// Used by the shell hook, so it reads only and never fails loudly.
pub fn unsorted(p: &Paths) -> Result<Vec<String>> {
    let kept = load_kept(p)?;
    let mut names = Vec::new();
    for shell in Shell::ALL {
        let dir = shell.rc_dir(p);
        let shown = dir.strip_prefix(&p.home).unwrap_or(&dir).display().to_string();
        for (n, t) in rc_files(p, shell)? {
            if shell.is_category_file(&n) {
                continue;
            }
            let s = scan_text(shell, &t);
            if s.problem.is_some() || s.blocks.iter().any(|b| !kept.contains(&hash(&b.text))) {
                names.push(format!("~/{shown}/{n}"));
            }
        }
    }
    Ok(names)
}

fn tidy_dir(p: &Paths, shell: Shell) -> PathBuf {
    match shell {
        Shell::Bash => shell.rc_dir(p),
        Shell::Fish => shell.rc_dir(p).join("conf.d"),
    }
}

pub(crate) struct Src {
    pub(crate) shell: Shell,
    path: PathBuf,
    /// `~/…` or the absolute path, for messages.
    pub(crate) shown: String,
    /// File name, if the file lives in `~/.bashrc.d` (dup checks skip it; may be deleted).
    rc_name: Option<String>,
    text: String,
    scan: Scan,
}

#[cfg_attr(not(feature = "tui"), allow(dead_code))]
impl Src {
    /// Starts with `[[ $- == *i* ]] || return`: moved blocks get wrapped.
    pub(crate) fn guarded(&self) -> bool {
        self.scan.guard.is_some()
    }
}

/// What happens to a block on apply.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Act {
    Move(Category),
    /// Already in this `~/.bashrc.d` file: drop it from the source.
    Dup(String),
    /// Stays in the source and is never asked about again.
    Keep,
    /// Stays in the source for now.
    Leave,
}

impl Act {
    fn changes_file(&self) -> bool {
        matches!(self, Act::Move(_) | Act::Dup(_))
    }
}

pub(crate) struct Item {
    pub(crate) src: usize,
    pub(crate) block: Block,
    /// What lands in the target (wrapped if the source had an interactive guard).
    pub(crate) text: String,
    pub(crate) name: String,
    pub(crate) act: Act,
    /// `None`: sure. `Some("")`: no rule matched. `Some(why)`: may be a duplicate.
    pub(crate) note: Option<String>,
    /// What tidy chose before anyone answered (`act` starts equal to it).
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    pub(crate) suggested: Act,
    /// For a same-tool clash: the file and the block there that names the tool.
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    pub(crate) clash: Option<(String, String)>,
}

/// Everything tidy would do, before anyone has answered. Both front ends (plain prompts
/// and the TUI) only change `items[..].act`, then hand the plan to `apply`.
pub(crate) struct Plan {
    pub(crate) srcs: Vec<Src>,
    pub(crate) items: Vec<Item>,
    /// `~/.bashrc.d` files as they were when planning, for duplicate checks and previews.
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    pub(crate) files: Vec<(String, String)>,
}

impl Plan {
    pub(crate) fn has_changes(&self) -> bool {
        self.items.iter().any(|i| i.act != Act::Leave)
    }

    /// What source file `si` becomes with the current choices: the text left over, and
    /// whether the file is deleted instead (emptied, in `~/.bashrc.d`, not a symlink).
    pub(crate) fn leftover(&self, si: usize) -> (String, bool) {
        let s = &self.srcs[si];
        let mut gone: Vec<&Block> =
            self.items.iter().filter(|i| i.src == si && i.act.changes_file()).map(|i| &i.block).collect();
        // Blocks that stay in the file (kept, unanswered, or never listed because kept earlier).
        let stays = s.scan.blocks.len() > gone.len();
        let guard_block;
        if let (Some(g), false) = (s.scan.guard, stays) {
            let text = s.text.lines().nth(g).unwrap_or("").to_string();
            guard_block = Block { lines: vec![g], text, balanced: true };
            gone.push(&guard_block);
        }
        let rest = without(&s.text, &gone);
        let is_link = fs::symlink_metadata(&s.path).is_ok_and(|m| m.file_type().is_symlink());
        let delete = rest.trim().is_empty() && s.rc_name.is_some() && !is_link;
        (rest, delete)
    }

    /// Blocks that stay in source file `si` (kept earlier ones included).
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    pub(crate) fn remaining(&self, si: usize) -> usize {
        let gone = self.items.iter().filter(|i| i.src == si && i.act.changes_file()).count();
        self.srcs[si].scan.blocks.len().saturating_sub(gone)
    }

    pub(crate) fn target(&self, i: &Item, cat: Category) -> String {
        self.srcs[i.src].shell.target_name(cat, &i.text).unwrap_or_else(|| cat.stem().to_string())
    }

    pub(crate) fn can_move(&self, idx: usize, cat: Category) -> bool {
        let i = &self.items[idx];
        i.block.balanced && self.srcs[i.src].shell.target_name(cat, &i.text).is_some()
    }

    pub(crate) fn describe(&self, i: &Item) -> String {
        let s = &self.srcs[i.src];
        let guard = if s.scan.guard.is_some() { " (interactive only)" } else { "" };
        match &i.act {
            Act::Move(cat) => format!("{}: {} → {}{guard}", s.shown, i.name, self.target(i, *cat)),
            Act::Dup(other) => format!("{}: {} is already in {other}, remove this copy", s.shown, i.name),
            Act::Keep => format!("{}: {} stays, not asked again", s.shown, i.name),
            Act::Leave => format!("{}: {} stays", s.shown, i.name),
        }
    }
}

/// Scans and classifies; changes nothing. Files it can't sort are reported to `out`.
pub(crate) fn plan(p: &Paths, target: Option<PathBuf>, out: &mut impl Write) -> Result<Plan> {
    let srcs = collect(p, target, out)?;
    let kept = load_kept(p)?;
    let mut files = Vec::new();
    for shell in Shell::ALL {
        files.extend(rc_files(p, shell)?);
    }
    let mut items: Vec<Item> = Vec::new();
    for (si, s) in srcs.iter().enumerate() {
        let files = rc_files(p, s.shell)?;
        if let Some(why) = &s.scan.problem {
            writeln!(out, "rc-sort: left {} alone: {why}", s.shown)?;
            continue;
        }
        for b in &s.scan.blocks {
            if kept.contains(&hash(&b.text)) {
                continue;
            }
            let cat = s.shell.classify(b);
            let autoload = s.shell == Shell::Fish && cat == Some(Category::Functions);
            let text = if s.scan.guard.is_some() && !autoload { s.shell.wrap_interactive(&b.text) } else { b.text.clone() };
            let skip = s.rc_name.as_deref();
            let mut clash = None;
            let (act, note) = if let Some(other) = dup_in(&files, &text, skip) {
                (Act::Dup(other.to_string()), None)
            } else if let Some(why) = s.shell.distro_clash(p, &b.text)? {
                (Act::Leave, Some(why))
            } else {
                match cat {
                    Some(Category::Tools) => match tool_clash(s.shell, &files, &text, skip) {
                        Some((other, tool)) => {
                            clash = block_naming(s.shell, &files, &other, tool).map(|t| (other.clone(), t));
                            (Act::Leave, Some(format!("{other} already has a {tool} block")))
                        }
                        None => (Act::Move(Category::Tools), None),
                    },
                    Some(cat) => match s.shell.target_name(cat, &text) {
                        Some(name) if name.starts_with("functions/") && s.shell.rc_dir(p).join(&name).exists() => {
                            (Act::Leave, Some(format!("{name} already exists")))
                        }
                        _ => (Act::Move(cat), None),
                    },
                    None => (Act::Leave, Some(String::new())),
                }
            };
            let suggested = act.clone();
            items.push(Item { src: si, block: b.clone(), text, name: block_name(b), act, note, suggested, clash });
        }
    }
    Ok(Plan { srcs, items, files })
}


/// The first block of `file` (one of `files`) that names `tool`.
fn block_naming(shell: Shell, files: &[(String, String)], file: &str, tool: &str) -> Option<String> {
    let (_, text) = files.iter().find(|(n, _)| n == file)?;
    let lines: Vec<usize> = (0..text.lines().count()).collect();
    split_blocks(shell, text, &lines).into_iter().find(|b| tools_named(&b.text).contains(&tool)).map(|b| b.text)
}

/// Plain front end: prints the plan, asks about each unsure block in order, then `Apply? [y/N]`.
pub fn run_tidy(p: &Paths, target: Option<PathBuf>, input: &mut impl BufRead, out: &mut impl Write) -> Result<()> {
    let mut plan = plan(p, target, out)?;
    let sure: Vec<String> = plan.items.iter().filter(|i| i.note.is_none()).map(|i| plan.describe(i)).collect();
    if !sure.is_empty() {
        writeln!(out, "rc-sort tidy will:")?;
        for line in &sure {
            writeln!(out, "  {line}")?;
        }
    }
    for idx in 0..plan.items.len() {
        let Some(note) = plan.items[idx].note.clone() else { continue };
        let shown = plan.srcs[plan.items[idx].src].shown.clone();
        let header = if note.is_empty() {
            format!("rc-sort: unsure where this block from {shown} goes:")
        } else {
            format!("rc-sort: {note}; this one from {shown} may be a duplicate:")
        };
        let block = plan.items[idx].block.clone();
        match ask(&header, &block.text, input, out)? {
            Answer::Move(cat) if plan.can_move(idx, cat) => {
                plan.items[idx].act = Act::Move(cat);
                writeln!(out, "  {}", plan.describe(&plan.items[idx]))?;
            }
            Answer::Move(_) => writeln!(out, "  left in {shown}: it is half of an if/loop, or not one function")?,
            Answer::Keep => plan.items[idx].act = Act::Keep,
            Answer::Later => {}
        }
    }

    if !plan.has_changes() {
        writeln!(out, "rc-sort: nothing to tidy")?;
        return Ok(());
    }
    write!(out, "Apply? [y/N]: ")?;
    out.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    if !line.trim().eq_ignore_ascii_case("y") {
        writeln!(out, "rc-sort: nothing changed")?;
        return Ok(());
    }
    apply(p, &plan, out).map(|_| ())
}

/// Writes the plan: moves and drops blocks, deletes emptied files, saves kept blocks.
/// A source file that changed since planning is skipped; returns their `srcs` indexes.
pub(crate) fn apply(p: &Paths, plan: &Plan, out: &mut impl Write) -> Result<Vec<usize>> {
    let _lock = lock(p)?;
    let mut skipped = Vec::new();
    for (si, s) in plan.srcs.iter().enumerate() {
        let mine: Vec<&Item> = plan.items.iter().filter(|i| i.src == si).collect();
        if !mine.iter().any(|i| i.act.changes_file()) {
            continue;
        }
        if fs::read_to_string(&s.path).ok().as_deref() != Some(s.text.as_str()) {
            writeln!(out, "rc-sort: {} changed while you were answering; run tidy again", s.shown)?;
            skipped.push(si);
            continue;
        }
        backup(p, &s.shown.trim_start_matches("~/").replace('/', "_"), &s.text)?;
        for i in &mine {
            match &i.act {
                Act::Move(cat) => {
                    let msg = place(p, s.shell, &s.shown, &i.name, &i.text, *cat, s.rc_name.as_deref())?;
                    writeln!(out, "{msg}")?;
                }
                Act::Dup(other) => writeln!(
                    out,
                    "rc-sort: removed duplicate {} block from {} (already in {other})",
                    i.name, s.shown
                )?,
                Act::Keep | Act::Leave => {}
            }
        }
        let (rest, delete) = plan.leftover(si);
        if delete {
            fs::remove_file(&s.path).with_context(|| format!("removing {}", s.path.display()))?;
            writeln!(out, "rc-sort: removed {} (empty now)", s.shown)?;
        } else {
            atomic_write(&s.path, &rest)?;
        }
    }
    let kept: HashSet<u64> =
        plan.items.iter().filter(|i| i.act == Act::Keep).map(|i| hash(&i.block.text)).collect();
    if !kept.is_empty() {
        save_kept(p, &kept)?;
        writeln!(out, "rc-sort: kept {} block(s) in place; tidy won't ask about them again", kept.len())?;
    }
    writeln!(out, "rc-sort: done; backups in {}", p.state_dir().join("backups").display())?;
    let reload = |shell| match shell {
        Shell::Bash => "source ~/.bashrc",
        Shell::Fish => "exec fish",
    };
    let mut reloads: Vec<&str> = plan.srcs.iter().map(|s| reload(s.shell)).collect();
    reloads.dedup();
    writeln!(out, "rc-sort: open shells keep the old setup until you run: {}", reloads.join(" or "))?;
    Ok(skipped)
}

/// Adds `kept` to the saved state. Caller holds the lock.
fn save_kept(p: &Paths, kept: &HashSet<u64>) -> Result<()> {
    let mut s = State::load(p)?;
    s.kept.extend(kept.iter().copied());
    s.save(p)
}

/// The files to sort: `target` (a file or a dir of files), default `~/.bashrc.d` and fish's `conf.d`.
fn collect(p: &Paths, target: Option<PathBuf>, out: &mut impl Write) -> Result<Vec<Src>> {
    let home = fs::canonicalize(&p.home).unwrap_or_else(|_| p.home.clone());
    let rc_dirs: Vec<(Shell, PathBuf)> =
        Shell::ALL.iter().filter_map(|&s| fs::canonicalize(tidy_dir(p, s)).ok().map(|d| (s, d))).collect();
    let explicit = target.is_some();
    let roots = match target {
        Some(t) => vec![t],
        None => Shell::ALL.iter().map(|&s| tidy_dir(p, s)).collect(),
    };
    let mut srcs = Vec::new();
    for root in roots {
        let meta = match fs::metadata(&root) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {}", root.display())),
        };
        let candidates: Vec<PathBuf> = if meta.is_dir() {
            let mut v: Vec<PathBuf> = fs::read_dir(&root)
                .with_context(|| format!("reading {}", root.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file())
                .collect();
            v.sort();
            v
        } else {
            vec![root.clone()]
        };
        for path in candidates {
            // Resolve the directory, not the file: a symlinked rc file still counts as in its rc dir.
            let real = real_path(&path)?;
            let shown = match real.strip_prefix(&home) {
                Ok(rel) => format!("~/{}", rel.display()),
                Err(_) => real.display().to_string(),
            };
            let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let in_rc = rc_dirs.iter().find(|(_, d)| real.parent() == Some(d.as_path())).map(|(s, _)| *s);
            let shell = in_rc.unwrap_or(if name.ends_with(".fish") { Shell::Fish } else { Shell::Bash });
            let rc_name = in_rc.map(|s| if s == Shell::Fish { format!("conf.d/{name}") } else { name.clone() });
            if Shell::ALL.iter().flat_map(|s| s.watched()).any(|w| real == home.join(w)) {
                if !meta.is_dir() {
                    bail!("{shown} is kept at its default by the service; use `rc-sort once` for it");
                }
                continue;
            }
            if in_rc == Some(Shell::Fish) && !name.ends_with(".fish") {
                continue;
            }
            let ours = name == HOOK || name == FISH_HOOK || name.ends_with(".rc-sort.tmp");
            if let Some(rc) = &rc_name
                && (shell.is_category_file(rc) || ours)
            {
                if !meta.is_dir() {
                    writeln!(out, "rc-sort: {shown} is already one of the sorted files")?;
                }
                continue;
            }
            let Ok(text) = fs::read_to_string(&real) else {
                writeln!(out, "rc-sort: skipped {shown}: not a text file")?;
                continue;
            };
            let scan = scan_text(shell, &text);
            srcs.push(Src { shell, path: real, shown, rc_name, text, scan });
        }
    }
    Ok(srcs)
}

fn real_path(path: &Path) -> Result<PathBuf> {
    let parent = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let dir = fs::canonicalize(parent).with_context(|| format!("resolving {}", parent.display()))?;
    Ok(dir.join(path.file_name().context("path has no file name")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_forms() {
        assert!(is_interactive_guard(Shell::Bash, "[[ $- == *i* ]] || return"));
        assert!(is_interactive_guard(Shell::Bash, "[[ \"$-\" != *i* ]] && return 0;"));
        assert!(!is_interactive_guard(Shell::Bash, "[ -z \"$PS1\" ] && return"));
        assert!(is_interactive_guard(Shell::Fish, "status --is-interactive; or return"));
    }

    #[test]
    fn guarded_file_scans_without_the_guard() {
        let s = scan_text(Shell::Bash, "# prompt\n[[ $- == *i* ]] || return\neval \"$(starship init bash)\"\n");
        assert_eq!(s.guard, Some(1));
        assert!(s.problem.is_none());
        assert_eq!(s.blocks.len(), 1);
        assert_eq!(s.blocks[0].text, "eval \"$(starship init bash)\"");
    }

    #[test]
    fn return_mid_file_is_a_problem() {
        let s = scan_text(Shell::Bash, "export A=1\n[[ $- == *i* ]] || return\nalias x=y\n");
        assert_eq!(s.guard, None);
        assert!(s.problem.is_some());
    }

    #[test]
    fn wrap_indents_and_keeps_heredocs() {
        assert_eq!(Shell::Bash.wrap_interactive("a\n\nb"), "if [[ $- == *i* ]]; then\n    a\n\n    b\nfi");
        assert_eq!(Shell::Bash.wrap_interactive("cat <<EOF\nx\nEOF"), "if [[ $- == *i* ]]; then\ncat <<EOF\nx\nEOF\nfi");
    }
}
