use clap::{ArgGroup, Parser};
use uuid::Uuid;

use crate::api::models::{
    AppBinding, BindingResourceType, Cache, CreateBindingRequest, Database, PaginatedResponse,
    Secret, StorageBucket,
};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::{QuomeError, Result};
use crate::ui::{self, BindingRow};

/// Client-side mirror of the server's env-var rule (`^[A-Z][A-Z0-9_]*$`,
/// max 255). The server stays authoritative; this exists for a fast,
/// friendly error with a suggestion.
fn validate_env_var_name(name: &str) -> std::result::Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 255
        && name.chars().next().unwrap().is_ascii_uppercase()
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if ok {
        return Ok(());
    }
    let suggestion: String = name
        .chars()
        .map(|c| match c {
            'a'..='z' => c.to_ascii_uppercase(),
            'A'..='Z' | '0'..='9' | '_' => c,
            _ => '_',
        })
        .collect();
    Err(format!(
        "env var names must match ^[A-Z][A-Z0-9_]*$ — try --env-var {}",
        suggestion.trim_start_matches(|c: char| c.is_ascii_digit() || c == '_')
    ))
}

/// A resource flag value: a UUID passes through, anything else is a name
/// to resolve against that type's list endpoint.
enum ResourceRef {
    Id(Uuid),
    Name(String),
}

fn parse_resource_ref(value: &str) -> ResourceRef {
    match Uuid::parse_str(value) {
        Ok(id) => ResourceRef::Id(id),
        Err(_) => ResourceRef::Name(value.to_string()),
    }
}

/// SCOPE column: `app`, `preview`, or `env:<name-or-id>`.
fn scope_label(
    binding: &AppBinding,
    env_names: &std::collections::HashMap<String, String>,
) -> String {
    if let Some(env_id) = &binding.environment_id {
        let name = env_names
            .get(env_id)
            .cloned()
            .unwrap_or_else(|| env_id.clone());
        return format!("env:{}", name);
    }
    if binding.allow_in_preview {
        return "preview".to_string();
    }
    "app".to_string()
}

