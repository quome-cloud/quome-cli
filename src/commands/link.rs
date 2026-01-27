use clap::Parser;
use colored::Colorize;
use inquire::Select;

use crate::client::QuomeClient;
use crate::config::{Config, LinkedContext};
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {
    /// Organization ID (skips interactive selection)
    #[arg(long)]
    org: Option<String>,

    /// Application ID (skips interactive selection)
    #[arg(long)]
    app: Option<String>,
}

pub async fn execute(args: Args) -> Result<()> {
    let mut config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;

    // Get or select organization
    let (org_id, org_name) = if let Some(ref org_str) = args.org {
        let org_id = org_str.parse().map_err(|_| {
            crate::errors::QuomeError::ApiError("Invalid organization ID".into())
        })?;
        let org = client.get_org(org_id).await?;
        (org.id, org.name)
    } else {
        let orgs_resp = client.list_orgs().await?;

        if orgs_resp.organizations.is_empty() {
            println!("No organizations found. Create one with `quome orgs create <name>`");
            return Ok(());
        }

        let options: Vec<String> = orgs_resp
            .organizations
            .iter()
            .map(|o| format!("{} ({})", o.name, o.id))
            .collect();

        let selection = Select::new("Select organization:", options)
            .prompt()
            .map_err(|e| {
                crate::errors::QuomeError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;

        let idx = orgs_resp
            .organizations
            .iter()
            .position(|o| format!("{} ({})", o.name, o.id) == selection)
            .unwrap();

        let org = &orgs_resp.organizations[idx];
        (org.id, org.name.clone())
    };

    // Get or select application (optional)
    let (app_id, app_name) = if let Some(ref app_str) = args.app {
        let app_id = app_str.parse().map_err(|_| {
            crate::errors::QuomeError::ApiError("Invalid application ID".into())
        })?;
        let app = client.get_app(org_id, app_id).await?;
        (Some(app.id), Some(app.name))
    } else {
        let apps_resp = client.list_apps(org_id).await?;

        if apps_resp.apps.is_empty() {
            println!("No applications found in this organization.");
            (None, None)
        } else {
            let mut options: Vec<String> = apps_resp
                .apps
                .iter()
                .map(|a| format!("{} ({})", a.name, a.id))
                .collect();
            options.push("(Skip - don't link an app)".to_string());

            let selection = Select::new("Select application:", options)
                .prompt()
                .map_err(|e| {
                    crate::errors::QuomeError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))
                })?;

            if selection == "(Skip - don't link an app)" {
                (None, None)
            } else {
                let idx = apps_resp
                    .apps
                    .iter()
                    .position(|a| format!("{} ({})", a.name, a.id) == selection)
                    .unwrap();

                let app = &apps_resp.apps[idx];
                (Some(app.id), Some(app.name.clone()))
            }
        }
    };

    // Save linked context
    config.set_linked(LinkedContext {
        org_id,
        org_name: org_name.clone(),
        app_id,
        app_name: app_name.clone(),
    })?;
    config.save()?;

    println!("{} Linked to:", "Success!".green().bold());
    println!("  {} {}", "Organization:".dimmed(), org_name.cyan());
    if let Some(name) = app_name {
        println!("  {} {}", "Application:".dimmed(), name.cyan());
    }

    Ok(())
}
