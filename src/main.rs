use anyhow::{Context, Result};
use bashrc_sort::prompt::run_prompt;
use bashrc_sort::sort::run_once;
use bashrc_sort::state::Paths;
use bashrc_sort::watch::run_watch;
use std::path::PathBuf;

fn main() -> Result<()> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let p = Paths { home: PathBuf::from(home), skel: PathBuf::from("/etc/skel") };
    match std::env::args().nth(1).as_deref() {
        Some("watch") => run_watch(&p),
        Some("once") => run_once(&p),
        Some("prompt") => run_prompt(&p, &mut std::io::stdin().lock(), &mut std::io::stdout()),
        _ => {
            eprintln!("usage: bashrc-sort watch|prompt|once");
            std::process::exit(2);
        }
    }
}
