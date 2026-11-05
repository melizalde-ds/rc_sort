use similar::{DiffOp, TextDiff};

/// A replaced line this similar to a default line is an edit of it, not an addition.
const CHANGED_RATIO: f32 = 0.6;

/// Indexes (0-based, ascending) of lines in `current` that were added compared to `default`.
/// Edited default lines are not additions: moving them out could break the shell.
pub fn added_lines(default: &str, current: &str) -> Vec<usize> {
    let old: Vec<&str> = default.lines().collect();
    let new: Vec<&str> = current.lines().collect();
    let mut added = Vec::new();
    for op in TextDiff::from_lines(default, current).ops() {
        match *op {
            DiffOp::Insert { new_index, new_len, .. } => added.extend(new_index..new_index + new_len),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                let olds = &old[old_index..old_index + old_len];
                for (i, line) in new.iter().enumerate().skip(new_index).take(new_len) {
                    let edited = olds
                        .iter()
                        .any(|o| TextDiff::from_chars(*o, *line).ratio() >= CHANGED_RATIO);
                    if !edited {
                        added.push(i);
                    }
                }
            }
            DiffOp::Equal { .. } | DiffOp::Delete { .. } => {}
        }
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKEL: &str = "# .bash_profile\n\n# Get the aliases and functions\nif [ -f ~/.bashrc ]; then\n\t. ~/.bashrc\nfi\n\n# User specific environment and startup programs\n";

    #[test]
    fn appended_lines_are_added() {
        let cur = format!("{SKEL}\n# bun\nexport BUN_INSTALL=\"$HOME/.bun\"\n");
        assert_eq!(added_lines(SKEL, &cur), vec![8, 9, 10]); // blank + 2 lines
    }

    #[test]
    fn identical_file_has_no_additions() {
        assert!(added_lines(SKEL, SKEL).is_empty());
    }

    #[test]
    fn changed_default_line_is_not_added() {
        let cur = SKEL.replace("if [ -f ~/.bashrc ]; then", "if [ -f \"$HOME/.bashrc\" ]; then");
        assert!(added_lines(SKEL, &cur).is_empty());
    }

    #[test]
    fn append_after_deleted_default_lines_is_added() {
        // real ~/.bash_profile: last 2 skel lines deleted, then installer appends
        let base: String = SKEL.lines().take(6).map(|l| format!("{l}\n")).collect();
        let cur = format!("{base}# bun\nexport PATH=\"$HOME/.bun/bin:$PATH\"\n");
        assert_eq!(added_lines(SKEL, &cur), vec![6, 7]);
    }
}
