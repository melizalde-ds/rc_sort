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
    Paths { home, skel, fish_vendor_conf: Vec::new() }
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

const FISH_TEMPLATE: &str = "if status is-interactive\n    # Commands to run in interactive sessions can go here\nend\n";

#[test]
fn without_a_skel_default_the_fish_template_is_the_baseline() {
    // Arrange
    let p = setup("template");
    let p = Paths { skel: p.home.join("empty-skel"), ..p };
    fs::write(p.home.join(CONFIG), format!("{FISH_TEMPLATE}\nset -gx EDITOR nvim\n")).unwrap();

    // Act
    run_once(&p).unwrap();

    // Assert
    assert_eq!(read(&p, "config.fish").unwrap(), FISH_TEMPLATE);
    assert_eq!(read(&p, "conf.d/exports.fish").unwrap(), "set -gx EDITOR nvim\n");
}

#[test]
fn line_inside_the_interactive_block_moves_wrapped() {
    // Arrange
    let p = setup("guard");
    let p = Paths { skel: p.home.join("empty-skel"), ..p };
    let config = "if status is-interactive\n    # Commands to run in interactive sessions can go here\n    starship init fish | source\nend\n";
    fs::write(p.home.join(CONFIG), config).unwrap();

    // Act
    run_once(&p).unwrap();

    // Assert
    assert_eq!(read(&p, "config.fish").unwrap(), FISH_TEMPLATE);
    assert_eq!(read(&p, "conf.d/tools.fish").unwrap(), "if status is-interactive\n    starship init fish | source\nend\n");
}

#[test]
fn line_inside_another_default_block_is_asked() {
    // Arrange
    let p = setup("other-block");
    let skel = p.home.join("skel");
    fs::create_dir_all(skel.join(".config/fish")).unwrap();
    fs::write(skel.join(CONFIG), "if test -d ~/x\n    set y 1\nend\n").unwrap();
    let p = Paths { skel, ..p };
    fs::write(p.home.join(CONFIG), "if test -d ~/x\n    set y 1\n    set -gx EDITOR nvim\nend\n").unwrap();

    // Act
    run_once(&p).unwrap();

    // Assert
    let pending = State::load(&p).unwrap().pending;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].note, "it is inside a block of the default file");
}

fn with_distro_config(name: &str, distro: &str) -> Paths {
    let p = setup(name);
    let distro_file = p.home.join("distro.fish");
    fs::write(&distro_file, distro).unwrap();
    let skel = p.home.join("skel");
    fs::create_dir_all(skel.join(".config/fish")).unwrap();
    fs::write(skel.join(CONFIG), format!("source {}\n", distro_file.display())).unwrap();
    let p = Paths { skel, ..p };
    fs::copy(p.skel.join(CONFIG), p.home.join(CONFIG)).unwrap();
    p
}

