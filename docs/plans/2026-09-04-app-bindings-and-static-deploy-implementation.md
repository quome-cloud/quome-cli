# App Bindings + Static Deploy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `quome apps bindings|bind|unbind` (app resource bindings) and `quome deploy <dir>` (static-site upload), per `docs/plans/2026-09-04-app-bindings-and-static-deploy-design.md`.

**Architecture:** Both features ride existing quome-fastapi endpoints — no backend changes. Bindings: three new `AppsCommands` variants delegating to a new `src/commands/bindings.rs`, API methods on `QuomeClient` in `src/api/apps.rs`. Deploy: a new top-level `quome deploy` command orchestrating manifest walk (`src/manifest.rs`) → `POST static/deployments` → concurrent signed-URL PUTs → finalize → poll, ported behavior-for-behavior from the Python CLI in the quome-fastapi monorepo (`quome-cli/quome_cli/{main,manifest}.py` there — read it when porting).

**Tech Stack:** Rust, clap 4 (derive), tokio (JoinSet + Semaphore), reqwest, serde, tabled via `src/ui.rs`, `inquire::Confirm`, `indicatif`.

## Global Constraints

- Repo `/Users/rfsh/working/quome/quome-cli`, branch `feat/bindings-and-static-deploy`. Gate every task with `cargo fmt --check && cargo clippy -- -D warnings && cargo test` (matching `.github/workflows/ci.yml`).
- Follow the house command pattern exactly (see `src/commands/secrets.rs`): `Config::load()?` → `require_token()?` → org/app from args falling back to `require_linked_org()` / `require_linked_app()` → `QuomeClient::new(Some(&token), None)?` → `ui::spinner(...)` → API call → `--json` prints `serde_json::to_string_pretty`, otherwise `ui::print_table` / `ui::print_success`.
- Endpoints (existing): `GET|POST /api/v1/orgs/{org}/apps/{app}/bindings`, `DELETE /api/v1/orgs/{org}/apps/{app}/bindings/{binding_id}`; `POST /api/v1/orgs/{org}/apps` (create static app), `POST .../apps/{app}/static/sites`, `POST .../apps/{app}/static/deployments`, `POST .../static/deployments/{id}/finalize`, `GET .../apps/{app}/static/deployments`.
- Binding rules mirrored client-side (server stays authoritative): `env_var_name` matches `^[A-Z][A-Z0-9_]*$` (≤255); `--preview` is invalid with `--environment`; 403 on bindings gets the hint "managing bindings requires org admin".
- Resource flags: exactly one of `--secret|--database|--bucket|--cache` (clap `ArgGroup`); value is a UUID when parseable, else a name resolved via that type's list endpoint; `event_subscription` never offered.
- Deploy parity constants (from the Python original): poll every 2s with a 180s budget; terminal statuses `active`|`failed`; 8 concurrent uploads; manifest sends `{path, size}` per file; junk filters `__MACOSX`, `.DS_Store`, `Thumbs.db`; skip dirs `.git`, `node_modules`; keep meaningful dotfiles (e.g. `.well-known/`); root `index.html` required; MAX_FILES 5000; content-type overrides for `.js .mjs .css .svg .woff2 .woff .json .wasm .webp .avif`.
- Uploads use a BARE `reqwest::Client` (no `X-API-Key` default header — signed GCS URLs) with `Content-Type` from the manifest and 120s timeout.
- Commit messages: conventional, subject < 60 chars, no fluff adjectives, no emoji.

---

### Task 1: Binding models + API methods

**Files:**
- Modify: `src/api/models.rs` (append)
- Modify: `src/api/apps.rs` (append)

**Interfaces:**
- Produces (consumed by Task 2): `BindingResourceType` (serde snake_case enum), `AppBinding`, `CreateBindingRequest`; `QuomeClient::list_bindings(org_id: Uuid, app_id: Uuid) -> Result<Vec<AppBinding>>`, `create_binding(org_id, app_id, req: &CreateBindingRequest) -> Result<AppBinding>`, `delete_binding(org_id, app_id, binding_id: Uuid) -> Result<()>`.

- [ ] **Step 1: Write the failing serde tests**

Append to `src/api/models.rs`:

