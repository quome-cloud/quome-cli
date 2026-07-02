use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::{AppSource, AppSpecCreate, CreateAppRequest, UpdateAppRequest};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::{QuomeError, Result};
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
    /// Application name (lowercase letters, digits, hyphens)
    name: String,

    /// Application description
    #[arg(short, long)]
    description: Option<String>,

    /// Container image (e.g., nginx:1.27) — creates an image-sourced app
    #[arg(long, conflicts_with = "repo")]
    image: Option<String>,

    /// GitHub repository as owner/name — creates a git-sourced app
    #[arg(long)]
    repo: Option<String>,

    /// Git branch (used with --repo)
    #[arg(long, default_value = "main")]
    branch: String,

    /// Container port
    #[arg(long, default_value = "8080")]
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

    /// New description
    #[arg(long)]
    description: Option<String>,

    /// New deploy branch (git-sourced apps)
    #[arg(long)]
    branch: Option<String>,

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

fn status_color(status: &str) -> colored::ColoredString {
    match status {
        "running" => status.green(),
        "pending" | "provisioning" => status.yellow(),
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

    let sp = ui::spinner("Fetching applications...");
    let response = client.list_apps(org_id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.data)?);
    } else {
        if response.data.is_empty() {
            println!("No applications found.");
            return Ok(());
        }

        let rows: Vec<AppRow> = response
            .data
            .iter()
            .map(|app| AppRow {
                id: app.id.to_string(),
                name: app.name.clone(),
                status: status_color(&app.status).to_string(),
                url: app.primary_url.clone().unwrap_or_else(|| "-".to_string()),
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

    let source = if let Some(image) = args.image {
        AppSource::Image { image_url: image }
    } else if let Some(repo) = args.repo {
        let (owner, name) = repo
            .split_once('/')
            .ok_or_else(|| QuomeError::ApiError("--repo must be in owner/name format".into()))?;
        AppSource::Git {
            repo_owner: owner.to_string(),
            repo_name: name.to_string(),
            branch: args.branch,
        }
    } else {
        return Err(QuomeError::ApiError(
            "Provide a source: --image <image:tag> or --repo <owner/name>".into(),
        ));
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Creating application...");
    let app = client
        .create_app(
            org_id,
            &CreateAppRequest {
                name: args.name,
                description: args.description,
                source,
                spec: AppSpecCreate {
                    port: Some(args.port),
                    ..Default::default()
                },
            },
        )
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&app)?);
    } else {
        ui::print_success(
            "Created application",
            &[
                ("ID", &app.id.to_string()),
                ("Name", &app.name),
                ("Status", &app.status),
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
            ("Status", status_color(&app.status).to_string()),
        ];

        if let Some(ref desc) = app.description {
            details.push(("Description", desc.clone()));
        }
        if let Some(ref source_type) = app.source_type {
            details.push(("Source", source_type.clone()));
        }
        if let (Some(owner), Some(name)) = (&app.github_repo_owner, &app.github_repo_name) {
            details.push(("Repo", format!("{}/{}", owner, name)));
        }
        if let Some(ref image) = app.container_image_url {
            details.push(("Image", image.clone()));
        }
        if let Some(ref url) = app.primary_url {
            details.push(("URL", url.clone()));
        }
        if let Some(ref domain) = app.custom_domain {
            details.push(("Custom domain", domain.clone()));
        }

        details.push((
            "Created",
            app.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        ));
        details.push((
            "Updated",
            app.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        ));

        let details_ref: Vec<(&str, &str)> =
            details.iter().map(|(k, v)| (*k, v.as_str())).collect();

        ui::print_detail(&app.name, &details_ref);
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
                description: args.description,
                github_branch: args.branch,
            },
        )
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&app)?);
    } else {
        ui::print_success(
            "Updated application",
            &[("ID", &app.id.to_string()), ("Name", &app.name)],
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

    ui::print_success("Deleted application", &[("ID", &args.id.to_string())]);

    Ok(())
}
