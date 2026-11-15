//! `bashrc-sort install`: writes the shell hook and the systemd user unit, then (re)starts
//! the service. Safe to run again after `cargo install` of a new version.

use crate::sort::HOOK;
use crate::state::{atomic_write, read_opt, Paths};
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

const HOOK_TEXT: &str = include_str!("../dist/zz-bashrc-sort.sh");
const UNIT_TEXT: &str = include_str!("../dist/bashrc-sort.service");
pub const UNIT: &str = "bashrc-sort.service";

/// Where `cargo install` puts the binary; the hook and the unit both run it from here.
pub fn bin_path(p: &Paths) -> PathBuf {
    p.home.join(".cargo/bin/bashrc-sort")
}

pub fn unit_path(p: &Paths) -> PathBuf {
    p.home.join(".config/systemd/user").join(UNIT)
}

/// Writes the hook and the unit if they differ. Returns one line per file written.
pub fn install_files(p: &Paths) -> Result<Vec<String>> {
    let mut done = Vec::new();
    for (path, text) in [(p.rc_dir().join(HOOK), HOOK_TEXT), (unit_path(p), UNIT_TEXT)] {
        if read_opt(&path)?.as_deref() != Some(text) {
            atomic_write(&path, text)?;
            done.push(format!("bashrc-sort: wrote {}", path.display()));
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
             From the repo: cargo install --path . && bashrc-sort install",
            bin.display()
        );
    }
    for line in install_files(p)? {
        writeln!(out, "{line}")?;
    }
    for args in [&["daemon-reload"][..], &["enable", UNIT], &["restart", UNIT]] {
        systemctl(args)?;
    }
    writeln!(out, "bashrc-sort: service running (logs: journalctl --user -u bashrc-sort)")?;
    if let Ok(names) = crate::tidy::unsorted(p)
        && !names.is_empty()
    {
        writeln!(out, "bashrc-sort: not sorted yet in ~/.bashrc.d: {}; run: bashrc-sort tidy", names.join(", "))?;
    }
    writeln!(out, "bashrc-sort: open a new terminal to see its messages")?;
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
