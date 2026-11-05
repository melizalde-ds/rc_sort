use crate::blocks::{split_blocks, Block};
use crate::classify::{classify, tools_named, Category};
use crate::diff::added_lines;
use crate::state::{atomic_write, backup, hash, lock, normalize, read_opt, Paths, Pending, State, WATCHED};
use anyhow::Result;
use std::collections::HashSet;

/// One pass over both watched files: move sure blocks, record unsure ones as pending.
pub fn run_once(p: &Paths) -> Result<()> {
    let _lock = lock(p)?;
    let mut state = State::load(p)?;
    for file in WATCHED {
        let Some((current, blocks)) = scan(p, file)? else { continue };
        // Blocks the user edited or removed by hand are no longer worth asking about.
        let live: HashSet<u64> = blocks.iter().map(|b| hash(&b.text)).collect();
        state.pending.retain(|x| x.file != file || live.contains(&x.hash));
        let mut done: Vec<&Block> = Vec::new();
        for b in &blocks {
            let h = hash(&b.text);
            if state.kept.contains(&h) || state.pending.iter().any(|x| x.hash == h) {
                continue;
            }
            let Some(cat) = classify(b) else {
                state.pending.push(pending(file, b, String::new()));
                continue;
            };
            let target = read_opt(&p.rc_dir().join(cat.file_name()))?.unwrap_or_default();
            if !contains_block(&target, &b.text) {
                let ours = tools_named(&target);
                if let Some(tool) = tools_named(&b.text).into_iter().find(|t| ours.contains(t)) {
                    let note = format!("{} already has a {tool} block", cat.file_name());
                    state.pending.push(pending(file, b, note));
                    continue;
                }
            }
            if done.is_empty() {
                backup(p, file, &current)?;
            }
            state.messages.push(place(p, file, b, cat)?);
            done.push(b);
        }
        if !done.is_empty() {
            atomic_write(&p.home.join(file), &without(&current, &done))?;
        }
    }
    state.save(p)
}

/// Re-reads `file`, finds the block whose hash == `h`, moves it to `cat` (no duplicate-tool check:
/// the user decided). Ok(false) if the block is no longer there. Caller holds the lock.
pub fn move_block(p: &Paths, state: &mut State, file: &str, h: u64, cat: Category) -> Result<bool> {
    let Some((current, blocks)) = scan(p, file)? else { return Ok(false) };
    let Some(b) = blocks.iter().find(|b| hash(&b.text) == h) else { return Ok(false) };
    backup(p, file, &current)?;
    state.messages.push(place(p, file, b, cat)?);
    atomic_write(&p.home.join(file), &without(&current, &[b]))?;
    Ok(true)
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
fn scan(p: &Paths, file: &str) -> Result<Option<(String, Vec<Block>)>> {
    let Some(current) = read_opt(&p.home.join(file))? else { return Ok(None) };
    let Some(default) = read_opt(&p.skel.join(file))? else {
        eprintln!("bashrc-sort: no default {}, skipping ~/{file}", p.skel.join(file).display());
        return Ok(None);
    };
    let added = added_lines(&default, &current);
    let blocks = split_blocks(&current, &added);
    Ok(Some((current, blocks)))
}

/// Appends the block to its target unless it's already there. Returns the message.
fn place(p: &Paths, file: &str, b: &Block, cat: Category) -> Result<String> {
    let path = p.rc_dir().join(cat.file_name());
    let mut target = read_opt(&path)?.unwrap_or_default();
    let name = block_name(b);
    if contains_block(&target, &b.text) {
        return Ok(format!(
            "bashrc-sort: removed duplicate {name} block from ~/{file} (already in {})",
            cat.file_name()
        ));
    }
    if !target.is_empty() {
        if !target.ends_with('\n') {
            target.push('\n');
        }
        target.push('\n');
    }
    target.push_str(&normalize(&b.text));
    target.push('\n');
    atomic_write(&path, &target)?;
    Ok(format!("bashrc-sort: moved {name} block from ~/{file} to {}", cat.file_name()))
}

/// True if `block` appears in `target` as whole lines (ignoring trailing spaces).
fn contains_block(target: &str, block: &str) -> bool {
    format!("\n{}\n", normalize(target)).contains(&format!("\n{}\n", normalize(block)))
}

/// `current` without the given blocks. Blank lines directly above a block go too when the
/// block is followed by a blank line or the end of the file, so no stray blanks pile up
/// (the blank may match a default line, so it can't be judged by "added").
fn without(current: &str, blocks: &[&Block]) -> String {
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