```rust
#[cfg(test)]
mod binding_tests {
    use super::*;

    #[test]
    fn app_binding_deserializes_api_row() {
        let row = r#"{
            "id": "9b2f0a34-1111-2222-3333-444455556666",
            "app_id": "aaaa0a34-1111-2222-3333-444455556666",
            "resource_type": "secret",
            "resource_id": "bbbb0a34-1111-2222-3333-444455556666",
            "env_var_name": "DATABASE_PASSWORD",
            "container_name": null,
            "environment_id": null,
            "allow_in_preview": false,
            "created_at": "2026-09-01T00:00:00Z"
        }"#;
        let b: AppBinding = serde_json::from_str(row).unwrap();
        assert_eq!(b.resource_type, BindingResourceType::Secret);
        assert_eq!(b.env_var_name, "DATABASE_PASSWORD");
        assert!(b.environment_id.is_none());
    }

    #[test]
    fn create_binding_request_serializes_snake_case() {
        let req = CreateBindingRequest {
            resource_type: BindingResourceType::Bucket,
            resource_id: uuid::Uuid::nil(),
            env_var_name: "ASSETS".into(),
            container_name: None,
            environment_id: None,
            allow_in_preview: true,
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["resource_type"], "bucket");
        assert_eq!(v["allow_in_preview"], true);
        // None fields serialize as null — the API treats null and absent alike.
        assert!(v.get("container_name").is_some());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test binding_tests`
Expected: compile FAIL — `AppBinding` etc. not found.

- [ ] **Step 3: Implement the models**

Append to `src/api/models.rs` (above the test module):

```rust
// ── App resource bindings ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingResourceType {
    Secret,
    Database,
    Bucket,
    Cache,
    // Created by event flows, never by `quome apps bind` — present so list
    // rows deserialize.
    EventSubscription,
}

impl BindingResourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Secret => "secret",
            Self::Database => "database",
            Self::Bucket => "bucket",
            Self::Cache => "cache",
            Self::EventSubscription => "event_subscription",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppBinding {
    pub id: Uuid,
    pub app_id: Uuid,
    pub resource_type: BindingResourceType,
    pub resource_id: Uuid,
    pub env_var_name: String,
    #[serde(default)]
    pub container_name: Option<String>,
    #[serde(default)]
    pub environment_id: Option<String>,
    #[serde(default)]
    pub allow_in_preview: bool,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct CreateBindingRequest {
    pub resource_type: BindingResourceType,
    pub resource_id: Uuid,
    pub env_var_name: String,
    pub container_name: Option<String>,
    pub environment_id: Option<String>,
    pub allow_in_preview: bool,
}
```

- [ ] **Step 4: Implement the API methods**

Append inside the `impl QuomeClient` block in `src/api/apps.rs`:

```rust
    pub async fn list_bindings(&self, org_id: Uuid, app_id: Uuid) -> Result<Vec<AppBinding>> {
        self.get(&format!("/api/v1/orgs/{}/apps/{}/bindings", org_id, app_id))
            .await
    }

    pub async fn create_binding(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        req: &CreateBindingRequest,
    ) -> Result<AppBinding> {
        self.post(&format!("/api/v1/orgs/{}/apps/{}/bindings", org_id, app_id), req)
            .await
    }

    pub async fn delete_binding(&self, org_id: Uuid, app_id: Uuid, binding_id: Uuid) -> Result<()> {
        self.delete(&format!(
            "/api/v1/orgs/{}/apps/{}/bindings/{}",
            org_id, app_id, binding_id
        ))
        .await
    }
```

Add `AppBinding, CreateBindingRequest` to the file's `use crate::api::models::{...}` import.

- [ ] **Step 5: Verify pass + gate**

Run: `cargo test binding_tests && cargo fmt && cargo clippy -- -D warnings`
Expected: 2 tests pass, clean.

- [ ] **Step 6: Commit**

```bash
git add src/api/models.rs src/api/apps.rs
git commit -m "feat(api): app binding models and client methods"
```

---

### Task 2: `quome apps bindings / bind / unbind`

**Files:**
- Create: `src/commands/bindings.rs`
- Modify: `src/commands/apps.rs` (three new `AppsCommands` variants + dispatch)
- Modify: `src/commands/mod.rs` (register `pub mod bindings;`)
- Modify: `src/ui.rs` (append `BindingRow`)
- Modify: `docs/reference/apps.md` (Bindings section)

**Interfaces:**
- Consumes: Task 1's models/methods; `Config::require_linked_org/require_linked_app` (`src/config.rs:189-205`); existing `QuomeClient::{list_secrets, list_apps}` and the databases/buckets/caches list methods (grep `src/api/` for `list_databases`, `list_buckets`/storage, `list_caches` — use whatever names exist; if a type has no list method yet, add one following `list_secrets`).
- Produces: `bindings::execute(BindingsCmd) -> Result<()>` internals are private; `apps.rs` exposes `AppsCommands::{Bindings, Bind, Unbind}`.

