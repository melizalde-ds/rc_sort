use crate::sort::run_once;
use crate::shell::Shell;
use crate::state::Paths;
use anyhow::{Context, Result};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

/// The service: one pass now, then one pass per debounced change to a watched file.
/// Watches the home dir (not the files) so replace-by-rename is seen too.
/// Returns only if the watcher itself fails.
pub fn run_watch(p: &Paths) -> Result<()> {
    for shell in Shell::ALL {
        if let Ok(Some(why)) = shell.skip_reason(p) {
            eprintln!("rc-sort: {why}");
        }
    }
    let (tx, rx) = mpsc::channel();
    let mut debouncer = new_debouncer(Duration::from_secs(1), tx).context("starting file watcher")?;
    let mut watched: Vec<_> = Shell::ALL.iter().flat_map(|s| s.watched()).map(|w| p.home.join(w)).collect();
    let real: Vec<_> = watched.iter().filter_map(|w| std::fs::canonicalize(w).ok()).collect();
    watched.extend(real);
    let mut dirs: Vec<_> = watched.iter().filter_map(|w| w.parent()).filter(|d| d.is_dir()).collect();
    dirs.sort();
    dirs.dedup();
    for dir in dirs {
        debouncer
            .watcher()
            .watch(dir, RecursiveMode::NonRecursive)
            .with_context(|| format!("watching {}", dir.display()))?;
    }
    // Opening a file is an event too, and every pass reads the watched files, so only a
    // changed file starts a pass. Taken before the pass: our own rewrite starts one more.
    let mut seen = snapshot(&watched);
    pass(p);
    for batch in rx {
        let now = snapshot(&watched);
        match batch {
            // Events may be lost (e.g. queue overflow): check the files anyway.
            Err(e) => eprintln!("rc-sort: watch error: {e}"),
            Ok(_) if now == seen => continue,
            Ok(_) => {}
        }
        seen = now;
        pass(p);
    }
    anyhow::bail!("file watcher stopped")
}

fn snapshot(files: &[PathBuf]) -> Vec<Option<(SystemTime, u64)>> {
    files
        .iter()
        .map(|f| std::fs::metadata(f).ok().and_then(|m| Some((m.modified().ok()?, m.len()))))
        .collect()
}

/// A failed pass is logged (journal) and the service keeps running.
fn pass(p: &Paths) {
    if let Err(e) = run_once(p) {
        eprintln!("rc-sort: {e:#}");
    }
}
