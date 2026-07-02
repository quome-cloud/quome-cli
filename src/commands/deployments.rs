use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::{CreateDeploymentRequest, DeploymentStatus};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui::{self, DeploymentRow};

#[derive(Subcommand)]
pub enum DeploymentsCommands {
    /// List deployments
    List(ListArgs),
    /// Get deployment details
    Get(GetArgs),
    /// Trigger a new deployment
    Create(CreateArgs),
}

#[derive(Parser)]
pub struct ListArgs {
    /// Application ID (uses linked app if not provided)
    #[arg(long)]
    app: Option<Uuid>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct GetArgs {
    /// Deployment ID
    id: Uuid,

    /// Application ID (uses linked app if not provided)
    #[arg(long)]
    app: Option<Uuid>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct CreateArgs {
    /// Git branch to deploy (git-sourced apps)
    #[arg(long)]
    branch: Option<String>,

    /// Application ID (uses linked app if not provided)
    #[arg(long)]
    app: Option<Uuid>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(command: DeploymentsCommands) -> Result<()> {
    match command {
        DeploymentsCommands::List(args) => list(args).await,
        DeploymentsCommands::Get(args) => get(args).await,
        DeploymentsCommands::Create(args) => create(args).await,
    }
}

fn status_color(status: &DeploymentStatus) -> colored::ColoredString {
    match status {
        DeploymentStatus::Created => "created".yellow(),
        DeploymentStatus::InProgress => "in_progress".blue(),
        DeploymentStatus::Success => "success".green(),
        DeploymentStatus::Failed => "failed".red(),
        DeploymentStatus::Cancelled => "cancelled".dimmed(),
    }
}

async fn list(args: ListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let app_id = match args.app {
        Some(id) => id,
        None => config.require_linked_app()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching deployments...");
    let response = client.list_deployments(org_id, app_id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.data)?);
    } else {
        if response.data.is_empty() {
            println!("No deployments found.");
            return Ok(());
        }

        let rows: Vec<DeploymentRow> = response
            .data
            .iter()
            .map(|d| DeploymentRow {
                id: d.id.to_string(),
                status: status_color(&d.status).to_string(),
                branch: d.branch.clone().unwrap_or_else(|| "-".to_string()),
                created: d.created_at.format("%Y-%m-%d %H:%M").to_string(),
            })
            .collect();

        ui::print_table(rows);
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

    let app_id = match args.app {
        Some(id) => id,
        None => config.require_linked_app()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching deployment...");
    let deployment = client.get_deployment(org_id, app_id, args.id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&deployment)?);
    } else {
        let status_str = status_color(&deployment.status).to_string();
        let mut details = vec![
            ("ID", deployment.id.to_string()),
            ("Status", status_str),
            (
                "Created",
                deployment
                    .created_at
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string(),
            ),
        ];

        if let Some(ref branch) = deployment.branch {
            details.push(("Branch", branch.clone()));
        }
        if let Some(ref sha) = deployment.git_commit_sha {
            details.push(("Commit", sha.clone()));
        }
        if let Some(ref reason) = deployment.failure_reason {
            details.push(("Failure", reason.clone()));
        }

        let details_ref: Vec<(&str, &str)> =
            details.iter().map(|(k, v)| (*k, v.as_str())).collect();

        ui::print_detail("Deployment", &details_ref);

        if !deployment.events.is_empty() {
            println!();
            println!("{}", "Events".bold());
            for event in &deployment.events {
                println!(
                    "  {} {} {}",
                    event.created_at.format("%H:%M:%S").to_string().dimmed(),
                    "•".cyan(),
                    event.message
                );
            }
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

    let app_id = match args.app {
        Some(id) => id,
        None => config.require_linked_app()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Triggering deployment...");
    let deployment = client
        .create_deployment(
            org_id,
            app_id,
            &CreateDeploymentRequest {
                branch: args.branch,
                ..Default::default()
            },
        )
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&deployment)?);
    } else {
        ui::print_success(
            "Deployment triggered",
            &[
                ("ID", &deployment.id.to_string()),
                ("Status", &deployment.status.to_string()),
            ],
        );
    }

    Ok(())
}
