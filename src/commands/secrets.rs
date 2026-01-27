use clap::{Parser, Subcommand};
use uuid::Uuid;

use crate::api::models::{CreateSecretRequest, UpdateSecretRequest};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui::{self, SecretRow};

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

    let sp = ui::spinner("Fetching secrets...");
    let response = client.list_secrets(org_id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.secrets)?);
    } else {
        if response.secrets.is_empty() {
            println!("No secrets found.");
            return Ok(());
        }

        let rows: Vec<SecretRow> = response
            .secrets
            .iter()
            .map(|secret| SecretRow {
                name: secret.name.clone(),
                id: secret.id.to_string(),
                updated: secret.updated_at.format("%Y-%m-%d %H:%M").to_string(),
            })
            .collect();

        ui::print_table(rows);
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
    let sp = ui::spinner("Checking for existing secret...");
    let response = client.list_secrets(org_id).await?;
    let existing = response.secrets.iter().find(|s| s.name == args.name);
    sp.finish_and_clear();

    let (secret, action) = if let Some(existing_secret) = existing {
        // Update existing secret
        let sp = ui::spinner("Updating secret...");
        let secret = client
            .update_secret(
                org_id,
                existing_secret.id,
                &UpdateSecretRequest {
                    name: None,
                    value: Some(args.value),
                    description: args.description,
                },
            )
            .await?;
        sp.finish_and_clear();
        (secret, "Updated")
    } else {
        // Create new secret
        let sp = ui::spinner("Creating secret...");
        let secret = client
            .create_secret(
                org_id,
                &CreateSecretRequest {
                    name: args.name,
                    value: args.value,
                    description: args.description,
                },
            )
            .await?;
        sp.finish_and_clear();
        (secret, "Created")
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&secret)?);
    } else {
        ui::print_success(&format!("{} secret", action), &[
            ("Name", &secret.name),
            ("ID", &secret.id.to_string()),
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

    let client = QuomeClient::new(Some(&token), None)?;

    // Find secret by name
    let sp = ui::spinner("Fetching secret...");
    let response = client.list_secrets(org_id).await?;
    let secret_meta = response
        .secrets
        .iter()
        .find(|s| s.name == args.name)
        .ok_or_else(|| crate::errors::QuomeError::NotFound(format!("Secret '{}'", args.name)))?;

    let secret = client.get_secret(org_id, secret_meta.id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&secret)?);
    } else {
        match secret.value {
            Some(value) => println!("{}", value),
            None => println!("(no value returned)"),
        }
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
    let sp = ui::spinner("Fetching secret...");
    let response = client.list_secrets(org_id).await?;
    let secret = response
        .secrets
        .iter()
        .find(|s| s.name == args.name)
        .ok_or_else(|| crate::errors::QuomeError::NotFound(format!("Secret '{}'", args.name)))?;
    sp.finish_and_clear();

    let sp = ui::spinner("Deleting secret...");
    client.delete_secret(org_id, secret.id).await?;
    sp.finish_and_clear();

    ui::print_success("Deleted secret", &[
        ("Name", &args.name),
    ]);

    Ok(())
}
