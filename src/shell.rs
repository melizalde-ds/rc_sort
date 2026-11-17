use crate::blocks::{bash_depth, Block};
use crate::classify::{classify_bash, Category};
use crate::state::Paths;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shell {
    Bash,
}

impl Shell {
    pub const ALL: [Shell; 1] = [Shell::Bash];

    pub fn of(file: &str) -> Option<Shell> {
        Shell::ALL.into_iter().find(|s| s.watched().contains(&file))
    }

    pub fn watched(self) -> &'static [&'static str] {
        match self {
            Shell::Bash => &[".bashrc", ".bash_profile"],
        }
    }

    pub fn depth_delta(self, line: &str) -> i32 {
        match self {
            Shell::Bash => bash_depth(line),
        }
    }

    pub fn classify(self, block: &Block) -> Option<Category> {
        match self {
            Shell::Bash => classify_bash(block),
        }
    }

    pub fn target(self, p: &Paths, cat: Category) -> PathBuf {
        match self {
            Shell::Bash => p.rc_dir().join(cat.file_name()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_file_names_its_shell() {
        // Arrange
        let files = [".bashrc", ".bash_profile", ".profile"];

        // Act
        let shells: Vec<Option<Shell>> = files.iter().map(|f| Shell::of(f)).collect();

        // Assert
        assert_eq!(shells, vec![Some(Shell::Bash), Some(Shell::Bash), None]);
    }
}
