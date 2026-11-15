//! `bashrc-sort tidy [path]`: sorts files already in `~/.bashrc.d` (or a file or dir you
//! name) into aliases.sh / exports.sh / functions.sh / tools.sh. Interactive: shows the plan,
//! asks about unsure blocks, and changes nothing until you answer `y`.

use crate::blocks::{split_blocks, Block};
use crate::classify::{classify, leaves_file, Category};
use crate::prompt::{ask, Answer};
use crate::sort::{block_name, dup_in, is_category_file, place, rc_files, tool_clash, without, HOOK};
use crate::state::{atomic_write, backup, hash, load_kept, lock, Paths, State, WATCHED};
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
pub fn scan_text(text: &str) -> Scan {
    let lines: Vec<&str> = text.lines().collect();
    let first_code = lines.iter().position(|l| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with('#')
    });
    let guard = first_code.filter(|&i| is_interactive_guard(lines[i]));
    let lines_in: Vec<usize> = (0..lines.len()).filter(|&i| Some(i) != guard).collect();
    let blocks = split_blocks(text, &lines_in);
    let problem = blocks.iter().find(|b| leaves_file(b)).map(|b| {
        format!("it has a `return`/`exit` outside a function ({}); sort it by hand", block_name(b))
    });
    Scan { blocks, guard, problem }
}

/// `[[ $- == *i* ]] || return` or `[[ $- != *i* ]] && return` (quotes, `return 0`, `;` allowed).
fn is_interactive_guard(line: &str) -> bool {
    let t: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.trim_end_matches(';').trim_end();
    let t = t.strip_suffix(" 0").unwrap_or(t);
    let t = t.replace("\"$-\"", "$-");
    t == "[[ $- == *i* ]] || return" || t == "[[ $- != *i* ]] && return"
}

