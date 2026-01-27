use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::CreateOrgRequest;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Subcommand)]
pub enum OrgsCommands {
    /// List all organizations
    List(ListArgs),
    /// Create a new organization
    Create(CreateArgs),
    /// Get organization details
    Get(GetArgs),
}

#[derive(Parser)]
pub struct ListArgs {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct CreateArgs {
    /// Organization name
    name: String,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct GetArgs {
    /// Organization ID (uses linked org if not provided)
    #[arg(short, long)]
    id: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(command: OrgsCommands) -> Result<()> {
    match command {
        OrgsCommands::List(args) => list(args).await,
        OrgsCommands::Create(args) => create(args).await,
        OrgsCommands::Get(args) => get(args).await,
    }
}

async fn list(args: ListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;
    let response = client.list_orgs().await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.organizations)?);
    } else {
        if response.organizations.is_empty() {
            println!("No organizations found.");
            return Ok(());
        }

        println!(
            "{:<36}  {:<20}  {:<20}",
            "ID".bold(),
            "NAME".bold(),
            "CREATED".bold()
        );
        println!("{}", "-".repeat(78));

        for org in response.organizations {
            println!(
                "{:<36}  {:<20}  {:<20}",
                org.id,
                org.name,
                org.created_at.format("%Y-%m-%d %H:%M")
            );
        }
    }

    Ok(())
}

async fn create(args: CreateArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;
    let org = client
        .create_org(&CreateOrgRequest { name: args.name })
        .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&org)?);
    } else {
        println!("{} Created organization:", "Success!".green().bold());
        println!("  {} {}", "ID:".dimmed(), org.id);
        println!("  {} {}", "Name:".dimmed(), org.name);
    }

    Ok(())
}

async fn get(args: GetArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.id {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;
    let org = client.get_org(org_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&org)?);
    } else {
        println!("{}", "Organization".bold());
        println!("  {} {}", "ID:".dimmed(), org.id);
        println!("  {} {}", "Name:".dimmed(), org.name);
        println!(
            "  {} {}",
            "Created:".dimmed(),
            org.created_at.format("%Y-%m-%d %H:%M:%S")
        );
        println!(
            "  {} {}",
            "Updated:".dimmed(),
            org.updated_at.format("%Y-%m-%d %H:%M:%S")
        );
    }

    Ok(())
}
