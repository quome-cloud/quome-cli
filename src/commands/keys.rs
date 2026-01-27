use chrono::{Duration, Utc};
use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::CreateOrgKeyRequest;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Subcommand)]
pub enum KeysCommands {
    /// List API keys
    List(ListArgs),
    /// Create a new API key
    Create(CreateArgs),
    /// Delete an API key
    Delete(DeleteArgs),
}

#[derive(Parser)]
pub struct ListArgs {
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct CreateArgs {
    /// Days until expiration (0 = never expires)
    #[arg(long, default_value = "0")]
    expires_days: u32,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct DeleteArgs {
    /// API key ID
    id: Uuid,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Skip confirmation prompt
    #[arg(short, long)]
    force: bool,
}

pub async fn execute(command: KeysCommands) -> Result<()> {
    match command {
        KeysCommands::List(args) => list(args).await,
        KeysCommands::Create(args) => create(args).await,
        KeysCommands::Delete(args) => delete(args).await,
    }
}

async fn list(args: ListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let response = client.list_org_keys(org_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.keys)?);
    } else {
        if response.keys.is_empty() {
            println!("No API keys found.");
            return Ok(());
        }

        println!(
            "{:<36}  {:<20}",
            "ID".bold(),
            "CREATED".bold()
        );
        println!("{}", "-".repeat(58));

        for key in response.keys {
            println!(
                "{:<36}  {:<20}",
                key.id,
                key.created_at.format("%Y-%m-%d %H:%M")
            );
        }
    }

    Ok(())
}

async fn create(args: CreateArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let expiration = if args.expires_days > 0 {
        Some(Utc::now() + Duration::days(args.expires_days as i64))
    } else {
        None
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let key = client
        .create_org_key(org_id, &CreateOrgKeyRequest { expiration })
        .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&key)?);
    } else {
        println!("{} Created API key:", "Success!".green().bold());
        println!("  {} {}", "ID:".dimmed(), key.id);
        println!();
        println!(
            "  {} {}",
            "Key:".yellow().bold(),
            key.key.cyan()
        );
        println!();
        println!(
            "  {}",
            "Save this key - it won't be shown again!".yellow()
        );
    }

    Ok(())
}

async fn delete(args: DeleteArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    if !args.force {
        let confirm = inquire::Confirm::new(&format!(
            "Are you sure you want to delete API key {}?",
            args.id
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| {
            crate::errors::QuomeError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            ))
        })?;

        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let client = QuomeClient::new(Some(&token), None)?;
    client.delete_org_key(org_id, args.id).await?;

    println!(
        "{} Deleted API key {}",
        "Success!".green().bold(),
        args.id
    );

    Ok(())
}
