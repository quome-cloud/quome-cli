use std::io::IsTerminal;

use clap::{Parser, Subcommand};
use uuid::Uuid;

use crate::api::models::{AppEnvironment, CreateEnvironmentRequest, PromoteEnvironmentRequest};
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
    /// Promote a source environment's exact image to a target environment
    Promote(EnvPromoteArgs),
    /// Show or edit an environment's build/runtime override keys
    Config(EnvConfigArgs),
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

#[derive(Parser)]
pub struct EnvPromoteArgs {
    /// Target environment (name or UUID)
    target: String,
    /// Source environment (name or UUID)
    #[arg(long = "from")]
    from: String,
    /// Gate acknowledgement: the TARGET environment's name (for gated envs / CI)
    #[arg(long)]
    gate_ack: Option<String>,
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
pub struct EnvConfigArgs {
    #[command(subcommand)]
    action: Option<EnvConfigAction>,
    /// Environment (name or UUID) — required
    #[arg(long, global = true)]
    environment: Option<String>,
    /// Application ID (uses linked app if not provided)
    #[arg(long, global = true)]
    app: Option<Uuid>,
    /// Organization ID (uses linked org if not provided)
    #[arg(long, global = true)]
    org: Option<Uuid>,
    /// Output as JSON
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
pub enum EnvConfigAction {
    /// Show the environment's override keys (default)
    Show,
    /// Set override keys (KEY=VALUE ...)
    Set { pairs: Vec<String> },
    /// Remove override keys
    Unset { keys: Vec<String> },
}

pub(crate) fn resolve_context(
    org: Option<Uuid>,
    app: Option<Uuid>,
    config: &Config,
) -> Result<(Uuid, Uuid)> {
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

pub async fn execute(command: Option<EnvsCommands>) -> Result<()> {
    // Bare `quome apps envs` defaults to `list` — matching `envs config`'s
    // existing default-to-`show` pattern below.
    let command = command.unwrap_or(EnvsCommands::List(EnvListArgs {
        app: None,
        org: None,
        json: false,
    }));
    match command {
        EnvsCommands::List(args) => list(args).await,
        EnvsCommands::Create(args) => create(args).await,
        EnvsCommands::Delete(args) => delete(args).await,
        EnvsCommands::Promote(args) => promote(args).await,
        EnvsCommands::Config(args) => config_cmd(args).await,
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

async fn promote(args: EnvPromoteArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let (org_id, app_id) = resolve_context(args.org, args.app, &config)?;
    let client = QuomeClient::new(Some(&token), None)?;

    let target = resolve_environment(&client, org_id, app_id, &args.target).await?;
    let source = resolve_environment(&client, org_id, app_id, &args.from).await?;

    let sp = ui::spinner("Promoting...");
    let mut result = client
        .promote_environment(
            org_id,
            app_id,
            target.id,
            &PromoteEnvironmentRequest {
                from_environment_id: source.id,
                gate_ack: args.gate_ack.clone(),
            },
        )
        .await;
    sp.finish_and_clear();

    // Type-the-name gate: one interactive retry on a TTY.
    if let Err(QuomeError::ApiError(detail)) = &result {
        if is_gate_denial(detail) && args.gate_ack.is_none() {
            if std::io::stdin().is_terminal() {
                let typed = inquire::Text::new(&format!(
                    "'{}' is gated. Type the environment name to confirm:",
                    target.name
                ))
                .prompt()
                .map_err(|_| QuomeError::Usage("cancelled".into()))?;
                let sp = ui::spinner("Promoting...");
                result = client
                    .promote_environment(
                        org_id,
                        app_id,
                        target.id,
                        &PromoteEnvironmentRequest {
                            from_environment_id: source.id,
                            gate_ack: Some(typed),
                        },
                    )
                    .await;
                sp.finish_and_clear();
            } else {
                return Err(QuomeError::Usage(format!(
                    "{} — pass --gate-ack {}",
                    detail, target.name
                )));
            }
        }
    }
    let env = result?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&env)?);
    } else {
        ui::print_success(
            "Promotion started",
            &[
                ("Target", &env.name),
                ("From", &source.name),
                (
                    "Note",
                    "target deploys the source's exact image digest (no rebuild)",
                ),
            ],
        );
    }
    Ok(())
}

async fn config_cmd(args: EnvConfigArgs) -> Result<()> {
    let env_ref = args
        .environment
        .as_deref()
        .ok_or_else(|| QuomeError::Usage("--environment is required for envs config".into()))?;
    let config = Config::load()?;
    let token = config.require_token()?;
    let (org_id, app_id) = resolve_context(args.org, args.app, &config)?;
    let client = QuomeClient::new(Some(&token), None)?;
    let env = resolve_environment(&client, org_id, app_id, env_ref).await?;

    match args.action.unwrap_or(EnvConfigAction::Show) {
        EnvConfigAction::Show => {
            let mut overrides = env.config_overrides.clone();
            if let Some(map) = overrides.as_object_mut() {
                map.remove("env_vars");
                map.remove("sidecar_env_vars");
            }
            println!("{}", serde_json::to_string_pretty(&overrides)?);
        }
        EnvConfigAction::Set { pairs } => {
            if pairs.is_empty() {
                return Err(QuomeError::Usage(
                    "set requires at least one KEY=VALUE".into(),
                ));
            }
            let mut patch = serde_json::Map::new();
            for pair in &pairs {
                let (key, value) = pair
                    .split_once('=')
                    .ok_or_else(|| QuomeError::Usage(format!("'{}' is not KEY=VALUE", pair)))?;
                validate_config_key(key).map_err(QuomeError::Usage)?;
                patch.insert(key.to_string(), parse_config_value(value));
            }
            let changed: Vec<String> = patch.keys().cloned().collect();
            client
                .update_environment_overrides(
                    org_id,
                    app_id,
                    env.id,
                    &serde_json::Value::Object(patch),
                )
                .await?;
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "changed": changed, "environment": env.name
                    }))?
                );
            } else {
                println!(
                    "Set {} on '{}' — applies on the next deploy",
                    changed.join(", "),
                    env.name
                );
            }
        }
        EnvConfigAction::Unset { keys } => {
            if keys.is_empty() {
                return Err(QuomeError::Usage("unset requires at least one KEY".into()));
            }
            let mut patch = serde_json::Map::new();
            for key in &keys {
                validate_config_key(key).map_err(QuomeError::Usage)?;
                patch.insert(key.clone(), serde_json::Value::Null);
            }
            client
                .update_environment_overrides(
                    org_id,
                    app_id,
                    env.id,
                    &serde_json::Value::Object(patch),
                )
                .await?;
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "changed": keys, "environment": env.name
                    }))?
                );
            } else {
                println!(
                    "Unset {} on '{}' — applies on the next deploy",
                    keys.join(", "),
                    env.name
                );
            }
        }
    }
    Ok(())
}

