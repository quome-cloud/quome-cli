use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::DeploymentStatus;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Subcommand)]
pub enum DeploymentsCommands {
    /// List deployments
    List(ListArgs),
    /// Get deployment details
    Get(GetArgs),
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

pub async fn execute(command: DeploymentsCommands) -> Result<()> {
    match command {
        DeploymentsCommands::List(args) => list(args).await,
        DeploymentsCommands::Get(args) => get(args).await,
    }
}

fn status_color(status: &DeploymentStatus) -> colored::ColoredString {
    match status {
        DeploymentStatus::Created => "created".yellow(),
        DeploymentStatus::InProgress => "in_progress".blue(),
        DeploymentStatus::Deployed => "deployed".green(),
        DeploymentStatus::Failed => "failed".red(),
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
    let response = client.list_deployments(org_id, app_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.deployments)?);
    } else {
        if response.deployments.is_empty() {
            println!("No deployments found.");
            return Ok(());
        }

        println!(
            "{:<36}  {:<12}  {:<20}",
            "ID".bold(),
            "STATUS".bold(),
            "CREATED".bold()
        );
        println!("{}", "-".repeat(70));

        for deployment in response.deployments {
            println!(
                "{:<36}  {:<12}  {:<20}",
                deployment.id,
                status_color(&deployment.status),
                deployment.created_at.format("%Y-%m-%d %H:%M")
            );
        }
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
    let deployment = client.get_deployment(org_id, app_id, args.id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&deployment)?);
    } else {
        println!("{}", "Deployment".bold());
        println!("  {} {}", "ID:".dimmed(), deployment.id);
        println!("  {} {}", "Status:".dimmed(), status_color(&deployment.status));
        println!(
            "  {} {}",
            "Created:".dimmed(),
            deployment.created_at.format("%Y-%m-%d %H:%M:%S")
        );

        if let Some(ref msg) = deployment.failure_message {
            println!("  {} {}", "Failure:".red(), msg);
        }

        if !deployment.events.is_empty() {
            println!();
            println!("  {}", "Events:".bold());
            for event in &deployment.events {
                println!(
                    "    {} {} - {}",
                    event.created_at.format("%H:%M:%S").to_string().dimmed(),
                    "-".dimmed(),
                    event.message
                );
            }
        }
    }

    Ok(())
}