- [ ] **Step 1: Write the failing unit tests (pure logic)**

Create `src/commands/bindings.rs` starting with the pure helpers and their tests:

```rust
use clap::{ArgGroup, Parser};
use uuid::Uuid;

use crate::api::models::{AppBinding, BindingResourceType, CreateBindingRequest};
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
fn scope_label(binding: &AppBinding, env_names: &std::collections::HashMap<String, String>) -> String {
    if let Some(env_id) = &binding.environment_id {
        let name = env_names.get(env_id).cloned().unwrap_or_else(|| env_id.clone());
        return format!("env:{}", name);
    }
    if binding.allow_in_preview {
        return "preview".to_string();
    }
    "app".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_validation() {
        assert!(validate_env_var_name("DATABASE_PASSWORD").is_ok());
        assert!(validate_env_var_name("A1_B2").is_ok());
        let err = validate_env_var_name("database-password").unwrap_err();
        assert!(err.contains("DATABASE_PASSWORD"), "suggestion missing: {err}");
        assert!(validate_env_var_name("1BAD").is_err());
        assert!(validate_env_var_name("").is_err());
    }

    #[test]
    fn resource_ref_parses_uuid_or_name() {
        assert!(matches!(
            parse_resource_ref("9b2f0a34-1111-2222-3333-444455556666"),
            ResourceRef::Id(_)
        ));
        assert!(matches!(parse_resource_ref("prod-db-password"), ResourceRef::Name(_)));
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib bindings`
Expected: compile FAIL (`BindingRow` missing, module unregistered). Add `pub mod bindings;` to `src/commands/mod.rs` and the `BindingRow` below, then the tests compile and pass; the command wiring comes next.

Append to `src/ui.rs`:

```rust
#[derive(Tabled)]
pub struct BindingRow {
    #[tabled(rename = "ENV VAR")]
    pub env_var: String,
    #[tabled(rename = "TYPE")]
    pub resource_type: String,
    #[tabled(rename = "RESOURCE")]
    pub resource: String,
    #[tabled(rename = "SCOPE")]
    pub scope: String,
    #[tabled(rename = "BINDING ID")]
    pub id: String,
}
```

- [ ] **Step 3: Implement the three subcommands**

Add to `src/commands/bindings.rs` (below the helpers):

```rust
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
    /// Bind only for one app environment (environment UUID)
    #[arg(long)]
    environment: Option<Uuid>,
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
    /// Disambiguate --env-var to one environment's binding
    #[arg(long)]
    environment: Option<Uuid>,
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

fn resolve_context(
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

/// Resolve a resource flag to (type, id): UUIDs pass through, names go
/// through the type's list endpoint. Unknown names error with the list
/// command to run.
async fn resolve_resource(
    client: &QuomeClient,
    org_id: Uuid,
    resource_type: BindingResourceType,
    value: &str,
) -> Result<(BindingResourceType, Uuid, String)> {
    match parse_resource_ref(value) {
        ResourceRef::Id(id) => Ok((resource_type, id, value.to_string())),
        ResourceRef::Name(name) => {
            let (found, list_cmd): (Option<Uuid>, &str) = match resource_type {
                BindingResourceType::Secret => (
                    client
                        .list_secrets(org_id)
                        .await?
                        .data
                        .iter()
                        .find(|s| s.name == name)
                        .map(|s| s.id),
                    "quome secrets list",
                ),
                BindingResourceType::Database => (
                    client
                        .list_databases(org_id)
                        .await?
                        .data
                        .iter()
                        .find(|d| d.name == name)
                        .map(|d| d.id),
                    "quome databases list",
                ),
                BindingResourceType::Bucket => (
                    client
                        .list_buckets(org_id)
                        .await?
                        .data
                        .iter()
                        .find(|b| b.name == name)
                        .map(|b| b.id),
                    "quome storage list",
                ),
                BindingResourceType::Cache => (
                    client
                        .list_caches(org_id)
                        .await?
                        .data
                        .iter()
                        .find(|c| c.name == name)
                        .map(|c| c.id),
                    "quome caches list",
                ),
                BindingResourceType::EventSubscription => (None, ""),
            };
            match found {
                Some(id) => Ok((resource_type, id, name)),
                None => Err(QuomeError::NotFound(format!(
                    "no {} named '{}' — see `{}`",
                    resource_type.as_str(),
                    name,
                    list_cmd
                ))),
            }
        }
    }
}
```

