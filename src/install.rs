//! `rc-sort install`: writes the shell hook and the systemd user unit, then (re)starts
//! the service. Safe to run again after `cargo install` of a new version.
//! `rc-sort uninstall` undoes that. Your sorted files and the backups stay.

use crate::shell::Shell;
use crate::sort::{FISH_HOOK, HOOK};
use crate::state::{atomic_write, read_opt, Paths};
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

const HOOK_TEXT: &str = include_str!("../dist/zz-rc-sort.sh");
const FISH_HOOK_TEXT: &str = include_str!("../dist/zz-rc-sort.fish");
const UNIT_TEXT: &str = include_str!("../dist/rc-sort.service");
pub const UNIT: &str = "rc-sort.service";

/// Where `cargo install` puts the binary; the hook and the unit both run it from here.
pub fn bin_path(p: &Paths) -> PathBuf {
    p.home.join(".cargo/bin/rc-sort")
}

pub fn unit_path(p: &Paths) -> PathBuf {
    p.home.join(".config/systemd/user").join(UNIT)
}

fn fish_hook_path(p: &Paths) -> PathBuf {
    Shell::Fish.rc_dir(p).join("conf.d").join(FISH_HOOK)
}

/// Writes the hook and the unit if they differ. Returns one line per file written.
pub fn install_files(p: &Paths) -> Result<Vec<String>> {
    let mut done = Vec::new();
    let mut files = vec![(unit_path(p), UNIT_TEXT)];
    match Shell::Bash.skip_reason(p)? {
        Some(why) => done.push(format!("rc-sort: {why}")),
        None => files.insert(0, (p.rc_dir().join(HOOK), HOOK_TEXT)),
    }
    if Shell::Fish.rc_dir(p).is_dir() {
        files.push((fish_hook_path(p), FISH_HOOK_TEXT));
    }
    for (path, text) in files {
        if read_opt(&path)?.as_deref() != Some(text) {
            atomic_write(&path, text)?;
            done.push(format!("rc-sort: wrote {}", path.display()));
        }
    }
    Ok(done)
}

pub fn run_install(p: &Paths, out: &mut impl Write) -> Result<()> {
    let bin = bin_path(p);
    let me = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok());
    if bin.canonicalize().ok().is_none_or(|b| Some(b) != me) {
        bail!(
            "run this from {} (the service and hook use that copy).\n  \
             From the repo: cargo install --path . && rc-sort install",
            bin.display()
        );
    }
    for line in install_files(p)? {
        writeln!(out, "{line}")?;
    }
    for args in [&["daemon-reload"][..], &["enable", UNIT], &["restart", UNIT]] {
        systemctl(args)?;
    }
    writeln!(out, "rc-sort: service running (logs: journalctl --user -u rc-sort)")?;
    if let Ok(names) = crate::tidy::unsorted(p)
        && !names.is_empty()
    {
        writeln!(out, "rc-sort: not sorted yet: {}; run: rc-sort tidy", names.join(", "))?;
    }
    writeln!(out, "rc-sort: open a new terminal to see its messages")?;
    Ok(())
}

/// Removes the hook and the unit if present. Returns one line per file removed.
pub fn uninstall_files(p: &Paths) -> Result<Vec<String>> {
    let mut done = Vec::new();
    for path in [p.rc_dir().join(HOOK), fish_hook_path(p), unit_path(p)] {
        match std::fs::remove_file(&path) {
            Ok(()) => done.push(format!("rc-sort: removed {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
        }
    }
    Ok(done)
}

pub fn run_uninstall(p: &Paths, out: &mut impl Write) -> Result<()> {
    // Stop first, while systemd still knows the unit; skip if it was never installed.
    if unit_path(p).exists() {
        systemctl(&["disable", "--now", UNIT])?;
    }
    let removed = uninstall_files(p)?;
    systemctl(&["daemon-reload"])?;
    if removed.is_empty() {
        writeln!(out, "rc-sort: service and hook were not installed")?;
    }
    for line in removed {
        writeln!(out, "{line}")?;
    }
    writeln!(out, "rc-sort: your ~/.bashrc.d files are untouched")?;
    writeln!(out, "rc-sort: backups kept in {}", p.state_dir().join("backups").display())?;
    writeln!(out, "rc-sort: to remove the binary too: cargo uninstall rc-sort")?;
    Ok(())
}

fn systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .context("running systemctl (is systemd available?)")?;
    if !status.success() {
        bail!("systemctl --user {} failed ({status})", args.join(" "));
    }
    Ok(())
}
