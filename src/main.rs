mod cmd;

use std::ffi::OsStr;
use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ghbrk", about = "Privilege-separated git/gh broker")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the broker daemon
    Daemon,
    /// Invoke git through the broker (shim mode)
    Git {
        /// Arguments forwarded to git
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Invoke gh through the broker (shim mode)
    Gh {
        /// Arguments forwarded to gh
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() -> ExitCode {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("failed to install rustls crypto provider");
    let mut all_args: Vec<String> = std::env::args().collect();

    let argv0 = std::env::args_os().next().unwrap_or_default();
    let basename = Path::new(&argv0)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("");

    match basename {
        "git" => {
            let forwarded: Vec<String> = all_args.drain(1..).collect();
            cmd::git::run(&forwarded)
        }
        "gh" => {
            let forwarded: Vec<String> = all_args.drain(1..).collect();
            cmd::gh::run(&forwarded)
        }
        _ => {
            let cli = Cli::parse();
            match cli.command {
                Commands::Daemon => cmd::daemon::run(),
                Commands::Git { args } => cmd::git::run(&args),
                Commands::Gh { args } => cmd::gh::run(&args),
            }
        }
    }
}
