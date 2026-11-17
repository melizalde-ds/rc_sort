use crate::blocks::{bash_depth, fish_depth, Block};
use crate::classify::{bash_defined_names, classify_bash, classify_fish, fish_defined_names, function_name, Category};
use crate::state::{read_opt, Paths};
use anyhow::Result;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const FISH_TEMPLATE: &str = "if status is-interactive\n    # Commands to run in interactive sessions can go here\nend\n";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shell {
    Bash,
    Fish,
}

impl Shell {
    pub const ALL: [Shell; 2] = [Shell::Bash, Shell::Fish];

    pub fn of(file: &str) -> Option<Shell> {
        Shell::ALL.into_iter().find(|s| s.watched().contains(&file))
    }

    pub fn watched(self) -> &'static [&'static str] {
        match self {
            Shell::Bash => &[".bashrc", ".bash_profile"],
            Shell::Fish => &[".config/fish/config.fish"],
        }
    }

    pub fn skip_reason(self, p: &Paths) -> Result<Option<String>> {
        match self {
            Shell::Bash => {
                let path = p.skel.join(".bashrc");
                Ok(read_opt(&path)?
                    .filter(|text| !text.contains(".bashrc.d"))
                    .map(|_| format!("{} never loads ~/.bashrc.d, so bash is not sorted", path.display())))
            }
            Shell::Fish => Ok(None),
        }
    }

    pub fn default_text(self, p: &Paths, file: &str) -> Result<Option<String>> {
        let skel = read_opt(&p.skel.join(file))?;
        Ok(match self {
            Shell::Bash => skel,
            Shell::Fish => skel.or_else(|| Some(FISH_TEMPLATE.to_string())),
        })
    }

    pub fn distro_clash(self, p: &Paths, text: &str) -> Result<Option<String>> {
        if self == Shell::Bash {
            return Ok(None);
        }
        let ours = fish_defined_names(text);
        let file = self.watched()[0];
        let Some(default) = self.default_text(p, file)? else { return Ok(None) };
        let mut sources = vec![(format!("the default {file}"), default.clone())];
        for path in default.lines().filter_map(|l| l.trim().strip_prefix("source ")).map(str::trim) {
            if path.starts_with('/')
                && let Some(text) = read_opt(std::path::Path::new(path))?
            {
                sources.push((path.to_string(), text));
            }
        }
        Ok(sources.iter().find_map(|(from, theirs)| {
            let theirs = fish_defined_names(theirs);
            ours.iter().find(|n| theirs.contains(n)).map(|n| format!("{from} also defines {n}"))
        }))
    }

    pub fn defined_names(self, text: &str) -> Vec<&str> {
        match self {
            Shell::Bash => bash_defined_names(text),
            Shell::Fish => fish_defined_names(text),
        }
    }

    pub fn syntax_error(self, text: &str) -> Option<String> {
        let check = match self {
            Shell::Bash => ["bash", "-n"],
            Shell::Fish => ["fish", "--no-execute"],
        };
        let mut child = Command::new(check[0])
            .arg(check[1])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let wrote = child.stdin.take().is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
        let passed = child.wait().is_ok_and(|status| status.success());
        (wrote && !passed).then(|| format!("{} finds a syntax error in it", check.join(" ")))
    }

    pub fn depth_delta(self, line: &str) -> i32 {
        match self {
            Shell::Bash => bash_depth(line),
            Shell::Fish => fish_depth(line),
        }
    }

    pub fn classify(self, block: &Block) -> Option<Category> {
        match self {
            Shell::Bash => classify_bash(block),
            Shell::Fish => classify_fish(block),
        }
    }

    pub fn opens_interactive(self, line: &str) -> bool {
        let t = line.split_whitespace().collect::<Vec<_>>().join(" ");
        match self {
            Shell::Bash => t == "if [[ $- == *i* ]]; then",
            Shell::Fish => t == "if status is-interactive" || t == "if status --is-interactive",
        }
    }

    pub fn wrap_interactive(self, text: &str) -> String {
        // Indenting would change a heredoc's content; leave those as they are.
        let body = if text.contains("<<") {
            text.to_string()
        } else {
            text.lines()
                .map(|l| if l.trim().is_empty() { String::new() } else { format!("    {l}") })
                .collect::<Vec<_>>()
                .join("\n")
        };
        match self {
            Shell::Bash => format!("if [[ $- == *i* ]]; then\n{body}\nfi"),
            Shell::Fish => format!("if status is-interactive\n{body}\nend"),
        }
    }

    pub fn rc_dir(self, p: &Paths) -> PathBuf {
        match self {
            Shell::Bash => p.rc_dir(),
            Shell::Fish => p.home.join(".config/fish"),
        }
    }

    pub fn target_name(self, cat: Category, text: &str) -> Option<String> {
        match (self, cat) {
            (Shell::Bash, _) => Some(cat.file_name().to_string()),
            (Shell::Fish, Category::Functions) => function_name(text).map(|n| format!("functions/{n}.fish")),
            (Shell::Fish, _) => Some(format!("conf.d/{}.fish", cat.stem())),
        }
    }

    pub fn is_category_file(self, name: &str) -> bool {
        match self {
            Shell::Bash => Category::ALL.iter().any(|c| c.file_name() == name),
            Shell::Fish => {
                name.starts_with("functions/")
                    || Category::ALL.iter().any(|&c| self.target_name(c, "").as_deref() == Some(name))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_file_names_its_shell() {
        // Arrange
        let files = [".bashrc", ".config/fish/config.fish", ".profile"];

        // Act
        let shells: Vec<Option<Shell>> = files.iter().map(|f| Shell::of(f)).collect();

        // Assert
        assert_eq!(shells, vec![Some(Shell::Bash), Some(Shell::Fish), None]);
    }
}
