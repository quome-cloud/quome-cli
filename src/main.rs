mod api;
mod auto_update;
mod client;
mod cloud;
mod commands;
mod config;
mod errors;
// Later commits wire the machine agent into the CLI; until then nothing calls it.
#[allow(dead_code)]
mod machine;
mod manifest;
mod settings;
mod ui;
mod url_policy;

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
    /// Deploy a directory of static files
    Deploy(commands::deploy::DeployArgs),
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
    /// View organization audit events
    Events(commands::events::Args),
    /// Functions-as-a-service: Python compiled to WebAssembly, run from a warm sandbox pool
    #[command(name = "fn", visible_alias = "functions")]
    Fn(commands::functions::FnArgs),
    /// Run a Quome sandbox host on this computer
    Host {
        #[command(subcommand)]
        command: commands::host::HostCommands,
    },
    /// Connect this computer to Quome Cloud
    Connect(commands::connect::Args),
    /// Run the Quome Cloud MCP server (stdio) for Claude Code
    Mcp(commands::mcp::Args),
    /// Keep the Quome playbook skills in your agent clients up to date
    Skills {
        #[command(subcommand)]
        command: commands::skills::SkillsCommands,
    },
    /// Serve this computer's local models to your organization
    Models {
        #[command(subcommand)]
        command: commands::models::ModelsCommands,
    },
    /// Inspect or drop this computer's Quome Cloud connection
    Cloud {
        #[command(subcommand)]
        command: commands::cloud::CloudCommands,
    },
    /// Git credential helper for the org's Git repos (Quome Cloud)
    #[command(name = "git-credential")]
    GitCredential(commands::git_credential::Args),
    /// Upgrade quome to the latest version
    Upgrade,
    /// The daily background update check (internal)
    #[command(name = "__auto-update", hide = true)]
    AutoUpdate,
}

impl Commands {
    /// `git-credential` runs inside git and `upgrade` already updates.
    fn starts_update_check(&self) -> bool {
        !matches!(
            self,
            Commands::GitCredential(_)
                | Commands::Upgrade
                | Commands::AutoUpdate
                | Commands::Models {
                    command: commands::models::ModelsCommands::Serve { .. }
                }
        )
    }

    /// `mcp` and `git-credential` are read by programs, so they never print the update line.
    fn shows_update_notice(&self) -> bool {
        !matches!(
            self,
            Commands::Mcp(_)
                | Commands::GitCredential(_)
                | Commands::AutoUpdate
                | Commands::Models {
                    command: commands::models::ModelsCommands::Serve { .. }
                }
        )
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let shows_update_notice = cli.command.shows_update_notice();
    if cli.command.starts_update_check() {
        auto_update::start_background_check();
    }

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
        Commands::Deploy(args) => commands::deploy::execute(args).await,
        Commands::Databases { command } => commands::databases::execute(command).await,
        Commands::Logs(args) => commands::logs::execute(args).await,
        Commands::Secrets { command } => commands::secrets::execute(command).await,
        Commands::Keys { command } => commands::keys::execute(command).await,
        Commands::Events(args) => commands::events::execute(args).await,
        Commands::Fn(args) => commands::functions::execute(args).await,
        Commands::Host { command } => commands::host::execute(command).await,
        Commands::Connect(args) => commands::connect::execute(args).await,
        Commands::Mcp(args) => commands::mcp::execute(args).await,
        Commands::Skills { command } => commands::skills::execute(command).await,
        Commands::Models { command } => commands::models::execute(command).await,
        Commands::Cloud { command } => commands::cloud::execute(command).await,
        Commands::GitCredential(args) => commands::git_credential::execute(args).await,
        Commands::Upgrade => commands::upgrade::execute().await,
        Commands::AutoUpdate => {
            auto_update::run().await;
            Ok(())
        }
    };

    if shows_update_notice {
        auto_update::print_notice();
    }

    if let Err(e) = result {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(args: &[&str]) -> Commands {
        let mut argv = vec!["quome"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn mcp_checks_for_updates_but_never_prints_the_notice() {
        let mcp = command(&["mcp"]);
        assert!(mcp.starts_update_check());
        assert!(!mcp.shows_update_notice());
    }

    #[test]
    fn git_credential_stays_out_of_updates() {
        let git = command(&["git-credential", "get"]);
        assert!(!git.starts_update_check());
        assert!(!git.shows_update_notice());
    }

    #[test]
    fn the_background_check_cannot_start_itself() {
        let check = command(&[auto_update::SUBCOMMAND]);
        assert!(!check.starts_update_check());
        assert!(!check.shows_update_notice());
    }

    #[test]
    fn an_ordinary_command_checks_and_can_show_the_notice() {
        let whoami = command(&["whoami"]);
        assert!(whoami.starts_update_check());
        assert!(whoami.shows_update_notice());
    }

    #[test]
    fn serve_has_no_update_check() {
        let serve = command(&["models", "serve"]);
        assert!(!serve.starts_update_check());
        assert!(!serve.shows_update_notice());
        // The other machine commands are ordinary.
        for args in [&["models", "status"][..], &["models", "disconnect"]] {
            let c = command(args);
            assert!(c.starts_update_check() && c.shows_update_notice());
        }
        // Every pre-existing predicate is unchanged.
        let table: [(&[&str], bool, bool); 6] = [
            (&["mcp"], true, false),
            (&["git-credential", "get"], false, false),
            (&["__auto-update"], false, false),
            (&["whoami"], true, true),
            (&["upgrade"], false, true),
            (&["connect"], true, true),
        ];
        for (args, starts, shows) in table {
            let c = command(args);
            assert_eq!(
                (c.starts_update_check(), c.shows_update_notice()),
                (starts, shows),
                "{args:?}"
            );
        }
    }
}
