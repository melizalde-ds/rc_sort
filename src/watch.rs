use crate::sort::run_once;
use crate::shell::Shell;
use crate::state::Paths;
use anyhow::{Context, Result};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::sync::mpsc;
use std::time::Duration;

/// The service: one pass now, then one pass per debounced change to a watched file.
/// Watches the home dir (not the files) so replace-by-rename is seen too.
/// Returns only if the watcher itself fails.
pub fn run_watch(p: &Paths) -> Result<()> {
    pass(p);
    let (tx, rx) = mpsc::channel();
    let mut debouncer = new_debouncer(Duration::from_secs(1), tx).context("starting file watcher")?;
    let watched: Vec<_> = Shell::ALL.iter().flat_map(|s| s.watched()).map(|w| p.home.join(w)).collect();
    let mut dirs: Vec<_> = watched.iter().filter_map(|w| w.parent()).filter(|d| d.is_dir()).collect();
    dirs.dedup();
    for dir in dirs {
        debouncer
            .watcher()
            .watch(dir, RecursiveMode::NonRecursive)
            .with_context(|| format!("watching {}", dir.display()))?;
    }
    for batch in rx {
        match batch {
            Ok(events) => {
                let ours = events.iter().any(|e| watched.contains(&e.path));
                // Our own rewrite triggers one more pass, which finds nothing to do.
                if ours {
                    pass(p);
                }
            }
            Err(e) => {
                // Events may be lost (e.g. queue overflow): check the files anyway.
                eprintln!("rc-sort: watch error: {e}");
                pass(p);
            }
        }
    }
    anyhow::bail!("file watcher stopped")
}

/// A failed pass is logged (journal) and the service keeps running.
fn pass(p: &Paths) {
    if let Err(e) = run_once(p) {
        eprintln!("rc-sort: {e:#}");
    }
}
