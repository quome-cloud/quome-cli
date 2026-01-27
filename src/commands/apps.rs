use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::{AppSpec, ContainerSpec, CreateAppRequest, UpdateAppRequest};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui::{self, AppRow};

#[derive(Subcommand)]
pub enum AppsCommands {
    /// List all applications
    List(ListArgs),
    /// Create a new application
    Create(CreateArgs),
    /// Get application details
    Get(GetArgs),
    /// Update an application
    Update(UpdateArgs),
    /// Delete an application
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
    /// Application name
    name: String,

    /// Application description
    #[arg(short, long)]
    description: Option<String>,

    /// Container image (e.g., nginx:latest)
    #[arg(long)]
    image: String,

    /// Container port
    #[arg(long, default_value = "80")]
    port: u16,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct GetArgs {
    /// Application ID (uses linked app if not provided)
    #[arg(short, long)]
    id: Option<Uuid>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct UpdateArgs {
    /// Application ID (uses linked app if not provided)
    #[arg(short, long)]
    id: Option<Uuid>,

    /// New name
    #[arg(long)]
    name: Option<String>,

    /// New description
    #[arg(long)]
    description: Option<String>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct DeleteArgs {
    /// Application ID
    id: Uuid,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Skip confirmation prompt
    #[arg(short, long)]
    force: bool,
}

pub async fn execute(command: AppsCommands) -> Result<()> {
    match command {
        AppsCommands::List(args) => list(args).await,
        AppsCommands::Create(args) => create(args).await,
        AppsCommands::Get(args) => get(args).await,
        AppsCommands::Update(args) => update(args).await,
        AppsCommands::Delete(args) => delete(args).await,
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

    let sp = ui::spinner("Fetching applications...");
    let response = client.list_apps(org_id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.apps)?);
    } else {
        if response.apps.is_empty() {
            println!("No applications found.");
            return Ok(());
        }

        let rows: Vec<AppRow> = response
            .apps
            .iter()
            .map(|app| AppRow {
                id: app.id.to_string(),
                name: app.name.clone(),
                created: app.created_at.format("%Y-%m-%d %H:%M").to_string(),
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

    let spec = AppSpec {
        containers: vec![ContainerSpec {
            name: args.name.clone(),
            image: args.image,
            port: args.port,
        }],
    };

    let sp = ui::spinner("Creating application...");
    let app = client
        .create_app(
            org_id,
            &CreateAppRequest {
                name: args.name,
                description: args.description,
                spec,
            },
        )
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&app)?);
    } else {
        ui::print_success("Created application", &[
            ("ID", &app.id.to_string()),
            ("Name", &app.name),
        ]);
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

    let app_id = match args.id {
        Some(id) => id,
        None => config.require_linked_app()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching application...");
    let app = client.get_app(org_id, app_id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&app)?);
    } else {
        let mut details = vec![
            ("ID", app.id.to_string()),
            ("Name", app.name.clone()),
        ];

        if let Some(ref desc) = app.description {
            details.push(("Description", desc.clone()));
        }

        details.push(("Created", app.created_at.format("%Y-%m-%d %H:%M:%S").to_string()));
        details.push(("Updated", app.updated_at.format("%Y-%m-%d %H:%M:%S").to_string()));

        let details_ref: Vec<(&str, &str)> = details
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();

        ui::print_detail(&app.name, &details_ref);

        // Show containers if any
        if let Some(ref spec) = app.spec {
            if !spec.containers.is_empty() {
                println!();
                println!("{}", "Containers".bold());
                for container in &spec.containers {
                    println!(
                        "  {} {} ({}, port {})",
                        "•".cyan(),
                        container.name.bold(),
                        container.image.dimmed(),
                        container.port
                    );
                }
            }
        }
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

    let app_id = match args.id {
        Some(id) => id,
        None => config.require_linked_app()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Updating application...");
    let app = client
        .update_app(
            org_id,
            app_id,
            &UpdateAppRequest {
                name: args.name,
                description: args.description,
                spec: None,
            },
        )
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&app)?);
    } else {
        ui::print_success("Updated application", &[
            ("ID", &app.id.to_string()),
            ("Name", &app.name),
        ]);
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
            "Are you sure you want to delete application {}?",
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

    let sp = ui::spinner("Deleting application...");
    client.delete_app(org_id, args.id).await?;
    sp.finish_and_clear();

    ui::print_success("Deleted application", &[
        ("ID", &args.id.to_string()),
    ]);

    Ok(())
}
