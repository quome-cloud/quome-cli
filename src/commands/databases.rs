use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::{CreateDatabaseRequest, UpdateDatabaseRequest};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui::{self, DatabaseRow};

#[derive(Subcommand)]
pub enum DatabasesCommands {
    /// List all databases
    List(ListArgs),
    /// Create a new database
    Create(CreateArgs),
    /// Get database details
    Get(GetArgs),
    /// Update a database
    Update(UpdateArgs),
    /// Delete a database
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
    /// Database name
    name: String,

    /// Database description
    #[arg(long)]
    description: Option<String>,

    /// PostgreSQL major version
    #[arg(long, default_value = "17")]
    version: String,

    /// Instance tier (e.g., db-f1-micro)
    #[arg(long, default_value = "db-f1-micro")]
    tier: String,

    /// Storage in GB
    #[arg(long, default_value = "10")]
    storage_gb: i32,

    /// Enable high availability
    #[arg(long)]
    ha: bool,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct GetArgs {
    /// Database ID
    id: Uuid,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct UpdateArgs {
    /// Database ID
    id: Uuid,

    /// New description
    #[arg(long)]
    description: Option<String>,

    /// New instance tier
    #[arg(long)]
    tier: Option<String>,

    /// New storage in GB
    #[arg(long)]
    storage_gb: Option<i32>,

    /// Enable or disable high availability
    #[arg(long)]
    ha: Option<bool>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct DeleteArgs {
    /// Database ID
    id: Uuid,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Skip confirmation prompt
    #[arg(short, long)]
    force: bool,
}

pub async fn execute(command: DatabasesCommands) -> Result<()> {
    match command {
        DatabasesCommands::List(args) => list(args).await,
        DatabasesCommands::Create(args) => create(args).await,
        DatabasesCommands::Get(args) => get(args).await,
        DatabasesCommands::Update(args) => update(args).await,
        DatabasesCommands::Delete(args) => delete(args).await,
    }
}

fn status_color(status: &str) -> colored::ColoredString {
    match status {
        "running" => status.green(),
        "pending" | "provisioning" | "updating" => status.yellow(),
        "failed" => status.red(),
        "stopped" | "deleting" => status.dimmed(),
        other => other.normal(),
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

    let sp = ui::spinner("Fetching databases...");
    let response = client.list_databases(org_id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.data)?);
    } else {
        if response.data.is_empty() {
            println!("No databases found.");
            return Ok(());
        }

        let rows: Vec<DatabaseRow> = response
            .data
            .iter()
            .map(|db| DatabaseRow {
                id: db.id.to_string(),
                name: db.name.clone(),
                version: format!("PG {}", db.version),
                tier: db.tier.clone(),
                status: status_color(&db.status).to_string(),
                created: db.created_at.format("%Y-%m-%d %H:%M").to_string(),
            })
            .collect();

        ui::print_table(rows);
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

    let client = QuomeClient::new(Some(&token), None)?;

    let req = CreateDatabaseRequest {
        name: args.name.clone(),
        description: args.description,
        version: args.version,
        tier: args.tier,
        storage_gb: args.storage_gb,
        ha_enabled: args.ha,
    };

    let sp = ui::spinner("Creating database...");
    let db = client.create_database(org_id, &req).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&db)?);
    } else {
        ui::print_success(
            "Created database",
            &[
                ("ID", &db.id.to_string()),
                ("Name", &db.name),
                ("Status", &db.status),
            ],
        );
    }

    Ok(())
}

async fn get(args: GetArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching database...");
    let db = client.get_database(org_id, args.id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&db)?);
    } else {
        let mut details = vec![
            ("ID", db.id.to_string()),
            ("Name", db.name.clone()),
            ("Status", status_color(&db.status).to_string()),
            ("PostgreSQL", format!("v{}", db.version)),
            ("Tier", db.tier.clone()),
            ("Storage", format!("{} GB", db.storage_gb)),
            ("HA", db.ha_enabled.to_string()),
        ];

        if let Some(ref ip) = db.private_ip {
            details.push(("Private IP", ip.clone()));
        }
        details.push((
            "Created",
            db.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        ));
        details.push((
            "Updated",
            db.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        ));

        let details_ref: Vec<(&str, &str)> =
            details.iter().map(|(k, v)| (*k, v.as_str())).collect();

        ui::print_detail(&db.name, &details_ref);
    }

    Ok(())
}

async fn update(args: UpdateArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let req = UpdateDatabaseRequest {
        description: args.description,
        tier: args.tier,
        storage_gb: args.storage_gb,
        ha_enabled: args.ha,
    };

    let sp = ui::spinner("Updating database...");
    let db = client.update_database(org_id, args.id, &req).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&db)?);
    } else {
        ui::print_success(
            "Updated database",
            &[("ID", &db.id.to_string()), ("Name", &db.name)],
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
            "Are you sure you want to delete database {}?",
            args.id
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| crate::errors::QuomeError::Io(std::io::Error::other(e.to_string())))?;

        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Deleting database...");
    client.delete_database(org_id, args.id).await?;
    sp.finish_and_clear();

    ui::print_success("Deleted database", &[("ID", &args.id.to_string())]);

    Ok(())
}
