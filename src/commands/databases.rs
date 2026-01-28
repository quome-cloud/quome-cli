use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::{
    ComputeRequested, CreateDatabaseRequest, DatabaseCompute, DatabasePostgres, DatabaseReplicas,
    DatabaseState, DatabaseStorage, StorageRequested, UpdateDatabaseRequest,
};
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

    /// PostgreSQL major version (15, 16, or 17)
    #[arg(long, default_value = "17")]
    version: i32,

    /// Number of vCPUs
    #[arg(long, default_value = "1")]
    vcpu: String,

    /// Memory allocation (e.g., 2Gi)
    #[arg(long, default_value = "2Gi")]
    memory: String,

    /// Disk space (e.g., 1024Mi)
    #[arg(long, default_value = "1024Mi")]
    disk: String,

    /// Number of replicas
    #[arg(long, default_value = "1")]
    replicas: i32,

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

    /// New name
    #[arg(long)]
    name: Option<String>,

    /// Number of vCPUs
    #[arg(long)]
    vcpu: Option<String>,

    /// Memory allocation (e.g., 2Gi)
    #[arg(long)]
    memory: Option<String>,

    /// Disk space (e.g., 1024Mi)
    #[arg(long)]
    disk: Option<String>,

    /// Number of replicas
    #[arg(long)]
    replicas: Option<i32>,

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

fn state_color(state: &DatabaseState) -> colored::ColoredString {
    match state {
        DatabaseState::Initializing => "Initializing".yellow(),
        DatabaseState::Ready => "Ready".green(),
        DatabaseState::Paused => "Paused".dimmed(),
        DatabaseState::Stopping => "Stopping".yellow(),
        DatabaseState::Error => "Error".red(),
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
        println!("{}", serde_json::to_string_pretty(&response.databases)?);
    } else {
        if response.databases.is_empty() {
            println!("No databases found.");
            return Ok(());
        }

        let rows: Vec<DatabaseRow> = response
            .databases
            .iter()
            .map(|db| {
                let status = db
                    .status
                    .as_ref()
                    .map(|s| state_color(&s.state).to_string())
                    .unwrap_or_else(|| "-".to_string());
                DatabaseRow {
                    id: db.id.to_string(),
                    name: db.name.clone(),
                    version: format!("PG {}", db.postgres.major_version),
                    status,
                    created: db.created_at.format("%Y-%m-%d %H:%M").to_string(),
                }
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
        compute: DatabaseCompute {
            requested: ComputeRequested {
                vcpu: args.vcpu,
                memory: args.memory,
            },
        },
        storage: DatabaseStorage {
            requested: StorageRequested {
                disk_space: args.disk,
            },
        },
        replicas: DatabaseReplicas {
            requested: args.replicas,
        },
        postgres: DatabasePostgres {
            major_version: args.version,
        },
    };

    let sp = ui::spinner("Creating database...");
    let db = client.create_database(org_id, &req).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&db)?);
    } else {
        ui::print_success(
            "Created database",
            &[("ID", &db.id.to_string()), ("Name", &db.name)],
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
        let status = db
            .status
            .as_ref()
            .map(|s| state_color(&s.state).to_string())
            .unwrap_or_else(|| "-".to_string());

        let details = vec![
            ("ID", db.id.to_string()),
            ("Name", db.name.clone()),
            ("Status", status),
            ("PostgreSQL", format!("v{}", db.postgres.major_version)),
            (
                "Compute",
                format!(
                    "{} vCPU, {} memory",
                    db.compute.requested.vcpu, db.compute.requested.memory
                ),
            ),
            ("Storage", db.storage.requested.disk_space.clone()),
            ("Replicas", db.replicas.requested.to_string()),
            (
                "Created",
                db.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
            ),
            (
                "Updated",
                db.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
            ),
        ];

        let details_ref: Vec<(&str, &str)> = details.iter().map(|(k, v)| (*k, v.as_str())).collect();

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

    let compute = match (&args.vcpu, &args.memory) {
        (Some(vcpu), Some(memory)) => Some(DatabaseCompute {
            requested: ComputeRequested {
                vcpu: vcpu.clone(),
                memory: memory.clone(),
            },
        }),
        (Some(vcpu), None) => {
            // Fetch current to get memory
            let current = client.get_database(org_id, args.id).await?;
            Some(DatabaseCompute {
                requested: ComputeRequested {
                    vcpu: vcpu.clone(),
                    memory: current.compute.requested.memory,
                },
            })
        }
        (None, Some(memory)) => {
            // Fetch current to get vcpu
            let current = client.get_database(org_id, args.id).await?;
            Some(DatabaseCompute {
                requested: ComputeRequested {
                    vcpu: current.compute.requested.vcpu,
                    memory: memory.clone(),
                },
            })
        }
        (None, None) => None,
    };

    let storage = args.disk.map(|disk| DatabaseStorage {
        requested: StorageRequested { disk_space: disk },
    });

    let replicas = args.replicas.map(|r| DatabaseReplicas { requested: r });

    let req = UpdateDatabaseRequest {
        name: args.name,
        compute,
        storage,
        replicas,
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
