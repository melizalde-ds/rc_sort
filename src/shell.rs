use crate::blocks::{bash_depth, fish_depth, Block};
use crate::classify::{classify_bash, classify_fish, function_name, Category};
use crate::state::{read_opt, Paths};
use anyhow::Result;
use std::path::PathBuf;

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
