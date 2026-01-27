use clap::{Parser, Subcommand};
use uuid::Uuid;

use crate::api::models::CreateOrgRequest;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui::{self, OrgRow};

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

    let sp = ui::spinner("Fetching organizations...");
    let response = client.list_orgs().await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.organizations)?);
    } else {
        if response.organizations.is_empty() {
            println!("No organizations found.");
            return Ok(());
        }

        let rows: Vec<OrgRow> = response
            .organizations
            .iter()
            .map(|org| OrgRow {
                id: org.id.to_string(),
                name: org.name.clone(),
                created: org.created_at.format("%Y-%m-%d %H:%M").to_string(),
            })
            .collect();

        ui::print_table(rows);
    }

    Ok(())
}

async fn create(args: CreateArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Creating organization...");
    let org = client
        .create_org(&CreateOrgRequest { name: args.name })
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&org)?);
    } else {
        ui::print_success("Created organization", &[
            ("ID", &org.id.to_string()),
            ("Name", &org.name),
        ]);
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

    let sp = ui::spinner("Fetching organization...");
    let org = client.get_org(org_id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&org)?);
    } else {
        ui::print_detail(&org.name, &[
            ("ID", &org.id.to_string()),
            ("Name", &org.name),
            ("Created", &org.created_at.format("%Y-%m-%d %H:%M:%S").to_string()),
            ("Updated", &org.updated_at.format("%Y-%m-%d %H:%M:%S").to_string()),
        ]);
    }

    Ok(())
}
