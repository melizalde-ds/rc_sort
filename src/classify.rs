use crate::blocks::Block;

/// Which `~/.bashrc.d/` file a block belongs in.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Category {
    Aliases,
    Exports,
    Functions,
    Tools,
}

impl Category {
    pub const ALL: [Category; 4] =
        [Category::Aliases, Category::Exports, Category::Functions, Category::Tools];

    pub fn file_name(self) -> &'static str {
        match self {
            Category::Aliases => "aliases.sh",
            Category::Exports => "exports.sh",
            Category::Functions => "functions.sh",
            Category::Tools => "tools.sh",
        }
    }

    /// The prompt keys: t/e/a/f.
    pub fn from_key(c: char) -> Option<Self> {
        match c {
            't' => Some(Category::Tools),
            'e' => Some(Category::Exports),
            'a' => Some(Category::Aliases),
            'f' => Some(Category::Functions),
            _ => None,
        }
    }
}

const KNOWN_TOOLS: [&str; 13] = [
    "cargo", "rustup", "fnm", "nvm", "bun", "pnpm", "deno", "pyenv", "conda", "sdkman", "zoxide",
    "starship", "direnv",
];

/// Sorts a block by the spec's rules, first match wins. `None` means unsure.
pub fn classify(b: &Block) -> Option<Category> {
    if !b.balanced || leaves_file(b) {
        return None;
    }
    let code: Vec<&str> = b
        .text
        .lines()
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.starts_with('#'))
        .collect();
    if code.iter().any(|t| defines_function(t)) {
        Some(Category::Functions)
    } else if code.iter().all(|t| t.starts_with("alias ")) {
        Some(Category::Aliases)
    } else if code.iter().any(|t| sets_path(t) || runs_command(t) || sources_file(t))
        || !tools_named(&code.join("\n")).is_empty()
    {
        Some(Category::Tools)
    } else if code.iter().all(|t| t.starts_with("export ") && t.contains('=')) {
        Some(Category::Exports)
    } else {
        None
    }
}

/// True if the block can `return`/`exit` outside a function body. Sourced from a
/// `~/.bashrc.d` file, that stops the rest of the file, so it must never be merged into one.
pub fn leaves_file(b: &Block) -> bool {
    let code: Vec<&str> = b
        .text
        .lines()
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.starts_with('#'))
        .collect();
    !code.iter().any(|t| defines_function(t))
        && code.iter().any(|t| {
            t.split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))
                .any(|w| w == "return" || w == "exit")
        })
}

/// Known tool names that appear as whole words in `text`, in first-seen order.
pub fn tools_named(text: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
        if let Some(&tool) = KNOWN_TOOLS.iter().find(|&&k| k == word)
            && !found.contains(&tool) {
                found.push(tool);
            }
    }
    found
}

/// `name()` / `name ()` or `function name`.
fn defines_function(t: &str) -> bool {
    if t.starts_with("function ") {
        return true;
    }
    let name_len = t
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-')))
        .unwrap_or(t.len());
    let starts_ok = t.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_');
    starts_ok && t[name_len..].trim_start().starts_with("()")
}

/// Assigns `PATH` itself (not `MANPATH`, not a bare `export PATH`).
fn sets_path(t: &str) -> bool {
    t.match_indices("PATH=").any(|(i, _)| {
        t[..i]
            .chars()
            .last()
            .is_none_or(|c| c.is_whitespace() || matches!(c, ';' | ')' | '('))
    })
}

/// Runs a command (`$(…)` or backticks), e.g. `eval "$(fnm env)"`. Such blocks go to
/// tools.sh, which loads last, so the command sees the PATH the tools set up.
fn runs_command(t: &str) -> bool {
    t.contains("$(") || t.contains('`')
}

