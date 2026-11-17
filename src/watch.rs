use crate::sort::run_once;
use crate::shell::Shell;
use crate::state::Paths;
use anyhow::{Context, Result};
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, Debouncer};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

/// The service: one pass now, then one pass per debounced change to a watched file.
/// Watches the folders (not the files) so replace-by-rename is seen too.
/// Returns only if the watcher itself fails.
pub fn run_watch(p: &Paths) -> Result<()> {
    for shell in Shell::ALL {
        if let Ok(Some(why)) = shell.skip_reason(p) {
            eprintln!("rc-sort: {why}");
        }
    }
    let (tx, rx) = mpsc::channel();
    let mut debouncer = new_debouncer(Duration::from_secs(1), tx).context("starting file watcher")?;
    let mut watched = watched_paths(p);
    watch_dirs(&mut debouncer, &watched)?;
    // Opening a file is an event too, and every pass reads the watched files, so only a
    // changed file starts a pass. Taken before the pass: our own rewrite starts one more.
    let mut seen = snapshot(&watched);
    pass(p);
    for batch in rx {
        watched = watched_paths(p);
        if let Err(e) = watch_dirs(&mut debouncer, &watched) {
            eprintln!("rc-sort: {e:#}");
        }
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

/// The watched files and, for symlinks, the files they point to. Resolved on every batch,
/// so a file a dotfiles tool turns into a link later is followed too.
fn watched_paths(p: &Paths) -> Vec<PathBuf> {
    let mut watched: Vec<_> = Shell::ALL.iter().flat_map(|s| s.watched()).map(|w| p.home.join(w)).collect();
    let real: Vec<_> = watched.iter().filter_map(|w| std::fs::canonicalize(w).ok()).collect();
    watched.extend(real);
    watched
}

/// Watches the nearest existing folder of each file: a missing `~/.config/fish` is watched
/// for from `~/.config`. Called on every batch, so a folder deleted and made again (a
/// dotfiles restore) is watched again too.
fn watch_dirs(debouncer: &mut Debouncer<RecommendedWatcher>, files: &[PathBuf]) -> Result<()> {
    for file in files {
        let Some(dir) = file.ancestors().skip(1).find(|d| d.is_dir()) else { continue };
        debouncer
            .watcher()
            .watch(dir, RecursiveMode::NonRecursive)
            .with_context(|| format!("watching {}", dir.display()))?;
    }
    Ok(())
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
