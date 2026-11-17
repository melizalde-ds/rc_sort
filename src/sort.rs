use crate::blocks::{split_blocks, Block};
use crate::classify::{leaves_file, runs_a_command, tools_named, Category};
use crate::diff::added_lines;
use crate::state::{atomic_write, backup, hash, lock, normalize, read_opt, Paths, Pending, State};
use crate::shell::Shell;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;

/// The shell hook, installed into `~/.bashrc.d/`. Never sorted.
pub const HOOK: &str = "zz-rc-sort.sh";
pub const FISH_HOOK: &str = "zz-rc-sort.fish";

/// One pass over both watched files: move sure blocks, record unsure ones as pending.
pub fn run_once(p: &Paths) -> Result<()> {
    let _lock = lock(p)?;
    let mut state = State::load(p)?;
    for (shell, &file) in Shell::ALL.iter().flat_map(|&s| s.watched().iter().map(move |f| (s, f))) {
        let Some((current, blocks)) = scan(p, shell, file)? else { continue };
        // Blocks the user edited or removed by hand are no longer worth asking about.
        let live: HashSet<u64> = blocks.iter().map(|b| hash(&b.text)).collect();
        state.pending.retain(|x| x.file != file || live.contains(&x.hash));
        let mut done: Vec<&Block> = Vec::new();
        for b in &blocks {
            let h = hash(&b.text);
            if state.kept.contains(&h) || state.pending.iter().any(|x| x.hash == h) {
                continue;
            }
            if file == ".bash_profile" && runs_a_command(b) {
                let note = "~/.bash_profile runs it once per login; ~/.bashrc.d runs it in every shell";
                state.pending.push(pending(file, b, note.into()));
                continue;
            }
            let enclosure = enclosure(shell, &current, b);
            if enclosure == Enclosure::Other {
                state.pending.push(pending(file, b, "it is inside a block of the default file".into()));
                continue;
            }
            let Some(cat) = shell.classify(b) else {
                state.pending.push(pending(file, b, String::new()));
                continue;
            };
            if let Some(note) = shell.distro_clash(p, &b.text)? {
                state.pending.push(pending(file, b, note));
                continue;
            }
            let text = placed_text(shell, enclosure, cat, &b.text);
            let files = rc_files(p, shell)?;
            let new = dup_in(&files, &text, None).is_none();
            if cat == Category::Tools && new
                && let Some((other, tool)) = tool_clash(shell, &files, &text, None)
            {
                let note = format!("{other} already has a {tool} block");
                state.pending.push(pending(file, b, note));
                continue;
            }
            if let Some(name) = shell.target_name(cat, &b.text)
                && new
                && name.starts_with("functions/")
                && shell.rc_dir(p).join(&name).exists()
            {
                state.pending.push(pending(file, b, format!("{name} already exists")));
                continue;
            }
            if done.is_empty() {
                backup(p, file, &current)?;
            }
            state.messages.push(place(p, shell, &format!("~/{file}"), &block_name(b), &text, cat, None)?);
            done.push(b);
        }
        if !done.is_empty() {
            rewrite(p, file, &current, &done)?;
        }
    }
    state.save(p)
}

/// Re-reads `file`, finds the block whose hash == `h`, moves it to `cat` (no duplicate-tool check:
/// the user decided). Ok(false) if the block is no longer there, or is half of an if/loop
/// (moving it would break both files). Caller holds the lock.
pub fn move_block(p: &Paths, state: &mut State, file: &str, h: u64, cat: Category) -> Result<bool> {
    let Some(shell) = Shell::of(file) else { return Ok(false) };
    let Some((current, blocks)) = scan(p, shell, file)? else { return Ok(false) };
    let Some(b) = blocks.iter().find(|b| b.balanced && !leaves_file(b) && hash(&b.text) == h) else {
        return Ok(false);
    };
    if shell.target_name(cat, &b.text).is_none() {
        return Ok(false);
    }
    let text = placed_text(shell, enclosure(shell, &current, b), cat, &b.text);
    backup(p, file, &current)?;
    state.messages.push(place(p, shell, &format!("~/{file}"), &block_name(b), &text, cat, None)?);
    rewrite(p, file, &current, &[b])?;
    Ok(true)
}

#[derive(PartialEq, Clone, Copy)]
enum Enclosure {
    Top,
    Interactive,
    Other,
}

