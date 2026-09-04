use clap::{Parser, Subcommand};
use uuid::Uuid;

use crate::api::models::{AppEnvironment, CreateEnvironmentRequest};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::{QuomeError, Result};
use crate::ui::{self, EnvRow};

#[derive(Subcommand)]
pub enum EnvsCommands {
    /// List the app's environments (pipeline order)
    List(EnvListArgs),
    /// Create an environment
    Create(EnvCreateArgs),
    /// Delete an environment (tears down its deploy target)
    Delete(EnvDeleteArgs),
}

#[derive(Parser)]
pub struct EnvListArgs {
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
pub struct EnvCreateArgs {
    /// Environment name (lowercase slug)
    name: String,
    /// Deploy branch for this environment
    #[arg(long)]
    branch: Option<String>,
    /// Disable auto-deploy on push
    #[arg(long)]
    no_auto_deploy: bool,
    /// Copy plain env vars from another environment (name or UUID)
    #[arg(long)]
    copy_vars_from: Option<String>,
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
pub struct EnvDeleteArgs {
    /// Environment to delete (name or UUID)
    env: String,
    /// Application ID (uses linked app if not provided)
    #[arg(long)]
    app: Option<Uuid>,
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,
    /// Skip confirmation prompt
    #[arg(short, long)]
    force: bool,
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

fn resolve_context(org: Option<Uuid>, app: Option<Uuid>, config: &Config) -> Result<(Uuid, Uuid)> {
    let org_id = match org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };
    let app_id = match app {
        Some(id) => id,
        None => config.require_linked_app()?,
    };
    Ok((org_id, app_id))
}

pub async fn execute(command: EnvsCommands) -> Result<()> {
    match command {
        EnvsCommands::List(args) => list(args).await,
        EnvsCommands::Create(args) => create(args).await,
        EnvsCommands::Delete(args) => delete(args).await,
    }
}

async fn list(args: EnvListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let (org_id, app_id) = resolve_context(args.org, args.app, &config)?;
    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching environments...");
    let mut envs = client.list_environments(org_id, app_id).await?;
    sp.finish_and_clear();
    envs.sort_by_key(|e| e.sort_order);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&envs)?);
        return Ok(());
    }
    if envs.is_empty() {
        println!("No environments — this app uses the single-environment default.");
        return Ok(());
    }
    let rows: Vec<EnvRow> = envs
        .iter()
        .map(|e| EnvRow {
            name: e.name.clone(),
            slug: e.slug.clone(),
            default: if e.is_default {
                "yes".into()
            } else {
                "".into()
            },
            branch: e.deploy_branch.clone().unwrap_or_default(),
            auto: if e.auto_deploy {
                "yes".into()
            } else {
                "no".into()
            },
            status: e.status.clone(),
            id: e.id.to_string(),
        })
        .collect();
    ui::print_table(rows);
    Ok(())
}

async fn create(args: EnvCreateArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let (org_id, app_id) = resolve_context(args.org, args.app, &config)?;
    let client = QuomeClient::new(Some(&token), None)?;

    let copy_from = match &args.copy_vars_from {
        Some(r) => Some(
            resolve_environment(&client, org_id, app_id, r)
                .await?
                .id
                .to_string(),
        ),
        None => None,
    };

    let sp = ui::spinner("Creating environment...");
    let env = client
        .create_environment(
            org_id,
            app_id,
            &CreateEnvironmentRequest {
                name: args.name,
                deploy_branch: args.branch,
                auto_deploy: !args.no_auto_deploy,
                copy_vars_from_environment_id: copy_from,
            },
        )
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&env)?);
    } else {
        ui::print_success(
            "Created environment",
            &[
                ("Name", &env.name),
                ("Branch", env.deploy_branch.as_deref().unwrap_or("-")),
                ("ID", &env.id.to_string()),
            ],
        );
    }
    Ok(())
}

