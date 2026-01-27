use clap::Parser;
use colored::Colorize;

use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(args: Args) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;
    let user = client.get_current_user().await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&user)?);
    } else {
        println!("{}", "Current User".bold());
        println!("  {} {}", "ID:".dimmed(), user.id);
        println!("  {} {}", "Username:".dimmed(), user.username);
        println!("  {} {}", "Email:".dimmed(), user.email);

        // Show linked context if any
        if let Some(linked) = config.get_linked()? {
            println!();
            println!("{}", "Linked Context".bold());
            println!("  {} {}", "Organization:".dimmed(), linked.org_name);
            if let Some(ref app_name) = linked.app_name {
                println!("  {} {}", "Application:".dimmed(), app_name);
            }
        }
    }

    Ok(())
}