fn enclosure(shell: Shell, current: &str, block: &Block) -> Enclosure {
    let lines: Vec<&str> = current.lines().collect();
    let mut open: Vec<usize> = Vec::new();
    for (i, line) in lines.iter().enumerate().take(block.lines[0]) {
        let delta = shell.depth_delta(line.trim());
        open.extend(std::iter::repeat_n(i, delta.max(0) as usize));
        open.truncate(open.len().saturating_sub((-delta).max(0) as usize));
    }
    match open.as_slice() {
        [] => Enclosure::Top,
        [i] if shell.opens_interactive(lines[*i]) => Enclosure::Interactive,
        _ => Enclosure::Other,
    }
}

fn placed_text(shell: Shell, enclosure: Enclosure, cat: Category, text: &str) -> String {
    match enclosure {
        Enclosure::Top => text.to_string(),
        Enclosure::Interactive if !(shell == Shell::Fish && cat == Category::Functions) => {
            shell.wrap_interactive(&dedent(text))
        }
        _ => dedent(text),
    }
}

fn dedent(text: &str) -> String {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let cut = text.lines().filter(|l| !l.trim().is_empty()).map(indent).min().unwrap_or(0);
    text.lines().map(|l| l.get(cut..).unwrap_or("").trim_end()).collect::<Vec<_>>().join("\n")
}

/// Writes `file` without `blocks`, unless it changed since `current` was read: an installer
/// appended mid-pass, and writing would drop its lines. Its write queues another pass, and
/// that pass removes the blocks already copied as duplicates.
fn rewrite(p: &Paths, file: &str, current: &str, blocks: &[&Block]) -> Result<()> {
    let path = p.home.join(file);
    if read_opt(&path)?.as_deref() != Some(current) {
        eprintln!("rc-sort: ~/{file} changed during the pass, leaving it for the next one");
        return Ok(());
    }
    atomic_write(&path, &without(current, blocks))
}

/// Short name for messages: the leading comment's text, else the first line (max 40 chars).
pub fn block_name(b: &Block) -> String {
    let first = b.text.lines().next().unwrap_or("").trim();
    match first.strip_prefix('#') {
        Some(c) => c.trim().to_string(),
        None => first.chars().take(40).collect(),
    }
}

/// The watched file's text, its added lines and blocks; None if it or its default is missing.
fn scan(p: &Paths, shell: Shell, file: &str) -> Result<Option<(String, Vec<Block>)>> {
    if let Some(why) = shell.skip_reason(p)? {
        eprintln!("rc-sort: {why}");
        return Ok(None);
    }
    let Some(current) = read_opt(&p.home.join(file))? else { return Ok(None) };
    let Some(default) = shell.default_text(p, file)? else {
        eprintln!("rc-sort: no default {}, skipping ~/{file}", p.skel.join(file).display());
        return Ok(None);
    };
    let added = added_lines(&default, &current);
    let blocks = split_blocks(shell, &current, &added);
    Ok(Some((current, blocks)))
}

/// Appends `text` to its target unless some `~/.bashrc.d` file (other than `skip`) already
/// has it. `from` is the source as shown to the user. Returns the message.
pub(crate) fn place(
    p: &Paths,
    shell: Shell,
    from: &str,
    name: &str,
    text: &str,
    cat: Category,
    skip: Option<&str>,
) -> Result<String> {
    if let Some(other) = dup_in(&rc_files(p, shell)?, text, skip) {
        return Ok(format!("rc-sort: removed duplicate {name} block from {from} (already in {other})"));
    }
    let target_name = shell.target_name(cat, text).context("the block is not one function")?;
    let path = shell.rc_dir(p).join(&target_name);
    let mut target = read_opt(&path)?.unwrap_or_default();
    if !target.is_empty() {
        if !target.ends_with('\n') {
            target.push('\n');
        }
        target.push('\n');
    }
    target.push_str(&normalize(text));
    target.push('\n');
    atomic_write(&path, &target)?;
    Ok(format!("rc-sort: moved {name} block from {from} to {target_name}"))
}