/// Parse a `KEY=VALUE` value into a JSON scalar: numbers and booleans parse
/// as their JSON type, everything else (including things that merely look
/// numeric-ish like "1Gi") stays a string.
fn parse_config_value(raw: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .filter(|v| v.is_number() || v.is_boolean())
        .unwrap_or_else(|| serde_json::Value::String(raw.to_string()))
}

/// `env_vars`/`sidecar_env_vars` are managed by `quome apps env-vars`, not
/// `envs config` — reject them here so the two commands don't fight over the
/// same PATCH surface.
fn validate_config_key(key: &str) -> std::result::Result<(), String> {
    if key == "env_vars" || key == "sidecar_env_vars" {
        return Err(format!(
            "'{}' is managed by `quome apps env-vars`, not `envs config`",
            key
        ));
    }
    Ok(())
}

/// Detect the promotion-gate denial so `promote` can retry with an
/// interactive type-the-name confirmation instead of surfacing a raw 403.
fn is_gate_denial(detail: &str) -> bool {
    detail.contains("gated")
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

    #[test]
    fn config_kv_parses_json_scalars() {
        assert_eq!(parse_config_value("2"), serde_json::json!(2));
        assert_eq!(parse_config_value("true"), serde_json::json!(true));
        assert_eq!(parse_config_value("1Gi"), serde_json::json!("1Gi"));
        assert_eq!(parse_config_value("2.5"), serde_json::json!(2.5));
    }

    #[test]
    fn config_rejects_env_var_keys() {
        for key in ["env_vars", "sidecar_env_vars"] {
            assert!(validate_config_key(key).is_err());
        }
        assert!(validate_config_key("memory").is_ok());
    }

    #[test]
    fn gate_detection() {
        assert!(is_gate_denial(
            "This environment is gated. Type the environment name ('production') to confirm."
        ));
        assert!(!is_gate_denial("Permission denied: update on app"));
    }
}