NOTE on list methods: `list_secrets` exists. Check `src/api/` for databases / storage(buckets) / caches list methods and use their real names and row types. If one is missing, add it to the matching `src/api/*.rs` following `list_secrets` (`GET /api/v1/orgs/{org}/<databases:dbaas|storage|caches>` — confirm the exact paths against the existing api files / backend specs; dbaas is the databases path segment). Adjust the `resolve_resource` arms and the `.data` access to the real return shapes (`PaginatedResponse<T>` vs `Vec<T>`).

Then the handlers:

```rust
pub async fn list(args: BindingsArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let (org_id, app_id) = resolve_context(args.org, args.app, &config)?;
    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching bindings...");
    let bindings = client.list_bindings(org_id, app_id).await?;
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
        return Err(QuomeError::ApiError(msg));
    }
    if args.preview && args.environment.is_some() {
        return Err(QuomeError::ApiError(
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
                environment_id: args.environment.map(|e| e.to_string()),
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
                ("Resource", &format!("{} {}", resource_type.as_str(), resource_name)),
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
            let env = args.environment.map(|e| e.to_string());
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
                    eprintln!("'{}' matches multiple bindings:", name);
                    for b in &matches {
                        eprintln!("  {}  (scope varies)", b.id);
                    }
                    return Err(QuomeError::ApiError(
                        "pass the binding ID to unbind exactly one".into(),
                    ));
                }
            }
        }
        (None, None) => {
            return Err(QuomeError::ApiError(
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
        .map_err(|_| QuomeError::ApiError("cancelled".into()))?;
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
        println!("{}", serde_json::json!({"removed": target.id}));
    } else {
        ui::print_success(
            "Removed binding",
            &[("Env var", &target.env_var_name), ("Binding ID", &target.id.to_string())],
        );
    }
    Ok(())
}

/// Bindings are org-admin gated server-side on top of app permissions.
fn admin_hint(err: QuomeError) -> QuomeError {
    match err {
        QuomeError::ApiError(msg) if msg.to_lowercase().contains("admin") || msg.contains("403") => {
            QuomeError::ApiError(format!("{} (managing bindings requires org admin)", msg))
        }
        other => other,
    }
}
```

Plus the two name-resolution helpers used by `list`:

```rust
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
                if let Ok(resp) = client.list_secrets(org_id).await {
                    for s in resp.data {
                        names.insert((t, s.id), s.name);
                    }
                }
            }
            BindingResourceType::Database => {
                if let Ok(resp) = client.list_databases(org_id).await {
                    for d in resp.data {
                        names.insert((t, d.id), d.name);
                    }
                }
            }
            BindingResourceType::Bucket => {
                if let Ok(resp) = client.list_buckets(org_id).await {
                    for b in resp.data {
                        names.insert((t, b.id), b.name);
                    }
                }
            }
            BindingResourceType::Cache => {
                if let Ok(resp) = client.list_caches(org_id).await {
                    for c in resp.data {
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

/// environment_id → environment name, when any binding is env-scoped.
/// Uses GET /api/v1/orgs/{org}/apps/{app}/environments; falls back to raw
/// ids on any error (environments may be feature-gated off).
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
    if let Ok(envs) = client
        .get::<Vec<serde_json::Value>>(&format!(
            "/api/v1/orgs/{}/apps/{}/environments",
            org_id, app_id
        ))
        .await
    {
        for e in envs {
            if let (Some(id), Some(name)) = (e["id"].as_str(), e["name"].as_str()) {
                map.insert(id.to_string(), name.to_string());
            }
        }
    }
    map
}
```

(`BindingResourceType` needs `Hash` for the map key — add `Hash` to its derive list. `QuomeClient::get` is already `pub`.)

- [ ] **Step 4: Wire into `apps`**

In `src/commands/apps.rs`, add to `AppsCommands`:

```rust
    /// List resource bindings (env vars backed by secrets/databases/buckets/caches)
    Bindings(crate::commands::bindings::BindingsArgs),
    /// Bind a resource to the app as an env var
    Bind(crate::commands::bindings::BindArgs),
    /// Remove a resource binding
    Unbind(crate::commands::bindings::UnbindArgs),
```

and to its `execute` match:

```rust
        AppsCommands::Bindings(args) => crate::commands::bindings::list(args).await,
        AppsCommands::Bind(args) => crate::commands::bindings::bind(args).await,
        AppsCommands::Unbind(args) => crate::commands::bindings::unbind(args).await,
```

- [ ] **Step 5: Docs**

