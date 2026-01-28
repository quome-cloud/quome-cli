use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::DeploymentStatus;
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
        DeploymentStatus::Success => "success".green(),
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

    let sp = ui::spinner("Fetching deployments...");
    let response = client.list_deployments(org_id, app_id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.deployments)?);
    } else {
        if response.deployments.is_empty() {
            println!("No deployments found.");
            return Ok(());
        }

        let rows: Vec<DeploymentRow> = response
            .deployments
            .iter()
            .map(|d| DeploymentRow {
                id: d.id.to_string(),
                status: status_color(&d.status).to_string(),
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
            ("Created", deployment.created_at.format("%Y-%m-%d %H:%M:%S").to_string()),
        ];

        if let Some(ref msg) = deployment.failure_message {
            details.push(("Failure", msg.clone()));
        }

        let details_ref: Vec<(&str, &str)> = details
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();

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
