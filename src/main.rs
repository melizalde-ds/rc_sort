use anyhow::{Context, Result};
use bashrc_sort::install::{run_install, run_uninstall};
use bashrc_sort::prompt::run_prompt;
use bashrc_sort::sort::run_once;
use bashrc_sort::state::Paths;
use bashrc_sort::tidy::run_tidy;
use bashrc_sort::watch::run_watch;
use std::path::PathBuf;

fn main() -> Result<()> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let p = Paths { home: PathBuf::from(home), skel: PathBuf::from("/etc/skel") };
    match std::env::args().nth(1).as_deref() {
        Some("watch") => run_watch(&p),
        Some("once") => run_once(&p),
        Some("install") => run_install(&p, &mut std::io::stdout()),
        Some("uninstall") => run_uninstall(&p, &mut std::io::stdout()),
        Some("tidy") => {
            let (flags, rest): (Vec<_>, Vec<_>) = std::env::args_os().skip(2).partition(|a| a == "--plain");
            let target = rest.into_iter().next().map(PathBuf::from);
            #[cfg(feature = "tui")]
            if flags.is_empty() && bashrc_sort::tidy_tui::usable() {
                return bashrc_sort::tidy_tui::run(&p, target, &mut std::io::stdout());
            }
            let _ = flags;
            run_tidy(&p, target, &mut std::io::stdin().lock(), &mut std::io::stdout())
        }
        Some("prompt") => run_prompt(&p, &mut std::io::stdin().lock(), &mut std::io::stdout()),
        _ => {
            eprintln!("usage: bashrc-sort install|uninstall|tidy [--plain] [file-or-dir]|once|watch|prompt");
            std::process::exit(2);
        }
    }
}
