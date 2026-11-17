use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueHint};
use clap_complete::Shell;
use rc_sort::install::{run_install, run_uninstall};
use rc_sort::prompt::run_prompt;
use rc_sort::sort::run_once;
use rc_sort::state::Paths;
use rc_sort::tidy::run_tidy;
use rc_sort::watch::run_watch;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Keeps your shell rc files sorted into tools, exports, aliases and functions"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Write the shell hook and systemd unit, then start the service")]
    Install,
    #[command(about = "Stop the service and remove the hook")]
    Uninstall,
    #[command(about = "Sort blocks already in your rc folders, or in the file you name")]
    Tidy {
        #[arg(long, help = "Plain prompts instead of the full-screen picker")]
        plain: bool,
        #[arg(value_hint = ValueHint::AnyPath, help = "File or folder to sort (default: ~/.bashrc.d and ~/.config/fish/conf.d)")]
        target: Option<PathBuf>,
    },
    #[command(about = "Run one sorting pass by hand")]
    Once,
    #[command(about = "Watch rc files and sort new lines as they appear (what the service runs)")]
    Watch,
    #[command(about = "Show moves and ask about unsure blocks (run by the shell hook)")]
    Prompt,
    #[command(about = "Print a tab-completion script for your shell")]
    Completions { shell: Shell },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let p = Paths {
        home: PathBuf::from(home),
        skel: PathBuf::from("/etc/skel"),
        // shortcut: the usual system folders, not every $XDG_DATA_DIRS entry fish reads.
        fish_vendor_conf: vec![
            PathBuf::from("/etc/fish/conf.d"),
            PathBuf::from("/usr/share/fish/vendor_conf.d"),
        ],
    };
    match cli.command {
        Command::Watch => run_watch(&p),
        Command::Once => run_once(&p),
        Command::Install => run_install(&p, &mut std::io::stdout()),
        Command::Uninstall => run_uninstall(&p, &mut std::io::stdout()),
        Command::Tidy { plain, target } => {
            #[cfg(feature = "tui")]
            if !plain && rc_sort::tidy_tui::usable() {
                return rc_sort::tidy_tui::run(&p, target, &mut std::io::stdout());
            }
            let _ = plain;
            run_tidy(
                &p,
                target,
                &mut std::io::stdin().lock(),
                &mut std::io::stdout(),
            )
        }
        Command::Prompt => run_prompt(&p, &mut std::io::stdin().lock(), &mut std::io::stdout()),
        Command::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "rc-sort",
                &mut std::io::stdout(),
            );
            Ok(())
        }
    }
}