/// `.`, `\.` or `source` as a command: at line start or after `&&`, `||`, `;`, `then`, `do`, `else`, `{`.
fn sources_file(t: &str) -> bool {
    let words: Vec<&str> = t.split_whitespace().collect();
    words.iter().enumerate().any(|(i, w)| {
        matches!(*w, "." | "\\." | "source")
            && (i == 0
                || matches!(words[i - 1], "&&" | "||" | "then" | "do" | "else" | "{")
                || words[i - 1].ends_with(';'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::split_blocks;
    use Category::*;

    /// Classifies `text` as one block (every line added).
    fn class(text: &str) -> Option<Category> {
        let added: Vec<usize> = (0..text.lines().count()).collect();
        let blocks = split_blocks(text, &added);
        assert_eq!(blocks.len(), 1, "sample must be one block: {text:?}");
        classify(&blocks[0])
    }

    #[test]
    fn bun_is_tools() {
        assert_eq!(class("# bun\nexport BUN_INSTALL=\"$HOME/.bun\"\nexport PATH=\"$BUN_INSTALL/bin:$PATH\""), Some(Tools));
    }

    #[test]
    fn pnpm_is_tools() {
        let t = "# pnpm\nexport PNPM_HOME=\"/home/u/.local/share/pnpm\"\ncase \":$PATH:\" in\n  *\":$PNPM_HOME:\"*) ;;\n  *) export PATH=\"$PNPM_HOME:$PATH\" ;;\nesac\n# pnpm end";
        assert_eq!(class(t), Some(Tools));
    }

    #[test]
    fn fnm_is_tools() {
        let t = "# fnm\nFNM_PATH=\"/home/u/.local/share/fnm\"\nif [ -d \"$FNM_PATH\" ]; then\n  export PATH=\"$FNM_PATH:$PATH\"\n  eval \"`fnm env`\"\nfi";
        assert_eq!(class(t), Some(Tools));
    }

    #[test]
    fn cargo_env_is_tools() {
        assert_eq!(class(". \"$HOME/.cargo/env\""), Some(Tools));
    }

    #[test]
    fn guarded_source_is_tools() {
        assert_eq!(class("[ -s \"$HOME/.foo/init.sh\" ] && \\. \"$HOME/.foo/init.sh\""), Some(Tools));
    }

    #[test]
    fn alias_is_aliases() {
        assert_eq!(class("alias zz=\"ls\""), Some(Aliases));
    }

    #[test]
    fn function_is_functions() {
        assert_eq!(class("mkcd() {\n  mkdir -p \"$1\"\n  cd \"$1\"\n}"), Some(Functions));
    }

    #[test]
    fn function_keyword_is_functions() {
        assert_eq!(class("function hi {\n echo hi\n}"), Some(Functions));
    }

    #[test]
    fn function_with_path_is_functions() {
        assert_eq!(class("f() { :; }\nexport PATH=\"$HOME/x:$PATH\""), Some(Functions));
    }

    #[test]
    fn plain_export_is_exports() {
        assert_eq!(class("export EDITOR=nano"), Some(Exports));
    }

    #[test]
    fn mixed_alias_export_is_unsure() {
        assert_eq!(class("alias a=b\nexport C=d"), None);
    }

    #[test]
    fn export_running_a_command_is_tools() {
        // exports.sh loads before tools.sh; `which java` needs the PATH that tools set up.
        let t = "export JAVA_HOME=$(dirname $(dirname $(readlink -f $(which java))))";
        assert_eq!(class(t), Some(Tools));
    }

    #[test]
    fn unknown_command_is_unsure() {
        assert_eq!(class("ssh-agent -s > /dev/null"), None);
    }

    #[test]
    fn unbalanced_block_is_unsure() {
        assert_eq!(class("if true; then\n eval \"$(fnm env)\""), None);
    }

    #[test]
    fn return_guard_is_unsure() {
        assert_eq!(class("[[ $- == *i* ]] || return\neval \"$(starship init bash)\""), None);
        assert_eq!(class("case $- in\n  *i*) ;;\n  *) return;;\nesac"), None);
    }

    #[test]
    fn return_inside_function_is_fine() {
        assert_eq!(class("f() {\n  [ -z \"$1\" ] && return 1\n  echo \"$1\"\n}"), Some(Functions));
        assert_eq!(class("alias q=exit"), Some(Aliases));
    }

    #[test]
    fn tools_named_finds_words() {
        assert_eq!(tools_named(". \"$HOME/.cargo/env\"\nexport PNPM_HOME=x"), vec!["cargo"]);
    }

    #[test]
    fn keys_and_file_names() {
        assert_eq!(Category::from_key('t'), Some(Tools));
        assert_eq!(Category::from_key('k'), None);
        assert_eq!(Functions.file_name(), "functions.sh");
    }
}
