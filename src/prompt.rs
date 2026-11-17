use crate::classify::Category;
use crate::sort::move_block;
use crate::state::{lock, Paths, State};
use anyhow::Result;
use std::fs;
use std::io::{BufRead, Write};

const QUESTION: &str = "[t]ools [e]xports [a]liases [f]unctions [k]eep in place: ";

/// What the user chose for a block.
pub(crate) enum Answer {
    Move(Category),
    Keep,
    Later,
}

/// Prints queued messages, then asks about each pending block. Run from the shell hook.
pub fn run_prompt(p: &Paths, input: &mut impl BufRead, out: &mut impl Write) -> Result<()> {
    // Fast path for every new terminal: no lock, no state dir created. `> 2` skips "" and "[]".
    let has = |name: &str| fs::metadata(p.state_dir().join(name)).is_ok_and(|m| m.len() > 2);
    if !has("messages") && !has("pending.json") {
        return warn_unsorted(p, out);
    }

    let pending = {
        let _lock = lock(p)?;
        let mut state = State::load(p)?;
        flush_messages(&mut state, out)?;
        state.save(p)?;
        state.pending
    }; // unlocked while waiting for the user

    for item in pending {
        let header = if item.note.is_empty() {
            format!("rc-sort: unsure where this block from ~/{} goes:", item.file)
        } else {
            format!("rc-sort: unsure where this block from ~/{} goes ({}):", item.file, item.note)
        };
        let answer = ask(&header, &item.text, input, out)?;
        if let Answer::Later = answer {
            continue;
        }
        let _lock = lock(p)?;
        let mut state = State::load(p)?;
        if !state.pending.iter().any(|x| x.hash == item.hash) {
            continue;
        }
        state.pending.retain(|x| x.hash != item.hash);
        match answer {
            Answer::Move(cat) => {
                if !move_block(p, &mut state, &item.file, item.hash, cat)? {
                    // Keep it, or the next pass would ask about it forever.
                    state.kept.insert(item.hash);
                    writeln!(
                        out,
                        "rc-sort: left in ~/{}: it changed since, or it is half of an if/loop",
                        item.file
                    )?;
                }
            }
            Answer::Keep => {
                state.kept.insert(item.hash);
            }
            Answer::Later => unreachable!(),
        }
        flush_messages(&mut state, out)?;
        state.save(p)?;
    }
    warn_unsorted(p, out)
}

/// One line naming `~/.bashrc.d` files that `tidy` would still sort. A read error here
/// must not break shell startup, so it just skips the warning.
fn warn_unsorted(p: &Paths, out: &mut impl Write) -> Result<()> {
    if let Ok(names) = crate::tidy::unsorted(p)
        && !names.is_empty()
    {
        writeln!(out, "rc-sort: not sorted yet: {} (run: rc-sort tidy)", names.join(", "))?;
    }
    Ok(())
}

fn flush_messages(state: &mut State, out: &mut impl Write) -> Result<()> {
    for m in state.messages.drain(..) {
        writeln!(out, "{m}")?;
    }
    Ok(())
}

/// Shows the block and reads an answer. Empty input or EOF means "ask again later";
/// an unknown key is asked once more.
pub(crate) fn ask(header: &str, text: &str, input: &mut impl BufRead, out: &mut impl Write) -> Result<Answer> {
    writeln!(out, "{header}")?;
    for line in text.lines() {
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
