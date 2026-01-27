use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::{AppSpec, ContainerSpec, CreateAppRequest, UpdateAppRequest};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

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
    let response = client.list_apps(org_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.apps)?);
    } else {
        if response.apps.is_empty() {
            println!("No applications found.");
            return Ok(());
        }

        println!(
            "{:<36}  {:<20}  {:<20}",
            "ID".bold(),
            "NAME".bold(),
            "CREATED".bold()
        );
        println!("{}", "-".repeat(78));

        for app in response.apps {
            println!(
                "{:<36}  {:<20}  {:<20}",
                app.id,
                app.name,
                app.created_at.format("%Y-%m-%d %H:%M")
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

    let client = QuomeClient::new(Some(&token), None)?;

    let spec = AppSpec {
        containers: vec![ContainerSpec {
            name: args.name.clone(),
            image: args.image,
            port: args.port,
        }],
    };

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

    if args.json {
        println!("{}", serde_json::to_string_pretty(&app)?);
    } else {
        println!("{} Created application:", "Success!".green().bold());
        println!("  {} {}", "ID:".dimmed(), app.id);
        println!("  {} {}", "Name:".dimmed(), app.name);
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
    let app = client.get_app(org_id, app_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&app)?);
    } else {
        println!("{}", "Application".bold());
        println!("  {} {}", "ID:".dimmed(), app.id);
        println!("  {} {}", "Name:".dimmed(), app.name);
        if let Some(ref desc) = app.description {
            println!("  {} {}", "Description:".dimmed(), desc);
        }
        println!(
            "  {} {}",
            "Created:".dimmed(),
            app.created_at.format("%Y-%m-%d %H:%M:%S")
        );

        if let Some(ref spec) = app.spec {
            if !spec.containers.is_empty() {
                println!();
                println!("  {}", "Containers:".bold());
                for container in &spec.containers {
                    println!("    {} {}", "-".dimmed(), container.name);
                    println!("      {} {}", "Image:".dimmed(), container.image);
                    println!("      {} {}", "Port:".dimmed(), container.port);
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

    if args.json {
        println!("{}", serde_json::to_string_pretty(&app)?);
    } else {
        println!("{} Updated application:", "Success!".green().bold());
        println!("  {} {}", "ID:".dimmed(), app.id);
        println!("  {} {}", "Name:".dimmed(), app.name);
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
    client.delete_app(org_id, args.id).await?;

    println!(
        "{} Deleted application {}",
        "Success!".green().bold(),
        args.id
    );

    Ok(())
}