#[derive(Parser)]
pub struct BindingsArgs {
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
#[command(group(
    ArgGroup::new("resource")
        .required(true)
        .args(["secret", "database", "bucket", "cache"]),
))]
pub struct BindArgs {
    /// Environment variable name (must match ^[A-Z][A-Z0-9_]*$)
    #[arg(long)]
    env_var: String,
    /// Secret to bind (name or UUID)
    #[arg(long)]
    secret: Option<String>,
    /// Database to bind (name or UUID)
    #[arg(long)]
    database: Option<String>,
    /// Storage bucket to bind (name or UUID)
    #[arg(long)]
    bucket: Option<String>,
    /// Cache to bind (name or UUID)
    #[arg(long)]
    cache: Option<String>,
    /// Application ID (uses linked app if not provided)
    #[arg(long)]
    app: Option<Uuid>,
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,
    /// Bind only for one app environment (name or UUID)
    #[arg(long)]
    environment: Option<String>,
    /// Also inject into PR preview deploys (app-level bindings only)
    #[arg(long)]
    preview: bool,
    /// Target container for multi-container apps
    #[arg(long)]
    container: Option<String>,
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct UnbindArgs {
    /// Binding ID to remove (or use --env-var)
    binding_id: Option<Uuid>,
    /// Resolve the binding by env var name instead of ID
    #[arg(long, conflicts_with = "binding_id")]
    env_var: Option<String>,
    /// Disambiguate --env-var to one environment's binding (name or UUID)
    #[arg(long)]
    environment: Option<String>,
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

/// Resolve a resource flag to (type, id, display name): UUIDs pass through,
/// names go through the type's list endpoint. Unknown names error with the
/// list command to run.
async fn resolve_resource(
    client: &QuomeClient,
    org_id: Uuid,
    resource_type: BindingResourceType,
    value: &str,
) -> Result<(BindingResourceType, Uuid, String)> {
    match parse_resource_ref(value) {
        ResourceRef::Id(id) => Ok((resource_type, id, value.to_string())),
        ResourceRef::Name(name) => {
            let (found, hint): (Option<Uuid>, &str) = match resource_type {
                BindingResourceType::Secret => (
                    client
                        .list_all_pages::<Secret>(&format!("/api/v1/orgs/{}/secrets", org_id))
                        .await?
                        .iter()
                        .find(|s| s.name == name)
                        .map(|s| s.id),
                    "see `quome secrets list`",
                ),
                BindingResourceType::Database => (
                    client
                        .list_all_pages::<Database>(&format!("/api/v1/orgs/{}/dbaas", org_id))
                        .await?
                        .iter()
                        .find(|d| d.name == name)
                        .map(|d| d.id),
                    "see `quome db list`",
                ),
                BindingResourceType::Bucket => (
                    client
                        .list_all_pages::<StorageBucket>(&format!(
                            "/api/v1/orgs/{}/storage",
                            org_id
                        ))
                        .await?
                        .iter()
                        .find(|b| b.name == name)
                        .map(|b| b.id),
                    "see the Storage page in the dashboard",
                ),
                BindingResourceType::Cache => (
                    client
                        .list_all_pages::<Cache>(&format!("/api/v1/orgs/{}/caches", org_id))
                        .await?
                        .iter()
                        .find(|c| c.name == name)
                        .map(|c| c.id),
                    "see the Caches page in the dashboard",
                ),
                BindingResourceType::EventSubscription => (None, ""),
            };
            match found {
                Some(id) => Ok((resource_type, id, name)),
                None => Err(QuomeError::NotFound(format!(
                    "no {} named '{}' — {}",
                    resource_type.as_str(),
                    name,
                    hint
                ))),
            }
        }
    }
}

/// One list call per resource type present, joined in-memory; a vanished
/// resource falls back to its raw UUID.
async fn resolve_resource_names(
    client: &QuomeClient,
    org_id: Uuid,
    bindings: &[AppBinding],
) -> std::collections::HashMap<(BindingResourceType, Uuid), String> {
    let mut names = std::collections::HashMap::new();
    let types: std::collections::HashSet<BindingResourceType> =
        bindings.iter().map(|b| b.resource_type).collect();
    for t in types {
        match t {
            BindingResourceType::Secret => {
                if let Ok(all) = client
                    .list_all_pages::<Secret>(&format!("/api/v1/orgs/{}/secrets", org_id))
                    .await
                {
                    for s in all {
                        names.insert((t, s.id), s.name);
                    }
                }
            }
            BindingResourceType::Database => {
                if let Ok(all) = client
                    .list_all_pages::<Database>(&format!("/api/v1/orgs/{}/dbaas", org_id))
                    .await
                {
                    for d in all {
                        names.insert((t, d.id), d.name);
                    }
                }
            }
            BindingResourceType::Bucket => {
                if let Ok(all) = client
                    .list_all_pages::<StorageBucket>(&format!("/api/v1/orgs/{}/storage", org_id))
                    .await
                {
                    for b in all {
                        names.insert((t, b.id), b.name);
                    }
                }
            }
            BindingResourceType::Cache => {
                if let Ok(all) = client
                    .list_all_pages::<Cache>(&format!("/api/v1/orgs/{}/caches", org_id))
                    .await
                {
                    for c in all {
                        names.insert((t, c.id), c.name);
                    }
                }
            }
            BindingResourceType::EventSubscription => {}
        }
    }
    names
}

fn resource_name_for(
    b: &AppBinding,
    names: &std::collections::HashMap<(BindingResourceType, Uuid), String>,
) -> String {
    names
        .get(&(b.resource_type, b.resource_id))
        .cloned()
        .unwrap_or_else(|| b.resource_id.to_string())
}

/// environment_id → environment name, when any binding is env-scoped. Uses
/// `GET /api/v1/orgs/{org}/apps/{app}/environments`; falls back to raw ids
/// on any error (environments may be feature-gated off, or the endpoint may
/// not exist on an older control plane).
async fn resolve_env_names(
    client: &QuomeClient,
    org_id: Uuid,
    app_id: Uuid,
    bindings: &[AppBinding],
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if bindings.iter().all(|b| b.environment_id.is_none()) {
        return map;
    }
    if let Ok(page) = client
        .get::<PaginatedResponse<serde_json::Value>>(&format!(
            "/api/v1/orgs/{}/apps/{}/environments",
            org_id, app_id
        ))
        .await
    {
        for e in page.data {
            if let (Some(id), Some(name)) = (e["id"].as_str(), e["name"].as_str()) {
                map.insert(id.to_string(), name.to_string());
            }
        }
    }
    map
}

/// Bindings are org-admin gated server-side on top of app permissions.
fn admin_hint(err: QuomeError) -> QuomeError {
    match err {
        QuomeError::ApiError(msg)
            if msg.to_lowercase().contains("admin") || msg.contains("403") =>
        {
            QuomeError::ApiError(format!("{} (managing bindings requires org admin)", msg))
        }
        other => other,
    }
}

pub async fn list(args: BindingsArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let (org_id, app_id) = resolve_context(args.org, args.app, &config)?;
    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching bindings...");
    let bindings = client
        .list_bindings(org_id, app_id)
        .await
        .map_err(admin_hint)?;
    let resource_names = resolve_resource_names(&client, org_id, &bindings).await;
    let env_names = resolve_env_names(&client, org_id, app_id, &bindings).await;
    sp.finish_and_clear();

    if args.json {
        let rows: Vec<serde_json::Value> = bindings
            .iter()
            .map(|b| {
                let mut v = serde_json::to_value(b).expect("binding serializes");
                v["resource_name"] =
                    serde_json::Value::String(resource_name_for(b, &resource_names));
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if bindings.is_empty() {
        println!("No bindings. Add one with `quome apps bind --env-var NAME --secret <name>`.");
        return Ok(());
    }
    let rows: Vec<BindingRow> = bindings
        .iter()
        .map(|b| BindingRow {
            env_var: b.env_var_name.clone(),
            resource_type: b.resource_type.as_str().to_string(),
            resource: resource_name_for(b, &resource_names),
            scope: scope_label(b, &env_names),
            id: b.id.to_string(),
        })
        .collect();
    ui::print_table(rows);
    Ok(())
}

pub async fn bind(args: BindArgs) -> Result<()> {
    if let Err(msg) = validate_env_var_name(&args.env_var) {
        return Err(QuomeError::Usage(msg));
    }
    if args.preview && args.environment.is_some() {
        return Err(QuomeError::Usage(
            "--preview is only valid for app-level bindings (drop --environment): \
             env-specific overrides are never injected into previews"
                .into(),
        ));
    }
    let config = Config::load()?;
    let token = config.require_token()?;
    let (org_id, app_id) = resolve_context(args.org, args.app, &config)?;
    let client = QuomeClient::new(Some(&token), None)?;

    let (resource_type, raw) = if let Some(v) = &args.secret {
        (BindingResourceType::Secret, v)
    } else if let Some(v) = &args.database {
        (BindingResourceType::Database, v)
    } else if let Some(v) = &args.bucket {
        (BindingResourceType::Bucket, v)
    } else if let Some(v) = &args.cache {
        (BindingResourceType::Cache, v)
    } else {
        unreachable!("clap ArgGroup guarantees one resource flag");
    };

    let sp = ui::spinner("Resolving resource...");
    let (resource_type, resource_id, resource_name) =
        resolve_resource(&client, org_id, resource_type, raw).await?;
    sp.finish_and_clear();

    let environment_id = match &args.environment {
        Some(env_ref) => Some(
            crate::commands::envs::resolve_environment(&client, org_id, app_id, env_ref)
                .await?
                .id,
        ),
        None => None,
    };

    let sp = ui::spinner("Creating binding...");
    let binding = client
        .create_binding(
            org_id,
            app_id,
            &CreateBindingRequest {
                resource_type,
                resource_id,
                env_var_name: args.env_var.clone(),
                container_name: args.container.clone(),
                environment_id: environment_id.map(|e| e.to_string()),
                allow_in_preview: args.preview,
            },
        )
        .await
        .map_err(admin_hint)?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&binding)?);
    } else {
        ui::print_success(
            "Created binding",
            &[
                ("Env var", &binding.env_var_name),
                (
                    "Resource",
                    &format!("{} {}", resource_type.as_str(), resource_name),
                ),
                ("Binding ID", &binding.id.to_string()),
            ],
        );
    }
    Ok(())
}

pub async fn unbind(args: UnbindArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let (org_id, app_id) = resolve_context(args.org, args.app, &config)?;
    let client = QuomeClient::new(Some(&token), None)?;

    let target: AppBinding = match (args.binding_id, &args.env_var) {
        (Some(id), _) => {
            let all = client.list_bindings(org_id, app_id).await?;
            all.into_iter()
                .find(|b| b.id == id)
                .ok_or_else(|| QuomeError::NotFound(format!("no binding {}", id)))?
        }
        (None, Some(name)) => {
            let all = client.list_bindings(org_id, app_id).await?;
            let env = match &args.environment {
                Some(env_ref) => Some(
                    crate::commands::envs::resolve_environment(&client, org_id, app_id, env_ref)
                        .await?
                        .id
                        .to_string(),
                ),
                None => None,
            };
            let matches: Vec<AppBinding> = all
                .into_iter()
                .filter(|b| &b.env_var_name == name && b.environment_id == env)
                .collect();
            match matches.len() {
                0 => {
                    return Err(QuomeError::NotFound(format!(
                        "no binding with env var '{}' at that scope — see `quome apps bindings`",
                        name
                    )))
                }
                1 => matches.into_iter().next().unwrap(),
                _ => {
                    // Never guess between scopes: list the candidates and stop.
                    // Environment names may fail to resolve (feature-gated off,
                    // older control plane) — scope_label falls back to the raw id.
                    let env_names = resolve_env_names(&client, org_id, app_id, &matches).await;
                    eprintln!("'{}' matches multiple bindings:", name);
                    for b in &matches {
                        eprintln!("  {}  ({})", b.id, scope_label(b, &env_names));
                    }
                    return Err(QuomeError::Usage(
                        "pass the binding ID to unbind exactly one".into(),
                    ));
                }
            }
        }
        (None, None) => {
            return Err(QuomeError::Usage(
                "pass a binding ID or --env-var NAME".into(),
            ))
        }
    };

    if !args.force {
        let confirm = inquire::Confirm::new(&format!(
            "Remove binding {} ({} → {})?",
            target.id,
            target.env_var_name,
            target.resource_type.as_str()
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| QuomeError::Io(std::io::Error::other(e.to_string())))?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let sp = ui::spinner("Removing binding...");
    client
        .delete_binding(org_id, app_id, target.id)
        .await
        .map_err(admin_hint)?;
    sp.finish_and_clear();

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"removed": target.id}))?
        );
    } else {
        ui::print_success(
            "Removed binding",
            &[
                ("Env var", &target.env_var_name),
                ("Binding ID", &target.id.to_string()),
            ],
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_validation() {
        assert!(validate_env_var_name("DATABASE_PASSWORD").is_ok());
        assert!(validate_env_var_name("A1_B2").is_ok());
        let err = validate_env_var_name("database-password").unwrap_err();
        assert!(
            err.contains("DATABASE_PASSWORD"),
            "suggestion missing: {err}"
        );
        assert!(validate_env_var_name("1BAD").is_err());
        assert!(validate_env_var_name("").is_err());
    }

    #[test]
    fn resource_ref_parses_uuid_or_name() {
        assert!(matches!(
            parse_resource_ref("9b2f0a34-1111-2222-3333-444455556666"),
            ResourceRef::Id(_)
        ));
        assert!(matches!(
            parse_resource_ref("prod-db-password"),
            ResourceRef::Name(_)
        ));
    }

    #[test]
    fn scope_labels() {
        let mut b = AppBinding {
            id: Uuid::nil(),
            app_id: Uuid::nil(),
            resource_type: BindingResourceType::Secret,
            resource_id: Uuid::nil(),
            env_var_name: "X".into(),
            container_name: None,
            environment_id: None,
            allow_in_preview: false,
            created_at: None,
        };
        let mut names = std::collections::HashMap::new();
        assert_eq!(scope_label(&b, &names), "app");
        b.allow_in_preview = true;
        assert_eq!(scope_label(&b, &names), "preview");
        b.environment_id = Some("env-1".into());
        names.insert("env-1".into(), "staging".into());
        assert_eq!(scope_label(&b, &names), "env:staging");
    }
}
