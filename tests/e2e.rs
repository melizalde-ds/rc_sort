use bashrc_sort::sort::run_once;
use bashrc_sort::state::{hash, Paths, State};
use std::fs;
use std::path::PathBuf;

const BUN: &str = "# bun\nexport BUN_INSTALL=\"$HOME/.bun\"\nexport PATH=$BUN_INSTALL/bin:$PATH\n";
const USER_BUN: &str = "# bun\nexport BUN_INSTALL=\"$HOME/.bun\"\ncase \":$PATH:\" in\n  *\":$BUN_INSTALL/bin:\"*) ;;\n  *) export PATH=\"$BUN_INSTALL/bin:$PATH\" ;;\nesac\n";

fn skel_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skel")
}

fn skel(file: &str) -> String {
    fs::read_to_string(skel_dir().join(file)).unwrap()
}

/// Temp HOME with default rc files and `tools.sh` = "# tools\n".
fn setup(name: &str) -> Paths {
    let root = std::env::temp_dir().join(format!("bashrc-sort-e2e-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let home = root.join("home");
    fs::create_dir_all(home.join(".bashrc.d")).unwrap();
    for f in [".bashrc", ".bash_profile"] {
        fs::write(home.join(f), skel(f)).unwrap();
    }
    fs::write(home.join(".bashrc.d/tools.sh"), "# tools\n").unwrap();
    Paths { home, skel: skel_dir() }
}

fn read(p: &Paths, rel: &str) -> String {
    fs::read_to_string(p.home.join(rel)).unwrap()
}

fn append(p: &Paths, rel: &str, text: &str) {
    let path = p.home.join(rel);
    let mut s = fs::read_to_string(&path).unwrap();
    s.push_str(text);
    fs::write(path, s).unwrap();
}

#[test]
fn bun_moves_to_tools_once() {
    let p = setup("bun");
    append(&p, ".bash_profile", &format!("\n{BUN}"));
    run_once(&p).unwrap();
    run_once(&p).unwrap();
    assert_eq!(read(&p, ".bash_profile"), skel(".bash_profile"));
    let tools = read(&p, ".bashrc.d/tools.sh");
    assert_eq!(tools.matches("# bun").count(), 1);
    assert!(tools.contains(BUN.trim_end()));
    let s = State::load(&p).unwrap();
    assert_eq!(s.messages, vec!["bashrc-sort: moved bun block from ~/.bash_profile to tools.sh"]);
    assert_eq!(fs::read_dir(p.state_dir().join("backups")).unwrap().count(), 1);
}

#[test]
fn same_tool_different_text_is_asked() {
    let p = setup("sametool");
    fs::write(p.home.join(".bashrc.d/tools.sh"), USER_BUN).unwrap();
    append(&p, ".bash_profile", &format!("\n{BUN}"));
    let before = read(&p, ".bash_profile");
    run_once(&p).unwrap();
    assert_eq!(read(&p, ".bash_profile"), before);
    assert_eq!(read(&p, ".bashrc.d/tools.sh"), USER_BUN);
    let s = State::load(&p).unwrap();
    assert_eq!(s.pending.len(), 1);
    assert_eq!(s.pending[0].note, "tools.sh already has a bun block");
}

#[test]
fn block_already_in_target_is_not_duplicated() {
    let p = setup("dup");
    fs::write(p.home.join(".bashrc.d/tools.sh"), format!("# tools\n\n{BUN}")).unwrap();
    append(&p, ".bash_profile", &format!("\n{BUN}"));
    run_once(&p).unwrap();
    assert_eq!(read(&p, ".bash_profile"), skel(".bash_profile"));
    assert_eq!(read(&p, ".bashrc.d/tools.sh").matches("# bun").count(), 1);
}

#[test]
fn append_to_file_without_trailing_newline() {
    let p = setup("nonl");
    fs::write(p.home.join(".bashrc.d/aliases.sh"), "alias a=b").unwrap();
    append(&p, ".bashrc", "alias zz=\"ls\"\n");
    run_once(&p).unwrap();
    assert_eq!(read(&p, ".bashrc.d/aliases.sh"), "alias a=b\n\nalias zz=\"ls\"\n");
    assert_eq!(read(&p, ".bashrc"), skel(".bashrc"));
}

#[test]
fn missing_target_is_created() {
    let p = setup("newtarget");
    append(&p, ".bashrc", "\nmkcd() {\n  mkdir -p \"$1\" && cd \"$1\"\n}\n");
    run_once(&p).unwrap();
    assert!(read(&p, ".bashrc.d/functions.sh").contains("mkcd() {"));
    assert_eq!(read(&p, ".bashrc"), skel(".bashrc"));
}

#[test]
fn unsure_stays_and_is_pending_once() {
    let p = setup("unsure");
    append(&p, ".bashrc", "ssh-agent -s > /dev/null\n");
    let before = read(&p, ".bashrc");
    run_once(&p).unwrap();
    run_once(&p).unwrap();
    assert_eq!(read(&p, ".bashrc"), before);
    let s = State::load(&p).unwrap();
    assert_eq!(s.pending.len(), 1);
    assert!(s.messages.is_empty());
}

#[test]
fn kept_block_is_ignored() {
    let p = setup("kept");
    append(&p, ".bashrc", "ssh-agent -s > /dev/null\n");
    let mut s = State::default();
    s.kept.insert(hash("ssh-agent -s > /dev/null"));
    s.save(&p).unwrap();
    run_once(&p).unwrap();
    assert!(State::load(&p).unwrap().pending.is_empty());
}

#[test]
fn missing_watched_file_is_skipped() {
    let p = setup("missing");
    fs::remove_file(p.home.join(".bash_profile")).unwrap();
    run_once(&p).unwrap();
    assert!(!p.home.join(".bash_profile").exists());
}

#[test]
fn edited_comment_lines_untouched() {
    let p = setup("comments");
    let edited = skel(".bashrc").replace(
        "# User specific aliases and functions",
        "# User specific config: aliases, exports, functions, tools",
    );
    fs::write(p.home.join(".bashrc"), &edited).unwrap();
    run_once(&p).unwrap();
    assert_eq!(read(&p, ".bashrc"), edited);
    let s = State::load(&p).unwrap();
    assert!(s.pending.is_empty() && s.messages.is_empty());
}

#[test]
fn real_edited_bash_profile_with_installer_append() {
    // The user's actual ~/.bash_profile: comment edited, last 2 default lines deleted.
    let p = setup("realprofile");
    let users = "# .bash_profile\n\n# Everything lives in ~/.bashrc and ~/.bashrc.d/\nif [ -f ~/.bashrc ]; then\n    . ~/.bashrc\nfi\n";
    fs::write(p.home.join(".bash_profile"), format!("{users}\n{BUN}")).unwrap();
    run_once(&p).unwrap();
    assert_eq!(read(&p, ".bash_profile"), users);
    assert!(read(&p, ".bashrc.d/tools.sh").contains(BUN.trim_end()));
    assert!(State::load(&p).unwrap().pending.is_empty());
}
