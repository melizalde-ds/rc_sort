/// A group of added lines that moves as one unit.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// Indexes into `current.lines()`.
    pub lines: Vec<usize>,
    /// The lines joined with `\n`, no trailing newline.
    pub text: String,
    /// Every `if`/`case`/loop/`{` opened in the block is closed again.
    pub balanced: bool,
}

/// Groups added lines into blocks. A block ends at a blank line or at a non-added line,
/// except a blank line inside an open construct or a `# X` … `# X end` marker pair.
/// Blocks that are only comments are dropped.
pub fn split_blocks(current: &str, added: &[usize]) -> Vec<Block> {
    let lines: Vec<&str> = current.lines().collect();
    let mut blocks = Vec::new();
    let mut start = 0;
    for i in 0..added.len() {
        // A non-added line always ends the block: it can't move with it.
        if i + 1 == added.len() || added[i + 1] != added[i] + 1 {
            split_run(&lines, &added[start..=i], &mut blocks);
            start = i + 1;
        }
    }
    blocks
}

/// Splits one run of consecutive added lines on blank lines.
fn split_run(lines: &[&str], run: &[usize], blocks: &mut Vec<Block>) {
    let mut cur = Vec::new();
    let mut depth = 0i32;
    let mut marker_end: Option<String> = None;
    for (k, &idx) in run.iter().enumerate() {
        let t = lines[idx].trim();
        if t.is_empty() {
            if depth > 0 || marker_end.is_some() {
                if !cur.is_empty() {
                    cur.push(idx);
                }
            } else {
                flush(lines, &mut cur, depth, blocks);
                depth = 0;
            }
            continue;
        }
        if marker_end.as_deref() == Some(t) {
            marker_end = None;
        } else if marker_end.is_none() {
            if let Some(name) = t.strip_prefix('#').map(str::trim) {
                let end = format!("# {name} end");
                if !name.is_empty() && run[k + 1..].iter().any(|&j| lines[j].trim() == end) {
                    marker_end = Some(end);
                }
            }
        }
        depth += depth_delta(t);
        cur.push(idx);
    }
    flush(lines, &mut cur, depth, blocks);
}

fn flush(lines: &[&str], cur: &mut Vec<usize>, depth: i32, blocks: &mut Vec<Block>) {
    while cur.last().is_some_and(|&i| lines[i].trim().is_empty()) {
        cur.pop();
    }
    let only_comments = cur.iter().all(|&i| {
        let t = lines[i].trim();
        t.is_empty() || t.starts_with('#')
    });
    if !only_comments {
        let text = cur.iter().map(|&i| lines[i]).collect::<Vec<_>>().join("\n");
        blocks.push(Block { lines: cur.clone(), text, balanced: depth == 0 });
    }
    cur.clear();
}

/// How much a line opens (+) or closes (-) shell constructs. Comment lines count 0.
fn depth_delta(t: &str) -> i32 {
    if t.starts_with('#') {
        return 0;
    }
    let mut d: i32 = t
        .split(|c: char| c.is_whitespace() || c == ';')
        .map(|w| match w {
            "if" | "case" | "for" | "while" | "until" => 1,
            "fi" | "esac" | "done" => -1,
            _ => 0,
        })
        .sum();
    if t.ends_with('{') {
        d += 1;
    }
    if t == "}" {
        d -= 1;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every line of `text` counts as added.
    fn all(text: &str) -> Vec<Block> {
        let added: Vec<usize> = (0..text.lines().count()).collect();
        split_blocks(text, &added)
    }

    #[test]
    fn bun_block_with_leading_comment() {
        let b = all("# bun\nexport BUN_INSTALL=...\nexport PATH=...\n");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].lines, vec![0, 1, 2]);
        assert_eq!(b[0].text, "# bun\nexport BUN_INSTALL=...\nexport PATH=...");
    }

    #[test]
    fn blank_line_splits_blocks() {
        let b = all("alias a=x\n\nalias b=y\n");
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].lines, vec![2]);
    }

    #[test]
    fn pnpm_markers_join_across_blank() {
        let text = "# pnpm\nexport PNPM_HOME=...\n\ncase \":$PATH:\" in\n  *\":$PNPM_HOME:\"*) ;;\n  *) export PATH=...;;\nesac\n# pnpm end\n";
        let b = all(text);
        assert_eq!(b.len(), 1);
        assert!(b[0].text.starts_with("# pnpm\n"));
        assert!(b[0].text.ends_with("# pnpm end"));
        assert!(b[0].balanced);
    }

    #[test]
    fn fnm_if_not_split() {
        let b = all("FNM_PATH=...\nif [ -d \"$FNM_PATH\" ]; then\n\n  eval \"$(fnm env)\"\nfi\n");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].lines, vec![0, 1, 2, 3, 4]);
        assert!(b[0].balanced);
    }

    #[test]
    fn function_body_not_split() {
        let b = all("mkcd() {\n  mkdir -p \"$1\"\n\n  cd \"$1\"\n}\n");
        assert_eq!(b.len(), 1);
        assert!(b[0].balanced);
    }

    #[test]
    fn unclosed_if_is_unbalanced() {
        let b = all("if true; then\n  echo hi\n");
        assert_eq!(b.len(), 1);
        assert!(!b[0].balanced);
    }

    #[test]
    fn one_line_if_is_balanced() {
        let b = all("if [ -f ~/x ]; then . ~/x; fi\n\nalias a=b\n");
        assert_eq!(b.len(), 2);
        assert!(b[0].balanced);
    }

    #[test]
    fn comment_only_block_dropped() {
        assert!(all("# just a note\n").is_empty());
    }

    #[test]
    fn non_added_line_splits() {
        let b = split_blocks("alias a=x\nDEFAULT\nalias b=y\n", &[0, 2]);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].text, "alias a=x");
        assert_eq!(b[1].text, "alias b=y");
    }
}