Append a `## Bindings` section to `docs/reference/apps.md` documenting the three subcommands with one example each (bind a secret, list showing the SCOPE column, unbind by env var), the env-var pattern, the `--preview` vs `--environment` exclusivity, and the org-admin requirement.

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: clean; bindings unit tests pass.

```bash
git add src/commands/bindings.rs src/commands/apps.rs src/commands/mod.rs src/ui.rs src/api docs/reference/apps.md
git commit -m "feat(apps): bind, unbind, bindings subcommands"
```

---

### Task 3: Manifest module (port of Python `manifest.py`)

**Files:**
- Create: `src/manifest.rs`
- Modify: `src/main.rs` (register `mod manifest;`)

**Interfaces:**
- Produces (consumed by Task 4): `ManifestEntry { path: String, size: u64, local: PathBuf }`, `build_manifest(root: &Path) -> Result<Vec<ManifestEntry>>`, `content_type_for(path: &str) -> &'static str` (or String), `MAX_FILES: usize = 5000`.

- [ ] **Step 1: Write the failing tests**

`src/manifest.rs` test module (use `tempfile` — add `tempfile = "3"` to `[dev-dependencies]` if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn site(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for f in files {
            let p = dir.path().join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
        }
        dir
    }

    #[test]
    fn requires_root_index_html() {
        let dir = site(&["about.html"]);
        let err = build_manifest(dir.path()).unwrap_err();
        assert!(err.to_string().contains("index.html"), "{err}");
    }

    #[test]
    fn walks_and_filters_junk() {
        let dir = site(&[
            "index.html",
            "assets/app.js",
            ".well-known/security.txt",
            ".DS_Store",
            "__MACOSX/resource",
            ".git/HEAD",
            "node_modules/pkg/index.js",
        ]);
        let entries = build_manifest(dir.path()).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"index.html"));
        assert!(paths.contains(&"assets/app.js"));
        assert!(paths.contains(&".well-known/security.txt"));
        assert_eq!(paths.len(), 3, "junk leaked: {paths:?}");
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type_for("a/app.js"), "text/javascript");
        assert_eq!(content_type_for("s.svg"), "image/svg+xml");
        assert_eq!(content_type_for("f.woff2"), "font/woff2");
        assert_eq!(content_type_for("x.html"), "text/html");
        assert_eq!(content_type_for("unknown.zzz"), "application/octet-stream");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib manifest`
Expected: compile FAIL — module doesn't exist yet (after creating the file with only tests, missing fns).

- [ ] **Step 3: Implement**

```rust
//! Directory → deploy manifest. Port of the Python CLI's manifest.py
//! (quome-fastapi monorepo): same junk filtering as the browser drop
//! pipeline plus filesystem-only exclusions (.git, node_modules).
//! Meaningful dotfiles (.well-known/) are kept. The root-index check
//! mirrors the server's validate_static_upload so a bad upload fails
//! before any bytes move.

use std::path::{Path, PathBuf};

use crate::errors::{QuomeError, Result};

pub const MAX_FILES: usize = 5000;

const JUNK_NAMES: &[&str] = &["__MACOSX", ".DS_Store", "Thumbs.db"];
const SKIP_DIRS: &[&str] = &[".git", "node_modules"];

#[derive(Debug, Clone)]
pub struct ManifestEntry {
    /// Forward-slash path relative to the site root.
    pub path: String,
    pub size: u64,
    pub local: PathBuf,
}

/// mimetype guessing is table-driven: pin the modern web types a wrong
/// guess would hurt, fall back to a small extension map, then
/// octet-stream.
pub fn content_type_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "html" | "htm" => "text/html",
        "txt" => "text/plain",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

pub fn build_manifest(root: &Path) -> Result<Vec<ManifestEntry>> {
    let mut entries = Vec::new();
    walk(root, root, &mut entries)?;
    if entries.len() > MAX_FILES {
        return Err(QuomeError::ApiError(format!(
            "{} files exceeds the {} file limit",
            entries.len(),
            MAX_FILES
        )));
    }
    if !entries.iter().any(|e| e.path == "index.html") {
        return Err(QuomeError::ApiError(
            "no index.html at the site root — deploy your build output directory".into(),
        ));
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<ManifestEntry>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if JUNK_NAMES.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        let meta = entry.metadata()?;
        if meta.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(root, &path, out)?;
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("walk stays under root")
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push(ManifestEntry {
                path: rel,
                size: meta.len(),
                local: path,
            });
        }
    }
    Ok(())
}
```

`QuomeError` needs a `From<std::io::Error>` — check `src/errors.rs`; if absent, add `#[error("io error: {0}")] Io(#[from] std::io::Error)`.

