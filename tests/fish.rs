use rc_sort::sort::run_once;
use rc_sort::state::{Paths, State};
use std::fs;
use std::path::PathBuf;

const CONFIG: &str = ".config/fish/config.fish";

fn setup(name: &str) -> Paths {
    let skel = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skel");
    let home = std::env::temp_dir().join(format!("rc-sort-fish-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(home.join(".config/fish")).unwrap();
    fs::copy(skel.join(CONFIG), home.join(CONFIG)).unwrap();
    Paths { home, skel }
}

fn default_config(p: &Paths) -> String {
    fs::read_to_string(p.skel.join(CONFIG)).unwrap()
}

fn append(p: &Paths, text: &str) {
    fs::write(p.home.join(CONFIG), format!("{}\n{text}", default_config(p))).unwrap();
}

fn read(p: &Paths, rel: &str) -> Option<String> {
    fs::read_to_string(p.home.join(".config/fish").join(rel)).ok()
}

#[test]
fn installer_blocks_move_to_conf_d() {
    // Arrange
    let p = setup("conf-d");
    append(&p, "# uv\nfish_add_path \"/home/u/.local/bin\"\n\nset -gx EDITOR nvim\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    assert_eq!(read(&p, "config.fish").unwrap(), default_config(&p));
    assert_eq!(read(&p, "conf.d/tools.fish").unwrap(), "# uv\nfish_add_path \"/home/u/.local/bin\"\n");
    assert_eq!(read(&p, "conf.d/exports.fish").unwrap(), "set -gx EDITOR nvim\n");
}

#[test]
fn function_gets_its_own_autoload_file() {
    // Arrange
    let p = setup("function");
    append(&p, "function mkcd\n    mkdir -p $argv[1]\nend\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    assert_eq!(read(&p, "functions/mkcd.fish").unwrap(), "function mkcd\n    mkdir -p $argv[1]\nend\n");
}

#[test]
fn function_whose_file_exists_is_asked() {
    // Arrange
    let p = setup("function-exists");
    fs::create_dir_all(p.home.join(".config/fish/functions")).unwrap();
    fs::write(p.home.join(".config/fish/functions/mkcd.fish"), "function mkcd\n    mkdir $argv\nend\n").unwrap();
    append(&p, "function mkcd\n    mkdir -p $argv[1]\nend\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    let pending = State::load(&p).unwrap().pending;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].note, "functions/mkcd.fish already exists");
}

#[test]
fn moves_are_backed_up() {
    // Arrange
    let p = setup("backup");
    append(&p, "set -gx EDITOR nvim\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    let backups = fs::read_dir(p.state_dir().join("backups")).unwrap().count();
    assert_eq!(backups, 1);
}

#[test]
fn message_names_the_fish_target() {
    // Arrange
    let p = setup("message");
    append(&p, "abbr -a gs git status\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    let messages = State::load(&p).unwrap().messages;
    assert_eq!(messages, vec!["rc-sort: moved abbr -a gs git status block from ~/.config/fish/config.fish to conf.d/aliases.fish"]);
}