async fn delete(args: EnvDeleteArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let (org_id, app_id) = resolve_context(args.org, args.app, &config)?;
    let client = QuomeClient::new(Some(&token), None)?;

    let env = resolve_environment(&client, org_id, app_id, &args.env).await?;
    if env.is_default {
        return Err(QuomeError::Usage(
            "the default environment cannot be deleted".into(),
        ));
    }
    if !args.force {
        let confirm = inquire::Confirm::new(&format!(
            "Delete environment '{}'? This tears down its deployment target and any \
             dedicated resources provisioned for it.",
            env.name
        ))
        .with_default(false)
        .prompt()
        .map_err(|_| QuomeError::Usage("cancelled".into()))?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let sp = ui::spinner("Deleting environment...");
    client.delete_environment(org_id, app_id, env.id).await?;
    sp.finish_and_clear();

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"deleted": env.id}))?
        );
    } else {
        ui::print_success("Deleted environment", &[("Name", &env.name)]);
    }
    Ok(())
}

/// Pure matcher the resolver delegates to (unit-testable without a client):
/// exact name match first, then UUID. Env names are slug-validated and
/// unique per app, so no ambiguity.
fn match_environment<'a>(envs: &'a [AppEnvironment], env_ref: &str) -> Option<&'a AppEnvironment> {
    if let Some(e) = envs.iter().find(|e| e.name == env_ref) {
        return Some(e);
    }
    Uuid::parse_str(env_ref)
        .ok()
        .and_then(|id| envs.iter().find(|e| e.id == id))
}

/// Resolve an environment reference: exact name match first, then UUID.
pub async fn resolve_environment(
    client: &QuomeClient,
    org_id: Uuid,
    app_id: Uuid,
    env_ref: &str,
) -> Result<AppEnvironment> {
    let envs = client.list_environments(org_id, app_id).await?;
    match_environment(&envs, env_ref).cloned().ok_or_else(|| {
        QuomeError::NotFound(format!(
            "no environment '{}' — see `quome apps envs`",
            env_ref
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str, id: Uuid) -> AppEnvironment {
        AppEnvironment {
            id,
            name: name.to_string(),
            slug: name.to_string(),
            is_default: false,
            deploy_branch: None,
            auto_deploy: true,
            status: "active".into(),
            sort_order: 0,
            promotion_gate: "none".into(),
            config_overrides: serde_json::Value::Null,
        }
    }

    // Fixed literals — the `uuid` crate is built without the `v4` feature
    // here, matching the rest of the codebase's tests (fixed UUID strings).
    const ID_A: &str = "9b2f0a34-1111-2222-3333-444455556666";
    const ID_B: &str = "3c1d2e4f-1111-2222-3333-444455556666";
    const ID_C: &str = "aaaa0a34-1111-2222-3333-444455556666";

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    #[test]
    fn match_environment_by_name() {
        let id = uuid(ID_A);
        let envs = vec![env("staging", id), env("prod", uuid(ID_B))];
        let found = match_environment(&envs, "staging").unwrap();
        assert_eq!(found.id, id);
    }

    #[test]
    fn match_environment_by_uuid() {
        let id = uuid(ID_A);
        let envs = vec![env("staging", id)];
        let found = match_environment(&envs, &id.to_string()).unwrap();
        assert_eq!(found.name, "staging");
    }

    #[test]
    fn match_environment_name_shadows_uuid_looking_name() {
        // A weird-but-valid environment name that happens to look like a
        // UUID string must still be matched by NAME first — it should never
        // fall through to the UUID branch and get compared as an id.
        let uuid_like_id = uuid(ID_A);
        let other_id = uuid(ID_B);
        let uuid_like_name = uuid_like_id.to_string();
        let envs = vec![env(&uuid_like_name, other_id), env("staging", uuid_like_id)];
        let found = match_environment(&envs, &uuid_like_name).unwrap();
        assert_eq!(found.id, other_id, "name match must win over uuid match");
    }

    #[test]
    fn match_environment_miss_returns_none() {
        let envs = vec![env("staging", uuid(ID_A))];
        assert!(match_environment(&envs, "nonexistent").is_none());
        assert!(match_environment(&envs, ID_C).is_none());
    }
}