/// `(name, text)` of every file in `~/.bashrc.d` the sorter reads, sorted by name: regular,
/// UTF-8 files, not the hook or our temp files.
pub(crate) fn rc_files(p: &Paths, shell: Shell) -> Result<Vec<(String, String)>> {
    let dir = shell.rc_dir(p);
    let mut out = match shell {
        Shell::Bash => list(&dir, "")?,
        Shell::Fish => [list(&dir.join("conf.d"), "conf.d/")?, list(&dir.join("functions"), "functions/")?]
            .concat()
            .into_iter()
            .filter(|(n, _)| n.ends_with(".fish"))
            .collect(),
    };
    out.sort();
    Ok(out)
}

fn list(dir: &std::path::Path, prefix: &str) -> Result<Vec<(String, String)>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut out = Vec::new();
    for e in entries {
        let e = e.with_context(|| format!("reading {}", dir.display()))?;
        let name = e.file_name().to_string_lossy().into_owned();
        if name == HOOK || name == FISH_HOOK || name.ends_with(".rc-sort.tmp") || !e.path().is_file() {
            continue;
        }
        if let Ok(text) = fs::read_to_string(e.path()) {
            out.push((format!("{prefix}{name}"), text));
        }
    }
    Ok(out)
}

/// The first file (other than `skip`) that already holds `text` as whole lines.
pub(crate) fn dup_in<'a>(files: &'a [(String, String)], text: &str, skip: Option<&str>) -> Option<&'a str> {
    files
        .iter()
        .find(|(n, t)| Some(n.as_str()) != skip && contains_block(t, text))
        .map(|(n, _)| n.as_str())
}

/// A known tool `text` sets up that tools.sh or a not-yet-sorted file (other than `skip`)
/// already names. aliases/exports/functions.sh are left out: `alias cb="cargo build"` is
/// not a cargo setup.
pub(crate) fn tool_clash(
    shell: Shell,
    files: &[(String, String)],
    text: &str,
    skip: Option<&str>,
) -> Option<(String, &'static str)> {
    let ours = tools_named(text);
    files
        .iter()
        .filter(|(n, _)| Some(n.as_str()) != skip)
        .filter(|(n, _)| shell.target_name(Category::Tools, "").as_deref() == Some(n) || !shell.is_category_file(n))
        .find_map(|(n, t)| {
            let theirs = tools_named(t);
            ours.iter().find(|x| theirs.contains(x)).map(|&tool| (n.clone(), tool))
        })
}

/// True if `block` appears in `target` as whole lines (ignoring trailing spaces).
fn contains_block(target: &str, block: &str) -> bool {
    format!("\n{}\n", normalize(target)).contains(&format!("\n{}\n", normalize(block)))
}

/// `current` without the given blocks. Blank lines directly above a block go too when the
/// block is followed by a blank line or the end of the file, so no stray blanks pile up
/// (the blank may match a default line, so it can't be judged by "added").
pub(crate) fn without(current: &str, blocks: &[&Block]) -> String {
    let lines: Vec<&str> = current.lines().collect();
    let blank = |i: usize| lines.get(i).is_none_or(|l| l.trim().is_empty());
    let mut drop: HashSet<usize> = HashSet::new();
    for b in blocks {
        drop.extend(&b.lines);
        let after = b.lines[b.lines.len() - 1] + 1;
        if blank(after) {
            let mut i = b.lines[0];
            while i > 0 && blank(i - 1) {
                i -= 1;
                drop.insert(i);
            }
        }
    }
    let mut out: String = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !drop.contains(i))
        .map(|(_, l)| format!("{l}\n"))
        .collect();
    if !current.ends_with('\n') {
        out.pop();
    }
    out
}

fn pending(file: &str, b: &Block, note: String) -> Pending {
    Pending { hash: hash(&b.text), file: file.to_string(), text: b.text.clone(), note }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn rewrite_skips_file_changed_since_read() {
        let home = std::env::temp_dir().join(format!("rc-sort-test-{}-rewrite", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        let p = Paths { home, skel: "/nonexistent".into() };
        // The pass read "a\n"; an installer appended "b" before the rewrite.
        fs::write(p.home.join(".bashrc"), "a\nb\n").unwrap();
        let block = Block { lines: vec![0], text: "a".into(), balanced: true };
        rewrite(&p, ".bashrc", "a\n", &[&block]).unwrap();
        assert_eq!(fs::read_to_string(p.home.join(".bashrc")).unwrap(), "a\nb\n");
    }
}
