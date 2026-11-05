use crate::sort::run_once;
use crate::state::{Paths, WATCHED};
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
    debouncer
        .watcher()
        .watch(&p.home, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", p.home.display()))?;
    for batch in rx {
        match batch {
            Ok(events) => {
                let ours = events.iter().any(|e| {
                    e.path.file_name().is_some_and(|n| WATCHED.iter().any(|w| n == *w))
                });
                // Our own rewrite triggers one more pass, which finds nothing to do.
                if ours {
                    pass(p);
                }
            }
            Err(e) => {
                // Events may be lost (e.g. queue overflow): check the files anyway.
                eprintln!("bashrc-sort: watch error: {e}");
                pass(p);
            }
        }
    }
    anyhow::bail!("file watcher stopped")
}

/// A failed pass is logged (journal) and the service keeps running.
fn pass(p: &Paths) {
    if let Err(e) = run_once(p) {
        eprintln!("bashrc-sort: {e:#}");
    }
}
