mod api;
mod client;
mod commands;
mod config;
mod errors;
mod settings;
mod ui;

use clap::Parser;
use colored::Colorize;

const BANNER: &str = r#"
   ██████╗ ██╗   ██╗ ██████╗ ███╗   ███╗███████╗
  ██╔═══██╗██║   ██║██╔═══██╗████╗ ████║██╔════╝
  ██║   ██║██║   ██║██║   ██║██╔████╔██║█████╗
  ██║▄▄ ██║██║   ██║██║   ██║██║╚██╔╝██║██╔══╝
  ╚██████╔╝╚██████╔╝╚██████╔╝██║ ╚═╝ ██║███████╗
   ╚══▀▀═╝  ╚═════╝  ╚═════╝ ╚═╝     ╚═╝╚══════╝
"#;

#[derive(Parser)]
#[command(name = "quome")]
#[command(about = "CLI for the Quome platform")]
#[command(version)]
#[command(before_help = BANNER)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Login to Quome
    Login(commands::login::Args),
    /// Logout from Quome
    Logout(commands::logout::Args),
    /// Show current user info
    Whoami(commands::whoami::Args),
    /// Link current directory to an org and app
    Link(commands::link::Args),
    /// Unlink current directory
    Unlink(commands::unlink::Args),
    /// Manage organizations
    Orgs {
        #[command(subcommand)]
        command: commands::orgs::OrgsCommands,
    },
    /// Manage organization members
    Members {
        #[command(subcommand)]
        command: commands::members::MembersCommands,
    },
    /// Manage applications
    Apps {
        #[command(subcommand)]
        command: commands::apps::AppsCommands,
    },
    /// Manage deployments
    Deployments {
        #[command(subcommand)]
        command: commands::deployments::DeploymentsCommands,
    },
    /// Manage databases
    #[command(name = "db")]
    Databases {
        #[command(subcommand)]
        command: commands::databases::DatabasesCommands,
    },
    /// View application logs
    Logs(commands::logs::Args),
    /// Manage secrets
    Secrets {
        #[command(subcommand)]
        command: commands::secrets::SecretsCommands,
    },
    /// Manage API keys
    Keys {
        #[command(subcommand)]
        command: commands::keys::KeysCommands,
    },
    /// View organization events
    Events(commands::events::Args),
    /// AI-powered app building agent
    Agent {
        #[command(subcommand)]
        command: commands::agent::AgentCommands,
    },
    /// Upgrade quome to the latest version
    Upgrade,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Login(args) => commands::login::execute(args).await,
        Commands::Logout(args) => commands::logout::execute(args).await,
        Commands::Whoami(args) => commands::whoami::execute(args).await,
        Commands::Link(args) => commands::link::execute(args).await,
        Commands::Unlink(args) => commands::unlink::execute(args).await,
        Commands::Orgs { command } => commands::orgs::execute(command).await,
        Commands::Members { command } => commands::members::execute(command).await,
        Commands::Apps { command } => commands::apps::execute(command).await,
        Commands::Deployments { command } => commands::deployments::execute(command).await,
        Commands::Databases { command } => commands::databases::execute(command).await,
        Commands::Logs(args) => commands::logs::execute(args).await,
        Commands::Secrets { command } => commands::secrets::execute(command).await,
        Commands::Keys { command } => commands::keys::execute(command).await,
        Commands::Events(args) => commands::events::execute(args).await,
        Commands::Agent { command } => commands::agent::execute(command).await,
        Commands::Upgrade => commands::upgrade::execute().await,
    };

    if let Err(e) = result {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }
}
