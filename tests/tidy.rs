use rc_sort::prompt::run_prompt;
use rc_sort::sort::run_once;
use rc_sort::state::{Paths, State, hash};
use rc_sort::tidy::{run_tidy, unsorted};
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::process::Command;

// The user's real ~/.bashrc.d on 2026-11-15.
const PATHS: &str = "export PATH=\"$HOME/.cargo/bin:$PATH\"\n";
const NODE: &str = "export PATH=\"$HOME/.local/share/fnm:$PATH\"\neval \"$(fnm env --use-on-cd --shell bash)\"\n";
const PROMPT: &str = "[[ $- == *i* ]] || return\nsource ~/.bash-preexec.sh\neval \"$(starship init bash)\"\neval \"$(atuin init bash --disable-up-arrow)\"\neval \"$(zoxide init bash)\"\n";

fn skel_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skel")
}

/// Temp HOME with default rc files and the given `~/.bashrc.d` files.
fn setup(name: &str, rc: &[(&str, &str)]) -> Paths {
    let root = std::env::temp_dir().join(format!("rc-sort-tidy-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let home = root.join("home");
    fs::create_dir_all(home.join(".bashrc.d")).unwrap();
    for f in [".bashrc", ".bash_profile"] {
        fs::copy(skel_dir().join(f), home.join(f)).unwrap();
    }
    for (n, t) in rc {
        fs::write(home.join(".bashrc.d").join(n), t).unwrap();
    }
    Paths { home, skel: skel_dir(), fish_vendor_conf: Vec::new() }
}

fn rc(p: &Paths, name: &str) -> Option<String> {
    fs::read_to_string(p.rc_dir().join(name)).ok()
}

fn tidy(p: &Paths, target: Option<PathBuf>, input: &str) -> String {
    let mut out = Vec::new();
    run_tidy(p, target, &mut Cursor::new(input.as_bytes()), &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn real_layout_lands_in_tools_in_order() {
    let p = setup("real", &[("00-paths.sh", PATHS), ("10-node.sh", NODE), ("20-prompt.sh", PROMPT)]);
    let out = tidy(&p, None, "y\n");
    assert!(out.contains("~/.bashrc.d/20-prompt.sh: source ~/.bash-preexec.sh → tools.sh (interactive only)"), "{out}");
    for f in ["00-paths.sh", "10-node.sh", "20-prompt.sh"] {
        assert_eq!(rc(&p, f), None, "{f} should be gone");
    }
    let tools = rc(&p, "tools.sh").unwrap();
    let want = format!(
        "{}\n{}\nif [[ $- == *i* ]]; then\n    source ~/.bash-preexec.sh\n    eval \"$(starship init bash)\"\n    eval \"$(atuin init bash --disable-up-arrow)\"\n    eval \"$(zoxide init bash)\"\nfi\n",
        PATHS, NODE
    );
    assert_eq!(tools, want);
    assert_eq!(fs::read_dir(p.state_dir().join("backups")).unwrap().count(), 3);
    assert!(unsorted(&p).unwrap().is_empty());
}

#[test]
fn guarded_block_does_not_hide_later_lines() {
    let guarded = "[[ $- == *i* ]] || return\nexport PATH=\"$HOME/x:$PATH\"\nexport INTER=1\n";
    let p = setup("guardrun", &[("20-prompt.sh", guarded)]);
    tidy(&p, None, "y\n");
    // The service later appends more tool setup below the wrapped block.
    let tools = p.rc_dir().join("tools.sh");
    let mut t = fs::read_to_string(&tools).unwrap();
    t.push_str("\nexport AFTER=1\n");
    fs::write(&tools, t).unwrap();
    let run = |flag: &str| {
        let o = Command::new("bash")
            .args([flag, &format!(". '{}'; echo \"$INTER/$AFTER\"", tools.display())])
            .env("HOME", &p.home)
            .output()
            .unwrap();
        String::from_utf8(o.stdout).unwrap().trim().to_string()
    };
    assert_eq!(run("-c"), "/1", "non-interactive: skips the guarded part, keeps the rest");
    assert_eq!(run("-ic"), "1/1");
}

#[test]
fn splits_into_all_four_files() {
    let mixed = "alias ll='ls -l'\n\nexport EDITOR=nano\n\nmkcd() {\n  mkdir -p \"$1\" && cd \"$1\"\n}\n\n. \"$HOME/.cargo/env\"\n";
    let p = setup("four", &[("99-misc.sh", mixed)]);
    tidy(&p, None, "y\n");
    assert_eq!(rc(&p, "aliases.sh").unwrap(), "alias ll='ls -l'\n");
    assert_eq!(rc(&p, "exports.sh").unwrap(), "export EDITOR=nano\n");
    assert!(rc(&p, "functions.sh").unwrap().starts_with("mkcd() {"));
    assert_eq!(rc(&p, "tools.sh").unwrap(), ". \"$HOME/.cargo/env\"\n");
    assert_eq!(rc(&p, "99-misc.sh"), None);
}

#[test]
fn not_confirming_changes_nothing() {
    let p = setup("no", &[("10-node.sh", NODE)]);
    let out = tidy(&p, None, "n\n");
    assert!(out.contains("nothing changed"));
    assert_eq!(rc(&p, "10-node.sh").unwrap(), NODE);
    assert_eq!(rc(&p, "tools.sh"), None);
    // EOF (no terminal) is a no too.
    tidy(&p, None, "");
    assert_eq!(rc(&p, "10-node.sh").unwrap(), NODE);
}

#[test]
fn exact_duplicate_in_another_file_is_dropped() {
    let p = setup("dup", &[("tools.sh", NODE), ("10-node.sh", NODE)]);
    let out = tidy(&p, None, "y\n");
    assert!(out.contains("already in tools.sh"), "{out}");
    assert_eq!(rc(&p, "tools.sh").unwrap(), NODE);
    assert_eq!(rc(&p, "10-node.sh"), None);
}

#[test]
fn block_shared_by_two_unsorted_files_is_kept_once() {
    // Arrange
    let p = setup("shared", &[("old.sh", "export EDITOR=nano\n"), ("work.sh", "export EDITOR=nano\n")]);

    // Act
    tidy(&p, None, "e\ny\n");

    // Assert
    assert_eq!(rc(&p, "exports.sh").as_deref(), Some("export EDITOR=nano\n"));
}

#[test]
fn half_typed_alias_is_asked() {
    // Arrange
    let p = setup("syntax", &[("old.sh", "alias gs='git sta\n")]);

    // Act
    let out = tidy(&p, None, "\n");

    // Assert
    assert!(out.contains("bash -n finds a syntax error in it"), "{out}");
}

#[test]
fn half_typed_alias_stays_when_answered() {
    // Arrange
    let p = setup("syntax-answer", &[("old.sh", "alias gs='git sta\n")]);

    // Act
    tidy(&p, None, "a\ny\n");

    // Assert
    assert_eq!(rc(&p, "aliases.sh"), None);
}

#[test]
fn same_tool_elsewhere_is_asked_and_can_be_kept() {
    let p = setup("clash", &[("tools.sh", ". \"$HOME/.cargo/env\"\n"), ("00-paths.sh", PATHS)]);
    let out = tidy(&p, None, "k\ny\n");
    assert!(out.contains("tools.sh already has a cargo block"), "{out}");
    assert!(out.contains("kept 1 block(s) in place"), "{out}");
    assert_eq!(rc(&p, "00-paths.sh").unwrap(), PATHS);
    assert!(State::load(&p).unwrap().kept.contains(&hash(PATHS)));
    assert!(unsorted(&p).unwrap().is_empty(), "kept blocks stop the warning");
    assert!(!tidy(&p, None, "").contains("cargo"), "not asked again");
}

#[test]
fn declined_keep_is_not_saved() {
    let p = setup("nokeep", &[("tools.sh", ". \"$HOME/.cargo/env\"\n"), ("00-paths.sh", PATHS)]);
    let out = tidy(&p, None, "k\nn\n");
    assert!(out.contains("nothing changed"), "{out}");
    assert!(State::load(&p).unwrap().kept.is_empty(), "keep is saved only on apply");
    assert!(tidy(&p, None, "").contains("cargo"), "asked again next time");
}

#[test]
fn mid_file_return_is_left_alone() {
    let odd = "export A=1\n[ -z \"$PS1\" ] && return\nalias x=y\n";
    let p = setup("ret", &[("50-odd.sh", odd)]);
    let out = tidy(&p, None, "y\n");
    assert!(out.contains("left ~/.bashrc.d/50-odd.sh alone"), "{out}");
    assert_eq!(rc(&p, "50-odd.sh").unwrap(), odd);
}

#[test]
fn kept_unsure_block_keeps_the_file() {
    let f = "ssh-agent -s > /dev/null\n\nalias g=git\n";
    let p = setup("partial", &[("30-x.sh", f)]);
    tidy(&p, None, "k\ny\n");
    assert_eq!(rc(&p, "30-x.sh").unwrap(), "ssh-agent -s > /dev/null\n");
    assert_eq!(rc(&p, "aliases.sh").unwrap(), "alias g=git\n");
    assert!(unsorted(&p).unwrap().is_empty());
}

#[test]
fn single_file_outside_bashrc_d() {
    let p = setup("outside", &[]);
    let f = p.home.join(".bash_aliases");
    fs::write(&f, "alias g=git\n").unwrap();
    tidy(&p, Some(f.clone()), "y\n");
    assert_eq!(rc(&p, "aliases.sh").unwrap(), "alias g=git\n");
    assert_eq!(fs::read_to_string(&f).unwrap(), "", "emptied, not deleted: it isn't ours");
}

#[test]
fn refuses_watched_files() {
    let p = setup("watched", &[]);
    let r = run_tidy(&p, Some(p.home.join(".bashrc")), &mut Cursor::new(&b"y\n"[..]), &mut Vec::new());
    assert!(r.unwrap_err().to_string().contains("rc-sort once"));
}

#[test]
fn symlinked_rc_file_is_not_its_own_duplicate() {
    let p = setup("link", &[]);
    let real = p.home.join("dotfiles-node.sh");
    fs::write(&real, NODE).unwrap();
    std::os::unix::fs::symlink(&real, p.rc_dir().join("10-node.sh")).unwrap();
    tidy(&p, None, "y\n");
    assert_eq!(rc(&p, "tools.sh").unwrap(), NODE);
    assert_eq!(fs::read_to_string(&real).unwrap(), "", "link target emptied, link kept");
}

// ---- cross-file checks in the service pass ----

#[test]
fn installer_block_already_in_numbered_file_is_removed() {
    let p = setup("svcdup", &[("10-node.sh", NODE)]);
    let mut b = fs::read_to_string(p.home.join(".bashrc")).unwrap();
    b.push_str(NODE);
    fs::write(p.home.join(".bashrc"), b).unwrap();
    run_once(&p).unwrap();
    assert_eq!(
        fs::read_to_string(p.home.join(".bashrc")).unwrap(),
        fs::read_to_string(skel_dir().join(".bashrc")).unwrap()
    );
    assert_eq!(rc(&p, "tools.sh"), None, "not copied again");
    let s = State::load(&p).unwrap();
    assert!(s.messages[0].contains("already in 10-node.sh"), "{:?}", s.messages);
}

#[test]
fn installer_same_tool_as_numbered_file_is_asked() {
    let p = setup("svcclash", &[("10-node.sh", NODE)]);
    let mut b = fs::read_to_string(p.home.join(".bashrc")).unwrap();
    b.push_str("# fnm\nFNM_PATH=\"$HOME/.local/share/fnm\"\nif [ -d \"$FNM_PATH\" ]; then\n  export PATH=\"$FNM_PATH:$PATH\"\n  eval \"`fnm env`\"\nfi\n");
    fs::write(p.home.join(".bashrc"), b).unwrap();
    run_once(&p).unwrap();
    let s = State::load(&p).unwrap();
    assert_eq!(s.pending.len(), 1);
    assert_eq!(s.pending[0].note, "10-node.sh already has a fnm block");
}

#[test]
fn hook_warns_about_unsorted_files() {
    let p = setup("warn", &[("10-node.sh", NODE), ("tools.sh", "# t\n")]);
    let mut out = Vec::new();
    run_prompt(&p, &mut Cursor::new(&b""[..]), &mut out).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "rc-sort: not sorted yet: ~/.bashrc.d/10-node.sh (run: rc-sort tidy)\n"
    );
    assert!(!p.state_dir().exists(), "warning stays read-only");
}

// ---- install ----

#[test]
fn install_writes_hook_and_unit_once() {
    use rc_sort::install::{install_files, unit_path};
    let p = setup("install", &[]);
    let first = install_files(&p).unwrap();
    assert_eq!(first.len(), 2);
    let hook = rc(&p, "zz-rc-sort.sh").unwrap();
    assert!(hook.contains("rc-sort\" prompt"));
    assert!(fs::read_to_string(unit_path(&p)).unwrap().contains("ExecStart=%h/.cargo/bin/rc-sort watch"));
    assert!(install_files(&p).unwrap().is_empty(), "second run changes nothing");
    assert!(unsorted(&p).unwrap().is_empty(), "the hook itself is never 'unsorted'");
}

#[test]
fn uninstall_removes_only_hook_and_unit() {
    use rc_sort::install::{install_files, uninstall_files, unit_path};
    let p = setup("uninstall", &[("tools.sh", NODE)]);
    install_files(&p).unwrap();
    assert_eq!(uninstall_files(&p).unwrap().len(), 2);
    assert_eq!(rc(&p, "zz-rc-sort.sh"), None);
    assert!(!unit_path(&p).exists());
    assert_eq!(rc(&p, "tools.sh").unwrap(), NODE, "sorted files stay");
    assert!(uninstall_files(&p).unwrap().is_empty(), "second run is a no-op");
}

fn arch_setup(name: &str) -> Paths {
    let skel = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skel-arch");
    let home = std::env::temp_dir().join(format!("rc-sort-arch-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();
    for f in [".bashrc", ".bash_profile"] {
        fs::copy(skel.join(f), home.join(f)).unwrap();
    }
    Paths { home, skel, fish_vendor_conf: Vec::new() }
}

#[test]
fn bash_is_not_sorted_where_the_default_never_loads_bashrc_d() {
    // Arrange
    let p = arch_setup("once");
    let bashrc = format!("{}\nalias g=git\n", fs::read_to_string(p.skel.join(".bashrc")).unwrap());
    fs::write(p.home.join(".bashrc"), &bashrc).unwrap();

    // Act
    run_once(&p).unwrap();

    // Assert
    assert_eq!(fs::read_to_string(p.home.join(".bashrc")).unwrap(), bashrc);
    assert_eq!(rc(&p, "aliases.sh"), None);
}

#[test]
fn install_skips_the_bash_hook_where_the_default_never_loads_bashrc_d() {
    use rc_sort::install::install_files;
    // Arrange
    let p = arch_setup("install");

    // Act
    let lines = install_files(&p).unwrap();

    // Assert
    assert_eq!(rc(&p, "zz-rc-sort.sh"), None);
    assert!(lines.iter().any(|l| l.contains("never loads ~/.bashrc.d")), "{lines:?}");
}