- [ ] **Step 4: Verify pass + gate**

Run: `cargo test --lib manifest && cargo fmt && cargo clippy -- -D warnings`
Expected: 3 tests pass, clean.

- [ ] **Step 5: Commit**

```bash
git add src/manifest.rs src/main.rs src/errors.rs Cargo.toml Cargo.lock
git commit -m "feat: static deploy manifest walk"
```

---

### Task 4: Static-sites API + `quome deploy`

**Files:**
- Create: `src/api/static_sites.rs` (+ register in `src/api/mod.rs`)
- Create: `src/commands/deploy.rs` (+ register in `src/commands/mod.rs`)
- Modify: `src/main.rs` (top-level `Deploy` command + dispatch)
- Modify: `src/api/models.rs` (static models)
- Create: `docs/reference/deploy.md`

**Interfaces:**
- Consumes: Task 3's `build_manifest`/`ManifestEntry`/`content_type_for`; `QuomeClient::{list_apps, get_app, create_app}` (existing; check `CreateAppRequest` supports the static source shape — the Python CLI posts `{"name": ..., "source": {"type": "static", "framework": "plain"}}`; extend `CreateAppRequest` with an optional `source` field if it lacks one).
- Produces: `quome deploy` end to end.

- [ ] **Step 1: Models + API methods (with serde test first)**

Append to `src/api/models.rs`:

```rust
// ── Static sites ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct StaticManifestFile {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct CreateStaticDeploymentRequest {
    pub source_type: &'static str, // always "api"
    pub files: Vec<StaticManifestFile>,
}

#[derive(Debug, Deserialize)]
pub struct StaticDeploymentSession {
    pub deployment_id: Uuid,
    pub upload_urls: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct StaticDeployment {
    pub id: Uuid,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}
```

