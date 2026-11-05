use crate::classify::Category;
use crate::sort::move_block;
use crate::state::{lock, Paths, Pending, State};
use anyhow::Result;
use std::fs;
use std::io::{BufRead, Write};

const QUESTION: &str = "[t]ools [e]xports [a]liases [f]unctions [k]eep in place: ";

/// What the user chose for a pending block.
enum Answer {
    Move(Category),
    Keep,
    Later,
}

/// Prints queued messages, then asks about each pending block. Run from the shell hook.
pub fn run_prompt(p: &Paths, input: &mut impl BufRead, out: &mut impl Write) -> Result<()> {
    // Fast path for every new terminal: no lock, no state dir created. `> 2` skips "" and "[]".
    let has = |name: &str| fs::metadata(p.state_dir().join(name)).is_ok_and(|m| m.len() > 2);
    if !has("messages") && !has("pending.json") {
        return Ok(());
    }

    let pending = {
        let _lock = lock(p)?;
        let mut state = State::load(p)?;
        flush_messages(&mut state, out)?;
        state.save(p)?;
        state.pending
    }; // unlocked while waiting for the user

    for item in pending {
        let answer = ask(&item, input, out)?;
        if let Answer::Later = answer {
            continue;
        }
        let _lock = lock(p)?;
        let mut state = State::load(p)?;
        state.pending.retain(|x| x.hash != item.hash);
        match answer {
            Answer::Move(cat) => {
                // Ok(false): the block was edited or removed since; nothing to move.
                move_block(p, &mut state, &item.file, item.hash, cat)?;
            }
            Answer::Keep => {
                state.kept.insert(item.hash);
            }
            Answer::Later => unreachable!(),
        }
        flush_messages(&mut state, out)?;
        state.save(p)?;
    }
    Ok(())
}

fn flush_messages(state: &mut State, out: &mut impl Write) -> Result<()> {
    for m in state.messages.drain(..) {
        writeln!(out, "{m}")?;
    }
    Ok(())
}

/// Shows the block and reads an answer. Empty input or EOF means "ask again next terminal";
/// an unknown key is asked once more.
fn ask(item: &Pending, input: &mut impl BufRead, out: &mut impl Write) -> Result<Answer> {
    if item.note.is_empty() {
        writeln!(out, "bashrc-sort: unsure where this block from ~/{} goes:", item.file)?;
    } else {
        writeln!(out, "bashrc-sort: {}; this one from ~/{} may be a duplicate:", item.note, item.file)?;
    }
    for line in item.text.lines() {
        writeln!(out, "    {line}")?;
    }
    for _ in 0..2 {
        write!(out, "{QUESTION}")?;
        out.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            writeln!(out)?;
            return Ok(Answer::Later);
        }
        match line.trim().to_ascii_lowercase().chars().next() {
            None => return Ok(Answer::Later),
            Some('k') => return Ok(Answer::Keep),
            Some(c) => {
                if let Some(cat) = Category::from_key(c) {
                    return Ok(Answer::Move(cat));
                }
            }
        }
    }
    Ok(Answer::Later)
}
