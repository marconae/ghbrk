mod cmd;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ghbrk", version, about = "Privilege-separated git/gh broker")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the broker daemon
    Daemon,
    /// Diagnose the ghbrk environment (daemon, credentials, policy)
    Doctor,
    /// Explain how the broker would resolve and evaluate a command without running it
    Explain {
        /// Command tokens to explain (e.g. `git push origin main`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Show the allowed and forbidden operations for a repository
    Policy {
        /// Repository in `org/repo` form
        repo: String,
    },
    /// Relay a remote git operation through the broker
    Git {
        /// Arguments forwarded to git
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Relay a gh invocation through the broker
    Gh {
        /// Arguments forwarded to gh
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Grant operations or a named role on a repository to a user
    Allow {
        /// Repository in `org/repo` format
        repo: String,
        /// Operations or role name to grant
        operands: Vec<String>,
        /// Grant to this user instead of the current user
        #[arg(long)]
        user: Option<String>,
    },
}

fn main() -> ExitCode {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("failed to install rustls crypto provider");
    let cli = Cli::parse();
    match cli.command {
        Commands::Daemon => cmd::daemon::run(),
        Commands::Doctor => cmd::doctor::run(),
        Commands::Explain { args } => cmd::explain::run(&args),
        Commands::Policy { repo } => cmd::policy::run(&repo),
        Commands::Git { args } => cmd::git::run(&args),
        Commands::Gh { args } => cmd::gh::run(&args),
        Commands::Allow {
            repo,
            operands,
            user,
        } => cmd::allow::run(repo, operands, user),
    }
}