/// The block as it must read once it leaves a guarded file.
fn wrap_interactive(text: &str) -> String {
    // Indenting would change a heredoc's content; leave those as they are.
    let body = if text.contains("<<") {
        text.to_string()
    } else {
        text.lines()
            .map(|l| if l.trim().is_empty() { String::new() } else { format!("    {l}") })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!("if [[ $- == *i* ]]; then\n{body}\nfi")
}

/// Names of `~/.bashrc.d` files that still hold blocks to sort (kept blocks don't count).
/// Used by the shell hook, so it reads only and never fails loudly.
pub fn unsorted(p: &Paths) -> Result<Vec<String>> {
    let kept = load_kept(p)?;
    Ok(rc_files(p)?
        .into_iter()
        .filter(|(n, _)| !is_category_file(n))
        .filter(|(_, t)| {
            let s = scan_text(t);
            s.problem.is_some() || s.blocks.iter().any(|b| !kept.contains(&hash(&b.text)))
        })
        .map(|(n, _)| n)
        .collect())
}

struct Src {
    path: PathBuf,
    /// `~/…` or the absolute path, for messages.
    shown: String,
    /// File name, if the file lives in `~/.bashrc.d` (dup checks skip it; may be deleted).
    rc_name: Option<String>,
    text: String,
    scan: Scan,
}

enum Act {
    Move(Category),
    Dup(String),
    Leave,
}

struct Item {
    src: usize,
    block: Block,
    /// What lands in the target (wrapped if the source had an interactive guard).
    text: String,
    name: String,
    act: Act,
}

pub fn run_tidy(p: &Paths, target: Option<PathBuf>, input: &mut impl BufRead, out: &mut impl Write) -> Result<()> {
    let srcs = collect(p, target, out)?;
    let mut kept = load_kept(p)?;
    let new_kept_start = kept.len();
    let mut items: Vec<Item> = Vec::new();
    let mut asks: Vec<(usize, String)> = Vec::new();
    let files = rc_files(p)?;

    for (si, s) in srcs.iter().enumerate() {
        if let Some(why) = &s.scan.problem {
            writeln!(out, "bashrc-sort: left {} alone: {why}", s.shown)?;
            continue;
        }
        for b in &s.scan.blocks {
            if kept.contains(&hash(&b.text)) {
                continue;
            }
            let text = if s.scan.guard.is_some() { wrap_interactive(&b.text) } else { b.text.clone() };
            let skip = s.rc_name.as_deref();
            let (act, note) = if let Some(other) = dup_in(&files, &text, skip) {
                (Act::Dup(other.to_string()), None)
            } else {
                match classify(b) {
                    Some(Category::Tools) => match tool_clash(&files, &text, skip) {
                        Some((other, tool)) => (Act::Leave, Some(format!("{other} already has a {tool} block"))),
                        None => (Act::Move(Category::Tools), None),
                    },
                    Some(cat) => (Act::Move(cat), None),
                    None => (Act::Leave, Some(String::new())),
                }
            };
            if let Some(n) = note {
                asks.push((items.len(), n));
            }
            items.push(Item { src: si, block: b.clone(), text, name: block_name(b), act });
        }
    }

    let sure: Vec<&Item> = items.iter().filter(|i| !matches!(i.act, Act::Leave)).collect();
    if !sure.is_empty() {
        writeln!(out, "bashrc-sort tidy will:")?;
        for i in &sure {
            writeln!(out, "  {}", describe(&srcs[i.src], i))?;
        }
    }
    for (idx, note) in asks {
        let (block, s) = (items[idx].block.clone(), &srcs[items[idx].src]);
        let header = if note.is_empty() {
            format!("bashrc-sort: unsure where this block from {} goes:", s.shown)
        } else {
            format!("bashrc-sort: {note}; this one from {} may be a duplicate:", s.shown)
        };
        match ask(&header, &block.text, input, out)? {
            Answer::Move(cat) if block.balanced => {
                items[idx].act = Act::Move(cat);
                writeln!(out, "  {}", describe(s, &items[idx]))?;
            }
            Answer::Move(_) => writeln!(out, "  left in {}: it is half of an if/loop", s.shown)?,
            Answer::Keep => {
                kept.insert(hash(&block.text));
            }
            Answer::Later => {}
        }
    }

    let todo = items.iter().any(|i| !matches!(i.act, Act::Leave));
    let kept_changed = kept.len() != new_kept_start;
    if !todo {
        if kept_changed {
            let _lock = lock(p)?;
            save_kept(p, &kept)?;
        }
        writeln!(out, "bashrc-sort: nothing to tidy")?;
        return Ok(());
    }
    write!(out, "Apply? [y/N]: ")?;
    out.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    if !line.trim().eq_ignore_ascii_case("y") {
        writeln!(out, "bashrc-sort: nothing changed")?;
        return Ok(());
    }

    let _lock = lock(p)?;
    for (si, s) in srcs.iter().enumerate() {
        let mine: Vec<&Item> = items.iter().filter(|i| i.src == si).collect();
        if !mine.iter().any(|i| !matches!(i.act, Act::Leave)) {
            continue;
        }
        if fs::read_to_string(&s.path).ok().as_deref() != Some(s.text.as_str()) {
            writeln!(out, "bashrc-sort: {} changed while you were answering; run tidy again", s.shown)?;
            continue;
        }
        backup(p, &s.shown.trim_start_matches("~/").replace('/', "_"), &s.text)?;
        let mut gone: Vec<&Block> = Vec::new();
        for i in &mine {
            match &i.act {
                Act::Move(cat) => {
                    let msg = place(p, &s.shown, &i.name, &i.text, *cat, s.rc_name.as_deref())?;
                    writeln!(out, "{msg}")?;
                }
                Act::Dup(other) => writeln!(
                    out,
                    "bashrc-sort: removed duplicate {} block from {} (already in {other})",
                    i.name, s.shown
                )?,
                Act::Leave => continue,
            }
            gone.push(&i.block);
        }
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
        if rest.trim().is_empty() && s.rc_name.is_some() && !is_link {
            fs::remove_file(&s.path).with_context(|| format!("removing {}", s.path.display()))?;
            writeln!(out, "bashrc-sort: removed {} (empty now)", s.shown)?;
        } else {
            atomic_write(&s.path, &rest)?;
        }
    }
    if kept_changed {
        save_kept(p, &kept)?;
    }
    writeln!(out, "bashrc-sort: done; backups in {}", p.state_dir().join("backups").display())?;
    writeln!(out, "bashrc-sort: open shells keep the old setup until you run: source ~/.bashrc")?;
    Ok(())
}

fn describe(s: &Src, i: &Item) -> String {
    let guard = if s.scan.guard.is_some() { " (interactive only)" } else { "" };
    match &i.act {
        Act::Move(cat) => format!("{}: {} → {}{guard}", s.shown, i.name, cat.file_name()),
        Act::Dup(other) => format!("{}: {} is already in {other}, drop it", s.shown, i.name),
        Act::Leave => format!("{}: {} stays", s.shown, i.name),
    }
}

/// Adds `kept` to the saved state. Caller holds the lock.
fn save_kept(p: &Paths, kept: &HashSet<u64>) -> Result<()> {
    let mut s = State::load(p)?;
    s.kept.extend(kept.iter().copied());
    s.save(p)
}

/// The files to sort: `target` (a file or a dir of files), default `~/.bashrc.d`.
fn collect(p: &Paths, target: Option<PathBuf>, out: &mut impl Write) -> Result<Vec<Src>> {
    let home = fs::canonicalize(&p.home).unwrap_or_else(|_| p.home.clone());
    let rc = fs::canonicalize(p.rc_dir()).ok();
    let explicit = target.is_some();
    let root = target.unwrap_or_else(|| p.rc_dir());
    let meta = match fs::metadata(&root) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => return Ok(Vec::new()),
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
    let mut srcs = Vec::new();
    for path in candidates {
        // Resolve the directory, not the file: a symlinked rc file still counts as in ~/.bashrc.d.
        let real = real_path(&path)?;
        let shown = match real.strip_prefix(&home) {
            Ok(rel) => format!("~/{}", rel.display()),
            Err(_) => real.display().to_string(),
        };
        let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let in_rc = rc.as_deref().is_some_and(|r| real.parent() == Some(r));
        if WATCHED.iter().any(|w| real == home.join(w)) {
            if !meta.is_dir() {
                bail!("{shown} is kept at its default by the service; use `bashrc-sort once` for it");
            }
            continue;
        }
        if in_rc && (is_category_file(&name) || name == HOOK || name.ends_with(".bashrc-sort.tmp")) {
            if !meta.is_dir() {
                writeln!(out, "bashrc-sort: {shown} is already one of the sorted files")?;
            }
            continue;
        }
        let Ok(text) = fs::read_to_string(&real) else {
            writeln!(out, "bashrc-sort: skipped {shown}: not a text file")?;
            continue;
        };
        let scan = scan_text(&text);
        srcs.push(Src { path: real, shown, rc_name: in_rc.then_some(name), text, scan });
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
        assert!(is_interactive_guard("[[ $- == *i* ]] || return"));
        assert!(is_interactive_guard("[[ \"$-\" != *i* ]] && return 0;"));
        assert!(!is_interactive_guard("[ -z \"$PS1\" ] && return"));
    }

    #[test]
    fn guarded_file_scans_without_the_guard() {
        let s = scan_text("# prompt\n[[ $- == *i* ]] || return\neval \"$(starship init bash)\"\n");
        assert_eq!(s.guard, Some(1));
        assert!(s.problem.is_none());
        assert_eq!(s.blocks.len(), 1);
        assert_eq!(s.blocks[0].text, "eval \"$(starship init bash)\"");
    }

    #[test]
    fn return_mid_file_is_a_problem() {
        let s = scan_text("export A=1\n[[ $- == *i* ]] || return\nalias x=y\n");
        assert_eq!(s.guard, None);
        assert!(s.problem.is_some());
    }

    #[test]
    fn wrap_indents_and_keeps_heredocs() {
        assert_eq!(wrap_interactive("a\n\nb"), "if [[ $- == *i* ]]; then\n    a\n\n    b\nfi");
        assert_eq!(wrap_interactive("cat <<EOF\nx\nEOF"), "if [[ $- == *i* ]]; then\ncat <<EOF\nx\nEOF\nfi");
    }
}