#[test]
fn alias_the_distro_config_also_defines_is_asked() {
    // Arrange
    let p = with_distro_config("distro-alias", "alias ls='eza -al'\n");
    append(&p, "alias ls \"eza --icons\"\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    let pending = State::load(&p).unwrap().pending;
    assert_eq!(pending.len(), 1);
    assert!(pending[0].note.ends_with("distro.fish also defines ls"), "{}", pending[0].note);
}

#[test]
fn function_the_distro_config_also_defines_is_asked() {
    // Arrange
    let p = with_distro_config("distro-function", "function copy\n    cp $argv\nend\n");
    append(&p, "function copy\n    cp -r $argv\nend\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    assert_eq!(State::load(&p).unwrap().pending.len(), 1);
    assert_eq!(read(&p, "functions/copy.fish"), None);
}

#[test]
fn alias_with_a_new_name_still_moves() {
    // Arrange
    let p = with_distro_config("distro-other", "alias ls='eza -al'\n");
    append(&p, "abbr -a gs git status\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    assert_eq!(read(&p, "conf.d/aliases.fish").unwrap(), "abbr -a gs git status\n");
}

#[test]
fn install_writes_the_fish_hook_when_fish_is_set_up() {
    use rc_sort::install::install_files;
    // Arrange
    let p = setup("install");

    // Act
    install_files(&p).unwrap();

    // Assert
    let hook = read(&p, "conf.d/zz-rc-sort.fish").unwrap();
    assert!(hook.contains("rc-sort prompt"), "{hook}");
}

#[test]
fn uninstall_removes_the_fish_hook() {
    use rc_sort::install::{install_files, uninstall_files};
    // Arrange
    let p = setup("uninstall");
    install_files(&p).unwrap();

    // Act
    uninstall_files(&p).unwrap();

    // Assert
    assert_eq!(read(&p, "conf.d/zz-rc-sort.fish"), None);
}

#[test]
fn watch_sorts_fish_changes() {
    // Arrange
    let p = setup("watch");
    let p2 = p.clone();
    std::thread::spawn(move || rc_sort::watch::run_watch(&p2));
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Act
    append(&p, "set -gx EDITOR nvim\n");

    // Assert
    let sorted = (0..50).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        read(&p, "conf.d/exports.fish").is_some_and(|s| s.contains("EDITOR"))
    });
    assert!(sorted);
}

fn conf_d(p: &Paths, files: &[(&str, &str)]) {
    let dir = p.home.join(".config/fish/conf.d");
    fs::create_dir_all(&dir).unwrap();
    for (name, text) in files {
        fs::write(dir.join(name), text).unwrap();
    }
}

fn tidy(p: &Paths, input: &str) -> String {
    let mut out = Vec::new();
    rc_sort::tidy::run_tidy(p, None, &mut std::io::Cursor::new(input.as_bytes()), &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

const PROMPT: &str = "if status is-interactive\n    starship init fish | source\n    zoxide init fish | source\nend\n";
const PATHS: &str = "fish_add_path ~/.cargo/bin ~/.local/bin\n";

#[test]
fn tidy_sorts_conf_d_files_into_tools_in_order() {
    // Arrange
    let p = setup("tidy");
    conf_d(&p, &[("prompt.fish", PROMPT), ("paths.fish", PATHS)]);

    // Act
    tidy(&p, "y\n");

    // Assert
    assert_eq!(read(&p, "conf.d/tools.fish").unwrap(), format!("{PATHS}\n{PROMPT}"));
    assert_eq!(read(&p, "conf.d/prompt.fish"), None);
}

#[test]
fn export_an_earlier_block_left_in_config_also_sets_is_asked() {
    // Arrange
    let p = setup("earlier-line-clash");
    append(&p, "set -gx EDITOR vim\nset -g fish_greeting\n\nset -gx EDITOR nvim\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    let pending = State::load(&p).unwrap().pending;
    let nvim = pending.iter().find(|x| x.text == "set -gx EDITOR nvim").map(|x| x.note.as_str());
    assert_eq!(nvim, Some("an earlier line in ~/.config/fish/config.fish also defines EDITOR"));
}

#[test]
fn tidy_asks_about_a_move_past_a_file_that_sets_the_same_variable() {
    // Arrange
    let p = setup("tidy-order");
    conf_d(&p, &[("a.fish", "set -gx EDITOR nvim\n"), ("b.fish", "set -gx EDITOR vim\n")]);

    // Act
    let out = tidy(&p, "\nn\n");

    // Assert
    assert!(out.contains("conf.d/b.fish also defines EDITOR"), "{out}");
}

#[test]
fn unsorted_names_fish_conf_d_files() {
    // Arrange
    let p = setup("unsorted");
    conf_d(&p, &[("paths.fish", PATHS), ("tools.fish", PROMPT)]);

    // Act
    let names = rc_sort::tidy::unsorted(&p).unwrap();

    // Assert
    assert_eq!(names, vec!["~/.config/fish/conf.d/paths.fish"]);
}

#[test]
fn tidy_wraps_blocks_from_a_fish_guarded_file() {
    // Arrange
    let p = setup("tidy-guard");
    conf_d(&p, &[("z.fish", "status is-interactive; or return\nzoxide init fish | source\n")]);

    // Act
    tidy(&p, "y\n");

    // Assert
    assert_eq!(read(&p, "conf.d/tools.fish").unwrap(), "if status is-interactive\n    zoxide init fish | source\nend\n");
}

#[test]
fn tidy_will_not_send_a_non_function_to_functions() {
    // Arrange
    let p = setup("tidy-function");
    conf_d(&p, &[("agent.fish", "ssh-agent -c > /dev/null\n")]);

    // Act
    let out = tidy(&p, "f\n");

    // Assert
    assert!(out.contains("rc-sort: nothing to tidy"), "{out}");
}

#[test]
fn half_typed_fish_alias_is_asked() {
    // Arrange
    let p = setup("syntax");
    append(&p, "alias gs 'git sta\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    assert_eq!(State::load(&p).unwrap().pending[0].note, "fish --no-execute finds a syntax error in it");
}

#[test]
fn export_a_later_conf_d_file_also_sets_is_asked() {
    // Arrange
    let p = setup("later-clash");
    conf_d(&p, &[("work.fish", "set -gx EDITOR vim\n")]);
    append(&p, "set -gx EDITOR nvim\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    assert_eq!(State::load(&p).unwrap().pending[0].note, "conf.d/work.fish loads later and also defines EDITOR");
}

#[test]
fn export_an_earlier_conf_d_file_also_sets_still_moves() {
    // Arrange
    let p = setup("earlier-clash");
    conf_d(&p, &[("10-work.fish", "set -gx EDITOR vim\n")]);
    append(&p, "set -gx EDITOR nvim\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    assert!(read(&p, "conf.d/exports.fish").is_some_and(|s| s.contains("nvim")));
}

#[test]
fn watch_sorts_a_fish_config_made_after_it_started() {
    // Arrange
    let p = setup("watch-late-dir");
    fs::remove_dir_all(p.home.join(".config/fish")).unwrap();
    let p2 = p.clone();
    std::thread::spawn(move || rc_sort::watch::run_watch(&p2));
    std::thread::sleep(std::time::Duration::from_millis(2000));

    // Act
    fs::create_dir_all(p.home.join(".config/fish")).unwrap();
    append(&p, "set -gx EDITOR nvim\n");

    // Assert
    let sorted = (0..50).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        read(&p, "conf.d/exports.fish").is_some_and(|s| s.contains("EDITOR"))
    });
    assert!(sorted);
}

#[test]
fn watch_sorts_after_the_fish_folder_is_restored() {
    // Arrange
    let p = setup("watch-restored-dir");
    let p2 = p.clone();
    std::thread::spawn(move || rc_sort::watch::run_watch(&p2));
    std::thread::sleep(std::time::Duration::from_millis(1500));
    fs::remove_dir_all(p.home.join(".config/fish")).unwrap();
    fs::create_dir_all(p.home.join(".config/fish")).unwrap();
    fs::write(p.home.join(CONFIG), default_config(&p)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2500));

    // Act
    append(&p, "set -gx EDITOR nvim\n");

    // Assert
    let sorted = (0..50).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        read(&p, "conf.d/exports.fish").is_some_and(|s| s.contains("EDITOR"))
    });
    assert!(sorted);
}

#[test]
fn export_a_later_system_conf_d_file_also_sets_is_asked() {
    // Arrange
    let p = setup("vendor-clash");
    let vendor = p.home.join("vendor_conf.d");
    fs::create_dir_all(&vendor).unwrap();
    fs::write(vendor.join("vapoursynth.fish"), "set --export --global VSSCRIPT_PATH /usr/lib/x.so\n").unwrap();
    let p = Paths { fish_vendor_conf: vec![vendor], ..p };
    append(&p, "set -gx VSSCRIPT_PATH /opt/x.so\n");

    // Act
    run_once(&p).unwrap();

    // Assert
    let note = State::load(&p).unwrap().pending[0].note.clone();
    assert!(note.ends_with("vendor_conf.d/vapoursynth.fish loads later and also defines VSSCRIPT_PATH"), "{note}");
}
