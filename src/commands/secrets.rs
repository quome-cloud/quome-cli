use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::{CreateSecretRequest, UpdateSecretRequest};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Subcommand)]
pub enum SecretsCommands {
    /// List all secrets
    List(ListArgs),
    /// Set (create or update) a secret
    Set(SetArgs),
    /// Get a secret value
    Get(GetArgs),
    /// Delete a secret
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
pub struct SetArgs {
    /// Secret name
    name: String,

    /// Secret value
    value: String,

    /// Secret description
    #[arg(short, long)]
    description: Option<String>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct GetArgs {
    /// Secret name
    name: String,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct DeleteArgs {
    /// Secret name
    name: String,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Skip confirmation prompt
    #[arg(short, long)]
    force: bool,
}

pub async fn execute(command: SecretsCommands) -> Result<()> {
    match command {
        SecretsCommands::List(args) => list(args).await,
        SecretsCommands::Set(args) => set(args).await,
        SecretsCommands::Get(args) => get(args).await,
        SecretsCommands::Delete(args) => delete(args).await,
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
    let response = client.list_secrets(org_id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.secrets)?);
    } else {
        if response.secrets.is_empty() {
            println!("No secrets found.");
            return Ok(());
        }

        println!(
            "{:<20}  {:<36}  {:<20}",
            "NAME".bold(),
            "ID".bold(),
            "UPDATED".bold()
        );
        println!("{}", "-".repeat(78));

        for secret in response.secrets {
            println!(
                "{:<20}  {:<36}  {:<20}",
                secret.name,
                secret.id,
                secret.updated_at.format("%Y-%m-%d %H:%M")
            );
        }
    }

    Ok(())
}

async fn set(args: SetArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    // Check if secret exists
    let response = client.list_secrets(org_id).await?;
    let existing = response.secrets.iter().find(|s| s.name == args.name);

    let secret = if let Some(existing_secret) = existing {
        // Update existing secret
        client
            .update_secret(
                org_id,
                existing_secret.id,
                &UpdateSecretRequest {
                    name: None,
                    value: Some(args.value),
                    description: args.description,
                },
            )
            .await?
    } else {
        // Create new secret
        client
            .create_secret(
                org_id,
                &CreateSecretRequest {
                    name: args.name,
                    value: args.value,
                    description: args.description,
                },
            )
            .await?
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&secret)?);
    } else {
        let action = if existing.is_some() {
            "Updated"
        } else {
            "Created"
        };
        println!("{} {} secret:", "Success!".green().bold(), action);
        println!("  {} {}", "Name:".dimmed(), secret.name);
        println!("  {} {}", "ID:".dimmed(), secret.id);
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

    // Find secret by name
    let response = client.list_secrets(org_id).await?;
    let secret_meta = response
        .secrets
        .iter()
        .find(|s| s.name == args.name)
        .ok_or_else(|| crate::errors::QuomeError::NotFound(format!("Secret '{}'", args.name)))?;

    let secret = client.get_secret(org_id, secret_meta.id).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&secret)?);
    } else {
        println!("{}", secret.value);
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
            "Are you sure you want to delete secret '{}'?",
            args.name
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

    // Find secret by name
    let response = client.list_secrets(org_id).await?;
    let secret = response
        .secrets
        .iter()
        .find(|s| s.name == args.name)
        .ok_or_else(|| crate::errors::QuomeError::NotFound(format!("Secret '{}'", args.name)))?;

    client.delete_secret(org_id, secret.id).await?;

    println!(
        "{} Deleted secret '{}'",
        "Success!".green().bold(),
        args.name
    );

    Ok(())
}