Serde test (same file's test module):

```rust
    #[test]
    fn static_session_deserializes() {
        let body = r#"{"deployment_id":"9b2f0a34-1111-2222-3333-444455556666",
                       "upload_urls":{"index.html":"https://signed"},
                       "expires_at":"2026-09-04T00:00:00Z"}"#;
        let s: StaticDeploymentSession = serde_json::from_str(body).unwrap();
        assert_eq!(s.upload_urls["index.html"], "https://signed");
    }
```

Create `src/api/static_sites.rs`:

```rust
use uuid::Uuid;

use crate::api::models::{
    CreateStaticDeploymentRequest, StaticDeployment, StaticDeploymentSession,
};
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn create_or_get_static_site(&self, org_id: Uuid, app_id: Uuid) -> Result<serde_json::Value> {
        self.post(
            &format!("/api/v1/orgs/{}/apps/{}/static/sites", org_id, app_id),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn create_static_deployment(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        req: &CreateStaticDeploymentRequest,
    ) -> Result<StaticDeploymentSession> {
        self.post(
            &format!("/api/v1/orgs/{}/apps/{}/static/deployments", org_id, app_id),
            req,
        )
        .await
    }

    pub async fn finalize_static_deployment(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        deployment_id: Uuid,
    ) -> Result<serde_json::Value> {
        self.post(
            &format!(
                "/api/v1/orgs/{}/apps/{}/static/deployments/{}/finalize",
                org_id, app_id, deployment_id
            ),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn list_static_deployments(
        &self,
        org_id: Uuid,
        app_id: Uuid,
    ) -> Result<Vec<StaticDeployment>> {
        self.get(&format!(
            "/api/v1/orgs/{}/apps/{}/static/deployments",
            org_id, app_id
        ))
        .await
    }
}
```

NOTE: confirm the list endpoint's envelope by reading the backend response model (quome-fastapi `app/api/v1/apps/static_sites.py` GET route) — if it returns `{data: [...]}` or a `PaginatedResponse`, wrap accordingly (the Python client iterated rows directly).

- [ ] **Step 2: The deploy command**

Create `src/commands/deploy.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::api::models::{CreateStaticDeploymentRequest, StaticManifestFile};
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::{QuomeError, Result};
use crate::manifest::{build_manifest, content_type_for, ManifestEntry};
use crate::ui;

const UPLOAD_WORKERS: usize = 8;
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const POLL_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Parser)]
pub struct DeployArgs {
    /// Site root to deploy (your build output — must contain index.html)
    #[arg(default_value = ".")]
    directory: PathBuf,
    /// App slug or UUID (uses linked app if not provided)
    #[arg(long, short)]
    app: Option<String>,
    /// Create the app if the slug doesn't exist yet
    #[arg(long)]
    create: bool,
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(args: DeployArgs) -> Result<()> {
    let entries = build_manifest(&args.directory.canonicalize().map_err(|_| {
        QuomeError::ApiError(format!("directory not found: {}", args.directory.display()))
    })?)?;

    let config = Config::load()?;
    let token = config.require_token()?;
    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };
    let client = QuomeClient::new(Some(&token), None)?;

    let (app_id, app_label) = resolve_app(&client, org_id, &config, &args).await?;

    let total_bytes: u64 = entries.iter().map(|e| e.size).sum();
    if !args.json {
        println!(
            "Deploying {} files ({} bytes) to {}",
            entries.len(),
            total_bytes,
            app_label
        );
    }

    let sp = ui::spinner("Starting deployment...");
    client.create_or_get_static_site(org_id, app_id).await?;
    let session = client
        .create_static_deployment(
            org_id,
            app_id,
            &CreateStaticDeploymentRequest {
                source_type: "api",
                files: entries
                    .iter()
                    .map(|e| StaticManifestFile {
                        path: e.path.clone(),
                        size: e.size,
                    })
                    .collect(),
            },
        )
        .await?;
    sp.finish_and_clear();

    upload_all(&entries, &session.upload_urls, total_bytes, args.json).await?;

    let sp = ui::spinner("Finalizing...");
    client
        .finalize_static_deployment(org_id, app_id, session.deployment_id)
        .await?;
    let row = poll_until_terminal(&client, org_id, app_id, session.deployment_id).await?;
    sp.finish_and_clear();

    if row.status == "failed" {
        return Err(QuomeError::ApiError(format!(
            "deploy failed: {}",
            row.error.unwrap_or_else(|| "unknown error".into())
        )));
    }

    let url = client.get_app(org_id, app_id).await.ok().and_then(|a| a.primary_url);
    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "deployment_id": session.deployment_id,
                "status": row.status,
                "url": url,
            })
        );
    } else {
        match url {
            Some(u) => ui::print_success("Deployed", &[("URL", &u)]),
            None => println!("Deployed. URL pending — check the app page in the dashboard."),
        }
    }
    Ok(())
}

/// --app slug|uuid → app id, falling back to the linked app; --create makes
/// a static app when the slug doesn't resolve.
async fn resolve_app(
    client: &QuomeClient,
    org_id: Uuid,
    config: &Config,
    args: &DeployArgs,
) -> Result<(Uuid, String)> {
    let Some(app_ref) = &args.app else {
        let id = config.require_linked_app()?;
        return Ok((id, id.to_string()));
    };
    if let Ok(id) = Uuid::parse_str(app_ref) {
        return Ok((id, app_ref.clone()));
    }
    let apps = client.list_apps(org_id).await?;
    if let Some(app) = apps
        .data
        .iter()
        .find(|a| a.slug.as_deref() == Some(app_ref.as_str()))
    {
        return Ok((app.id, app_ref.clone()));
    }
    if args.create {
        println!("App {} not found — creating it.", app_ref);
        let app = client.create_static_app(org_id, app_ref).await?;
        return Ok((app.id, app_ref.clone()));
    }
    let slugs: Vec<String> = apps
        .data
        .iter()
        .filter_map(|a| a.slug.clone())
        .collect();
    Err(QuomeError::NotFound(format!(
        "no app with slug or id '{}'. Existing: {}. Pass --create to create it.",
        app_ref,
        if slugs.is_empty() { "(none)".into() } else { slugs.join(", ") }
    )))
}

async fn upload_all(
    entries: &[ManifestEntry],
    upload_urls: &std::collections::HashMap<String, String>,
    total_bytes: u64,
    quiet: bool,
) -> Result<()> {
    // Bare client: signed GCS URLs — the X-API-Key default header must NOT
    // be sent, and Content-Type must match the manifest's type.
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| QuomeError::ApiError(e.to_string()))?;

    let bar = if quiet {
        ProgressBar::hidden()
    } else {
        let b = ProgressBar::new(total_bytes);
        b.set_style(
            ProgressStyle::with_template("{bar:30} {bytes}/{total_bytes} {bytes_per_sec}")
                .expect("static template"),
        );
        b
    };

    let sem = Arc::new(Semaphore::new(UPLOAD_WORKERS));
    let mut set: JoinSet<Result<u64>> = JoinSet::new();
    for entry in entries {
        let url = upload_urls
            .get(&entry.path)
            .ok_or_else(|| QuomeError::ApiError(format!("no upload URL for {}", entry.path)))?
            .clone();
        let http = http.clone();
        let sem = sem.clone();
        let path = entry.path.clone();
        let local = entry.local.clone();
        let size = entry.size;
        let content_type = content_type_for(&path);
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore open");
            let body = tokio::fs::read(&local)
                .await
                .map_err(|e| QuomeError::ApiError(format!("read {}: {}", path, e)))?;
            let resp = http
                .put(&url)
                .header("Content-Type", content_type)
                .body(body)
                .send()
                .await
                .map_err(|e| QuomeError::ApiError(format!("upload {}: {}", path, e)))?;
            if !resp.status().is_success() {
                return Err(QuomeError::ApiError(format!(
                    "upload failed for {}: HTTP {}",
                    path,
                    resp.status()
                )));
            }
            Ok(size)
        });
    }
    while let Some(joined) = set.join_next().await {
        let size = joined.map_err(|e| QuomeError::ApiError(e.to_string()))??;
        bar.inc(size);
    }
    bar.finish_and_clear();
    Ok(())
}

async fn poll_until_terminal(
    client: &QuomeClient,
    org_id: Uuid,
    app_id: Uuid,
    deployment_id: Uuid,
) -> Result<crate::api::models::StaticDeployment> {
    let deadline = Instant::now() + POLL_TIMEOUT;
    while Instant::now() < deadline {
        let rows = client.list_static_deployments(org_id, app_id).await?;
        if let Some(row) = rows.into_iter().find(|r| r.id == deployment_id) {
            if row.status == "active" || row.status == "failed" {
                return Ok(row);
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(QuomeError::ApiError(format!(
        "deploy did not reach a terminal state within {}s",
        POLL_TIMEOUT.as_secs()
    )))
}
```

`create_static_app` goes in `src/api/apps.rs`:

```rust
    /// Create a plain static app (the shape the Python CLI's --create used).
    pub async fn create_static_app(&self, org_id: Uuid, name: &str) -> Result<App> {
        self.post(
            &format!("/api/v1/orgs/{}/apps", org_id),
            &serde_json::json!({"name": name, "source": {"type": "static", "framework": "plain"}}),
        )
        .await
    }
```

`App` needs `primary_url: Option<String>` — add it (`#[serde(default)]`) if missing.

- [ ] **Step 3: Wire the top-level command**

In `src/main.rs` `Commands` enum: `/// Deploy a directory of static files` → `Deploy(commands::deploy::DeployArgs)`, dispatch `Commands::Deploy(args) => commands::deploy::execute(args).await`. Register `pub mod deploy;` in `src/commands/mod.rs` and `mod static_sites;` (or `pub mod`) in `src/api/mod.rs`.

- [ ] **Step 4: Docs**

Create `docs/reference/deploy.md`: synopsis, the index.html requirement, `--app`/`--create`, junk filtering, the admin-scope key requirement (deploy permissions resolve to admin — `*` or `admin:app` scope), and a two-command example (`quome login`, `quome deploy ./dist --app my-site --create`). Add the page to `docs/reference/README.md`'s index.

- [ ] **Step 5: Gate + commit**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: clean; all prior tests + the new serde test pass.

```bash
git add src/api src/commands src/main.rs src/ui.rs docs/reference Cargo.toml Cargo.lock
git commit -m "feat: quome deploy for static sites"
```

---

### Task 5: Live verification + PR

- [ ] **Step 1: Manual verification against dev** (controller/user-assisted; needs a dev API key)

```bash
cargo run -- login            # if not already logged in
cargo run -- link             # link a test org/app
cargo run -- apps bind --env-var CLI_TEST_SECRET --secret <existing-secret-name>
cargo run -- apps bindings    # row shows env var, secret name, scope app
cargo run -- apps unbind --env-var CLI_TEST_SECRET --force
mkdir -p /tmp/cli-site && printf '<h1>hi</h1>' > /tmp/cli-site/index.html
cargo run -- deploy /tmp/cli-site --app cli-deploy-test --create
# expect: progress bar, "Deployed", live URL serving the page
```

Record each command's output in the task report. Any server-behavior mismatch with the plan (envelopes, paths) gets fixed in the api layer and noted.

- [ ] **Step 2: PR**

```bash
git push -u origin feat/bindings-and-static-deploy
gh pr create --repo quome-cloud/quome-cli --base main \
  --title "feat: app bindings + static-site deploy" \
  --body "<summary: the two command groups, endpoints used, parity notes with the Python CLI, verification transcript>"
```

The quome-fastapi cleanup PR (delete the Python `quome-cli/` dir + rewrite the specs.md paragraph) happens after this merges and is NOT part of this plan.
