# Environments + Env-Vars Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `quome apps envs` (list/create/delete/promote/config) and `quome apps env-vars` (list/set/unset, app/env/sidecar scopes), per `docs/plans/2026-09-04-envs-and-env-vars-design.md`.

**Architecture:** A typed environments API layer + a shared name-or-UUID environment resolver feed two new command modules. Env-var writes follow the incident-shaped rules: app-level mutates ONLY `env_vars` inside an opaque `serde_json::Value` spec (no typed spec model exists or may be added), per-env reads-merges-writes a single `config_overrides` sub-map against the server's top-level merge-patch.

**Tech Stack:** Rust, clap 4 derive, tokio, serde_json, existing `QuomeClient` verbs + `list_all_pages`, `src/ui.rs` tables, `inquire`.

## Global Constraints

- Repo `/Users/rfsh/working/quome/quome-cli`, branch `feat/envs-and-env-vars` (stacked on `feat/bindings-and-static-deploy`; the PR targets that branch until quome-cli#6 merges, then retargets main). Gate every task: `cargo fmt --check && cargo clippy -- -D warnings && cargo test`.
- **NEVER define a typed Rust model for the app spec.** App-level env-var writes GET the app as raw `serde_json::Value`, mutate only `spec.env_vars` (or one sidecar's `env_vars`, located by its `name` field in the `spec.sidecars` LIST), and PUT `{"spec": <whole untouched Value>}`. Tests assert the rest of the spec is byte-identical.
- Per-env writes PATCH `{"config_overrides": {<one touched top-level key>: <full merged sub-map | null>}}` — never send untouched top-level keys; `null` deletes a key. The server merge-patches top-level keys only.
- `envs config` refuses `env_vars` / `sidecar_env_vars` keys (they belong to `env-vars`) and requires `--environment` on all verbs.
- KEY grammar client-side: `^[A-Za-z_][A-Za-z0-9_]*$` + reject the reserved `QUOME_` prefix; `KEY=VALUE` splits on the FIRST `=`. Server stays authoritative.
- Env references resolve name-first, then UUID; unknown → `no environment '<ref>' — see \`quome apps envs\``. The resolver also retrofits `bind`/`unbind --environment` (currently `Option<Uuid>`) to name-or-UUID.
- Promote gate: on a 403 whose detail contains "gated", prompt (TTY only) "Type '<target-name>' to confirm:" and retry ONCE with the typed `gate_ack`; non-TTY gets the 403 verbatim + hint to pass `--gate-ack <target-name>`.
- House patterns: `Config::load → require_token → org/app fallback → QuomeClient::new → ui::spinner → table or to_string_pretty JSON`; client-detected errors use `QuomeError::Usage`; confirms use `inquire::Confirm` with `--force` bypass.
- Backend endpoints (existing, verified): `GET|POST /api/v1/orgs/{org}/apps/{app}/environments` (GET paginated; POST body `{name, deploy_branch?, auto_deploy, copy_vars_from_environment_id?}` → env row), `PATCH|DELETE .../environments/{env_id}` (PATCH body `{config_overrides: ...}` etc.; DELETE returns a JSON body — treat as `delete()` success), `POST .../environments/{env_id}/promote` body `{from_environment_id, gate_ack?}` → env row. Env rows: `{id, app_id, organization_id, name, slug, is_default, kind, deploy_branch?, auto_deploy, status, sort_order, promotion_gate, color?, config_overrides, primary_url?, current_deployment_id?, last_deployed_at?, created_at?}`.
- Writes print the changed keys and `applies on the next deploy`.
- Commit messages: conventional, <60 chars, no fluff adjectives, no emoji. Never stage `.superpowers/`.

---

### Task 1: Environments API layer + shared resolver

**Files:**
- Create: `src/api/environments.rs` (+ register in `src/api/mod.rs`)
- Modify: `src/api/models.rs` (append `AppEnvironment`, `CreateEnvironmentRequest`, `UpdateEnvironmentRequest`, `PromoteEnvironmentRequest` + serde tests)
- Create: `src/commands/envs.rs` (resolver only in this task; commands come in Task 2) (+ register `pub mod envs;` in `src/commands/mod.rs`)

**Interfaces:**
- Produces: `AppEnvironment { id: Uuid, name: String, slug: String, is_default: bool, deploy_branch: Option<String>, auto_deploy: bool, status: String, sort_order: i64, promotion_gate: String, config_overrides: serde_json::Value }` (extra response fields tolerated by serde default behavior);
  `QuomeClient::list_environments(org, app) -> Result<Vec<AppEnvironment>>` (via `list_all_pages`),
  `create_environment(org, app, &CreateEnvironmentRequest) -> Result<AppEnvironment>`,
  `delete_environment(org, app, env) -> Result<()>`,
  `update_environment_overrides(org, app, env, overrides: &serde_json::Value) -> Result<AppEnvironment>`,
  `promote_environment(org, app, target_env, &PromoteEnvironmentRequest) -> Result<AppEnvironment>`;
  `envs::resolve_environment(client, org, app, ref_str) -> Result<AppEnvironment>` — the shared resolver Tasks 2-5 and the bindings retrofit consume.

- [ ] **Step 1: Write the failing serde tests**

Append to `src/api/models.rs` test module:

```rust
    #[test]
    fn app_environment_deserializes_api_row() {
        let row = r#"{
            "id": "9b2f0a34-1111-2222-3333-444455556666",
            "app_id": "aaaa0a34-1111-2222-3333-444455556666",
            "organization_id": "bbbb0a34-1111-2222-3333-444455556666",
            "name": "staging", "slug": "staging", "is_default": false,
            "kind": "persistent", "deploy_branch": "staging",
            "auto_deploy": true, "status": "active", "sort_order": 2,
            "promotion_gate": "confirm", "color": "blue",
            "config_overrides": {"env_vars": {"A": "1"}, "memory": "1Gi"},
            "primary_url": null, "current_deployment_id": null,
            "last_deployed_at": null, "created_at": "2026-09-01T00:00:00Z"
        }"#;
        let e: AppEnvironment = serde_json::from_str(row).unwrap();
        assert_eq!(e.name, "staging");
        assert_eq!(e.promotion_gate, "confirm");
        assert_eq!(e.config_overrides["env_vars"]["A"], "1");
    }

    #[test]
    fn create_environment_request_serializes() {
        let req = CreateEnvironmentRequest {
            name: "staging".into(),
            deploy_branch: Some("staging".into()),
            auto_deploy: true,
            copy_vars_from_environment_id: None,
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["name"], "staging");
        assert_eq!(v["auto_deploy"], true);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test app_environment`
Expected: compile FAIL — types not found.

- [ ] **Step 3: Implement models**

Append to `src/api/models.rs`:

```rust
// ── App environments ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppEnvironment {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub is_default: bool,
    #[serde(default)]
    pub deploy_branch: Option<String>,
    pub auto_deploy: bool,
    pub status: String,
    #[serde(default)]
    pub sort_order: i64,
    #[serde(default = "default_promotion_gate")]
    pub promotion_gate: String,
    #[serde(default)]
    pub config_overrides: serde_json::Value,
}

fn default_promotion_gate() -> String {
    "none".to_string()
}

#[derive(Debug, Serialize)]
pub struct CreateEnvironmentRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deploy_branch: Option<String>,
    pub auto_deploy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_vars_from_environment_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PromoteEnvironmentRequest {
    pub from_environment_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_ack: Option<String>,
}
```

- [ ] **Step 4: Implement the API file + resolver**

Create `src/api/environments.rs`:

```rust
use uuid::Uuid;

use crate::api::models::{AppEnvironment, CreateEnvironmentRequest, PromoteEnvironmentRequest};
use crate::client::QuomeClient;
use crate::errors::Result;

impl QuomeClient {
    pub async fn list_environments(
        &self,
        org_id: Uuid,
        app_id: Uuid,
    ) -> Result<Vec<AppEnvironment>> {
        self.list_all_pages(&format!(
            "/api/v1/orgs/{}/apps/{}/environments",
            org_id, app_id
        ))
        .await
    }

    pub async fn create_environment(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        req: &CreateEnvironmentRequest,
    ) -> Result<AppEnvironment> {
        self.post(
            &format!("/api/v1/orgs/{}/apps/{}/environments", org_id, app_id),
            req,
        )
        .await
    }

    pub async fn delete_environment(&self, org_id: Uuid, app_id: Uuid, env_id: Uuid) -> Result<()> {
        self.delete(&format!(
            "/api/v1/orgs/{}/apps/{}/environments/{}",
            org_id, app_id, env_id
        ))
        .await
    }

    /// PATCH the environment. Callers send ONLY the top-level keys they
    /// touched — the server merge-patches config_overrides at the top level
    /// (null deletes a key), so untouched keys must never ride along.
    pub async fn update_environment_overrides(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        env_id: Uuid,
        config_overrides: &serde_json::Value,
    ) -> Result<AppEnvironment> {
        self.patch(
            &format!(
                "/api/v1/orgs/{}/apps/{}/environments/{}",
                org_id, app_id, env_id
            ),
            &serde_json::json!({ "config_overrides": config_overrides }),
        )
        .await
    }

    pub async fn promote_environment(
        &self,
        org_id: Uuid,
        app_id: Uuid,
        target_env_id: Uuid,
        req: &PromoteEnvironmentRequest,
    ) -> Result<AppEnvironment> {
        self.post(
            &format!(
                "/api/v1/orgs/{}/apps/{}/environments/{}/promote",
                org_id, app_id, target_env_id
            ),
            req,
        )
        .await
    }
}
```

Check `src/client.rs` for a generic `patch` verb; if absent, add one mirroring `put`:

```rust
    pub async fn patch<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T> {
        let response = self.http.patch(self.url(path)).json(body).send().await?;
        self.handle_response(response).await
    }
```

Create `src/commands/envs.rs` with just the resolver (commands in Task 2):

```rust
use uuid::Uuid;

use crate::api::models::AppEnvironment;
use crate::client::QuomeClient;
use crate::errors::{QuomeError, Result};

/// Resolve an environment reference: exact name match first, then UUID.
/// Env names are slug-validated and unique per app, so no ambiguity.
pub async fn resolve_environment(
    client: &QuomeClient,
    org_id: Uuid,
    app_id: Uuid,
    env_ref: &str,
) -> Result<AppEnvironment> {
    let envs = client.list_environments(org_id, app_id).await?;
    if let Some(env) = envs.iter().find(|e| e.name == env_ref) {
        return Ok(env.clone());
    }
    if let Ok(id) = Uuid::parse_str(env_ref) {
        if let Some(env) = envs.iter().find(|e| e.id == id) {
            return Ok(env.clone());
        }
    }
    Err(QuomeError::NotFound(format!(
        "no environment '{}' — see `quome apps envs`",
        env_ref
    )))
}
```

- [ ] **Step 5: Verify + gate + commit**

Run: `cargo test app_environment && cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: new tests pass, everything else unchanged. (`#[allow(dead_code)]` on not-yet-consumed items per house convention; later tasks remove them.)

```bash
git add src/api/models.rs src/api/environments.rs src/api/mod.rs src/commands/envs.rs src/commands/mod.rs src/client.rs
git commit -m "feat(api): environments client and resolver"
```

---

### Task 2: `envs` list / create / delete + bindings retrofit

**Files:**
- Modify: `src/commands/envs.rs` (subcommand tree + three handlers)
- Modify: `src/commands/apps.rs` (`Envs` variant + dispatch)
- Modify: `src/commands/bindings.rs` (`--environment: Option<Uuid>` → `Option<String>` resolved via `resolve_environment`)
- Modify: `src/ui.rs` (append `EnvRow`)
- Test: unit tests in `envs.rs`

**Interfaces:**
- Consumes: Task 1's API methods + resolver.
- Produces: `EnvsCommands` enum with `List/Create/Delete/Promote/Config` variants (Promote/Config stubs REJECTED here — they land in Task 3; wire only List/Create/Delete now and add the other two variants in Task 3 so each task compiles standalone).

- [ ] **Step 1: `EnvRow` + subcommand tree + handlers**

Append to `src/ui.rs`:

```rust
#[derive(Tabled)]
pub struct EnvRow {
    #[tabled(rename = "NAME")]
    pub name: String,
    #[tabled(rename = "SLUG")]
    pub slug: String,
    #[tabled(rename = "DEFAULT")]
    pub default: String,
    #[tabled(rename = "BRANCH")]
    pub branch: String,
    #[tabled(rename = "AUTO")]
    pub auto: String,
    #[tabled(rename = "STATUS")]
    pub status: String,
    #[tabled(rename = "ID")]
    pub id: String,
}
```

Extend `src/commands/envs.rs`:

```rust
use clap::{Parser, Subcommand};

use crate::api::models::CreateEnvironmentRequest;
use crate::config::Config;
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
    #[arg(long)]
    app: Option<Uuid>,
    #[arg(long)]
    org: Option<Uuid>,
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct EnvDeleteArgs {
    /// Environment to delete (name or UUID)
    env: String,
    #[arg(long)]
    app: Option<Uuid>,
    #[arg(long)]
    org: Option<Uuid>,
    /// Skip confirmation prompt
    #[arg(short, long)]
    force: bool,
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
            default: if e.is_default { "yes".into() } else { "".into() },
            branch: e.deploy_branch.clone().unwrap_or_default(),
            auto: if e.auto_deploy { "yes".into() } else { "no".into() },
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
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({"deleted": env.id}))?);
    } else {
        ui::print_success("Deleted environment", &[("Name", &env.name)]);
    }
    Ok(())
}
```

(If `QuomeError::Usage`'s actual variant name differs, use the real one — it exists on the base branch; check `src/errors.rs`. The default-env delete guard: verify the backend rejects it too — pass its 4xx through if the client-side check is ever bypassed.)

- [ ] **Step 2: Wire into apps + retrofit bindings**

`src/commands/apps.rs`:

```rust
    /// Manage app environments
    Envs {
        #[command(subcommand)]
        command: crate::commands::envs::EnvsCommands,
    },
```
dispatch: `AppsCommands::Envs { command } => crate::commands::envs::execute(command).await,`

`src/commands/bindings.rs`: change `BindArgs.environment` and `UnbindArgs.environment` to `Option<String>`; where they were used, insert:

```rust
    let environment_id = match &args.environment {
        Some(env_ref) => Some(
            crate::commands::envs::resolve_environment(&client, org_id, app_id, env_ref)
                .await?
                .id,
        ),
        None => None,
    };
```
and thread `environment_id.map(|e| e.to_string())` into the create payload / unbind scope filter exactly where the Uuid was used before. Update the flag docs (`/// Bind only for one app environment (name or UUID)`), `docs/reference/apps.md` mentions, and any bindings unit tests pinned to Uuid parsing.

- [ ] **Step 3: Unit test the resolver**

In `src/commands/envs.rs` tests — the resolver needs a client, so test the pure parts: sort order rendering and the name-vs-uuid precedence via a small extracted helper:

```rust
/// Pure matcher the resolver delegates to (unit-testable without a client).
fn match_environment<'a>(envs: &'a [AppEnvironment], env_ref: &str) -> Option<&'a AppEnvironment> {
    if let Some(e) = envs.iter().find(|e| e.name == env_ref) {
        return Some(e);
    }
    Uuid::parse_str(env_ref)
        .ok()
        .and_then(|id| envs.iter().find(|e| e.id == id))
}
```
(refactor `resolve_environment` to call it), with tests: name hit, uuid hit, name shadows uuid-looking name, miss → None.

- [ ] **Step 4: Gate + commit**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`

```bash
git add src/commands/envs.rs src/commands/apps.rs src/commands/bindings.rs src/ui.rs docs/reference/apps.md
git commit -m "feat(apps): envs list, create, delete; env names in bind"
```

---

### Task 3: `envs promote` + `envs config`

**Files:**
- Modify: `src/commands/envs.rs` (two new variants + handlers + gate/config helpers with tests)

**Interfaces:**
- Consumes: Task 1's `promote_environment`/`update_environment_overrides` + resolver; Task 2's `EnvsCommands`/`resolve_context`.

- [ ] **Step 1: Failing unit tests for the pure helpers**

```rust
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
        assert!(is_gate_denial("This environment is gated. Type the environment name ('production') to confirm."));
        assert!(!is_gate_denial("Permission denied: update on app"));
    }
```

- [ ] **Step 2: Implement**

Helpers:

```rust
fn parse_config_value(raw: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .filter(|v| v.is_number() || v.is_boolean())
        .unwrap_or_else(|| serde_json::Value::String(raw.to_string()))
}

fn validate_config_key(key: &str) -> std::result::Result<(), String> {
    if key == "env_vars" || key == "sidecar_env_vars" {
        return Err(format!(
            "'{}' is managed by `quome apps env-vars`, not `envs config`",
            key
        ));
    }
    Ok(())
}

fn is_gate_denial(detail: &str) -> bool {
    detail.contains("gated")
}
```

New variants:

```rust
    /// Promote a source environment's exact image to a target environment
    Promote(EnvPromoteArgs),
    /// Show or edit an environment's build/runtime override keys
    Config(EnvConfigArgs),
```

```rust
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
    #[arg(long)]
    app: Option<Uuid>,
    #[arg(long)]
    org: Option<Uuid>,
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
    #[arg(long, global = true)]
    app: Option<Uuid>,
    #[arg(long, global = true)]
    org: Option<Uuid>,
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
```

Promote handler:

```rust
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
            if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
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
                ("Note", "target deploys the source's exact image digest (no rebuild)"),
            ],
        );
    }
    Ok(())
}
```

(Gate denials may surface as `QuomeError::ApiError` OR a dedicated 403 variant depending on `error_from_response` — check `src/client.rs` and match the real variant carrying the detail string.)

Config handler (`--environment` enforced at runtime since clap globals can't be required):

```rust
async fn config_cmd(args: EnvConfigArgs) -> Result<()> {
    let env_ref = args.environment.as_deref().ok_or_else(|| {
        QuomeError::Usage("--environment is required for envs config".into())
    })?;
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
                return Err(QuomeError::Usage("set requires at least one KEY=VALUE".into()));
            }
            let mut patch = serde_json::Map::new();
            for pair in &pairs {
                let (key, value) = pair.split_once('=').ok_or_else(|| {
                    QuomeError::Usage(format!("'{}' is not KEY=VALUE", pair))
                })?;
                validate_config_key(key).map_err(QuomeError::Usage)?;
                patch.insert(key.to_string(), parse_config_value(value));
            }
            let summary = patch.keys().cloned().collect::<Vec<_>>().join(", ");
            client
                .update_environment_overrides(org_id, app_id, env.id, &serde_json::Value::Object(patch))
                .await?;
            println!("Set {} on '{}' — applies on the next deploy", summary, env.name);
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
                .update_environment_overrides(org_id, app_id, env.id, &serde_json::Value::Object(patch))
                .await?;
            println!("Unset {} on '{}' — applies on the next deploy", keys.join(", "), env.name);
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Gate + commit**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`

```bash
git add src/commands/envs.rs
git commit -m "feat(apps): envs promote and config overrides"
```

---

### Task 4: Env-var core helpers (pure, heavily tested)

**Files:**
- Create: `src/commands/env_vars.rs` (helpers + tests only; commands in Task 5) (+ register in `src/commands/mod.rs`)

**Interfaces:**
- Produces (Task 5 consumes): `parse_pairs(&[String]) -> Result<Vec<(String, String)>>`, `validate_key(&str) -> Result<(), String>`, `mutate_spec_env_vars(spec: &mut serde_json::Value, container: Option<&str>, set: &[(String, String)], unset: &[String]) -> Result<Vec<String>>` (returns changed keys; errors on unknown container listing the spec's sidecar names, and on unset-of-absent-key naming the scope), `merged_overrides_map(overrides: &serde_json::Value, container: Option<&str>, set: &[(String, String)], unset: &[String]) -> Result<(String, serde_json::Value)>` (returns the touched top-level key name — `env_vars` or `sidecar_env_vars` — and its full new value, `Null` when emptied), `effective_rows(app_env_vars: &serde_json::Value, override_env_vars: &serde_json::Value) -> Vec<(String, String, &'static str)>` (key, value, `"app"|"env"`, sorted by key).

- [ ] **Step 1: Write the failing tests** (the heart of the incident-safety story)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec_fixture() -> serde_json::Value {
        json!({
            "env_vars": {"KEEP": "1", "EDIT": "old"},
            "sidecars": [
                {"name": "worker", "image": "x", "env_vars": {"W": "1"},
                 "unknown_future_field": {"nested": true}},
                {"name": "cache-warm", "image": "y"}
            ],
            "unknown_top_level": "must-survive",
            "use_managed_db": true
        })
    }

    #[test]
    fn key_grammar() {
        assert!(validate_key("GOOD_KEY").is_ok());
        assert!(validate_key("lower_ok").is_ok());
        assert!(validate_key("_LEAD").is_ok());
        assert!(validate_key("1BAD").is_err());
        assert!(validate_key("BAD-DASH").is_err());
        assert!(validate_key("QUOME_RESERVED").is_err());
    }

    #[test]
    fn pairs_split_on_first_equals() {
        let pairs = parse_pairs(&["A=b=c".into(), "B=".into()]).unwrap();
        assert_eq!(pairs[0], ("A".into(), "b=c".into()));
        assert_eq!(pairs[1], ("B".into(), "".into()));
        assert!(parse_pairs(&["NOEQUALS".into()]).is_err());
    }

    #[test]
    fn spec_mutation_touches_only_env_vars() {
        let mut spec = spec_fixture();
        let before = spec.clone();
        let changed = mutate_spec_env_vars(
            &mut spec,
            None,
            &[("NEW".into(), "v".into()), ("EDIT".into(), "new".into())],
            &["KEEP".to_string()],
        )
        .unwrap();
        assert_eq!(changed, vec!["NEW", "EDIT", "KEEP"]);
        assert_eq!(spec["env_vars"], json!({"NEW": "v", "EDIT": "new"}));
        // Everything except env_vars is byte-identical.
        let mut reverted = spec.clone();
        reverted["env_vars"] = before["env_vars"].clone();
        assert_eq!(reverted, before);
    }

    #[test]
    fn spec_mutation_sidecar_by_name_field() {
        let mut spec = spec_fixture();
        mutate_spec_env_vars(&mut spec, Some("worker"), &[("W2".into(), "2".into())], &[]).unwrap();
        assert_eq!(spec["sidecars"][0]["env_vars"], json!({"W": "1", "W2": "2"}));
        assert_eq!(spec["sidecars"][0]["unknown_future_field"], json!({"nested": true}));
        // Sidecar without env_vars gets one created.
        let mut spec2 = spec_fixture();
        mutate_spec_env_vars(&mut spec2, Some("cache-warm"), &[("A".into(), "1".into())], &[]).unwrap();
        assert_eq!(spec2["sidecars"][1]["env_vars"], json!({"A": "1"}));
        // Unknown container errors, naming the candidates.
        let err = mutate_spec_env_vars(&mut spec, Some("nope"), &[("A".into(), "1".into())], &[])
            .unwrap_err();
        assert!(err.to_string().contains("worker"), "{err}");
    }

    #[test]
    fn unset_of_absent_key_errors_with_scope() {
        let mut spec = spec_fixture();
        let err =
            mutate_spec_env_vars(&mut spec, None, &[], &["MISSING".to_string()]).unwrap_err();
        assert!(err.to_string().contains("MISSING"), "{err}");
    }

    #[test]
    fn overrides_merge_and_last_key_null() {
        let overrides = json!({"env_vars": {"A": "1", "B": "2"}, "memory": "1Gi"});
        // set merges into the FULL map
        let (key, val) =
            merged_overrides_map(&overrides, None, &[("C".into(), "3".into())], &[]).unwrap();
        assert_eq!(key, "env_vars");
        assert_eq!(val, json!({"A": "1", "B": "2", "C": "3"}));
        // unsetting everything sends null so the key is dropped server-side
        let (_, val) = merged_overrides_map(
            &overrides,
            None,
            &[],
            &["A".to_string(), "B".to_string()],
        )
        .unwrap();
        assert_eq!(val, serde_json::Value::Null);
        // sidecar scope touches sidecar_env_vars keyed by container
        let ov2 = json!({"sidecar_env_vars": {"worker": {"X": "1"}}});
        let (key, val) =
            merged_overrides_map(&ov2, Some("worker"), &[("Y".into(), "2".into())], &[]).unwrap();
        assert_eq!(key, "sidecar_env_vars");
        assert_eq!(val, json!({"worker": {"X": "1", "Y": "2"}}));
    }

    #[test]
    fn effective_merge_env_wins() {
        let rows = effective_rows(
            &json!({"A": "app", "B": "app"}),
            &json!({"B": "env", "C": "env"}),
        );
        assert_eq!(
            rows,
            vec![
                ("A".to_string(), "app".to_string(), "app"),
                ("B".to_string(), "env".to_string(), "env"),
                ("C".to_string(), "env".to_string(), "env"),
            ]
        );
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib env_vars`
Expected: compile FAIL.

- [ ] **Step 3: Implement the helpers**

```rust
//! Env-var mutation helpers. The write rules are incident-shaped
//! (quome-fastapi #2608/#2609): app-level writes mutate ONLY `env_vars`
//! inside an otherwise-opaque spec Value; per-env writes produce exactly one
//! touched top-level config_overrides key (full sub-map, or Null to drop it).

use crate::errors::{QuomeError, Result};

const RESERVED_PREFIX: &str = "QUOME_";

pub fn validate_key(key: &str) -> std::result::Result<(), String> {
    let mut chars = key.chars();
    let head_ok = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_');
    let tail_ok = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !head_ok || !tail_ok {
        return Err(format!("'{}' is not a valid env var name ([A-Za-z_][A-Za-z0-9_]*)", key));
    }
    if key.starts_with(RESERVED_PREFIX) {
        return Err(format!("'{}' uses the platform-reserved {} prefix", key, RESERVED_PREFIX));
    }
    Ok(())
}

pub fn parse_pairs(raw: &[String]) -> Result<Vec<(String, String)>> {
    raw.iter()
        .map(|pair| {
            let (k, v) = pair
                .split_once('=')
                .ok_or_else(|| QuomeError::Usage(format!("'{}' is not KEY=VALUE", pair)))?;
            validate_key(k).map_err(QuomeError::Usage)?;
            Ok((k.to_string(), v.to_string()))
        })
        .collect()
}

/// Mutate ONLY the env-var map at the requested scope inside an opaque spec.
/// Returns the changed keys in argument order (set keys, then unset keys).
pub fn mutate_spec_env_vars(
    spec: &mut serde_json::Value,
    container: Option<&str>,
    set: &[(String, String)],
    unset: &[String],
) -> Result<Vec<String>> {
    let scope_desc = container
        .map(|c| format!("sidecar '{}'", c))
        .unwrap_or_else(|| "the app spec".to_string());

    let map_slot: &mut serde_json::Value = match container {
        None => &mut spec["env_vars"],
        Some(name) => {
            let sidecars = spec
                .get_mut("sidecars")
                .and_then(|s| s.as_array_mut())
                .ok_or_else(|| QuomeError::Usage("this app has no sidecars".into()))?;
            let known: Vec<String> = sidecars
                .iter()
                .filter_map(|s| s["name"].as_str().map(String::from))
                .collect();
            let sc = sidecars
                .iter_mut()
                .find(|s| s["name"].as_str() == Some(name))
                .ok_or_else(|| {
                    QuomeError::Usage(format!(
                        "no sidecar '{}' — this app has: {}",
                        name,
                        known.join(", ")
                    ))
                })?;
            &mut sc["env_vars"]
        }
    };
    if map_slot.is_null() {
        *map_slot = serde_json::json!({});
    }
    let map = map_slot
        .as_object_mut()
        .ok_or_else(|| QuomeError::Usage("env_vars is not an object in the spec".into()))?;

    let mut changed = Vec::new();
    for (k, v) in set {
        map.insert(k.clone(), serde_json::Value::String(v.clone()));
        changed.push(k.clone());
    }
    for k in unset {
        if map.remove(k).is_none() {
            return Err(QuomeError::Usage(format!(
                "'{}' is not set in {}",
                k, scope_desc
            )));
        }
        changed.push(k.clone());
    }
    Ok(changed)
}

/// Produce (touched_top_level_key, full_new_value) for the config_overrides
/// merge-patch. Value::Null means "drop the key entirely".
pub fn merged_overrides_map(
    overrides: &serde_json::Value,
    container: Option<&str>,
    set: &[(String, String)],
    unset: &[String],
) -> Result<(String, serde_json::Value)> {
    match container {
        None => {
            let mut map = overrides["env_vars"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            apply(&mut map, set, unset, "this environment's overrides")?;
            let value = if map.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::Object(map)
            };
            Ok(("env_vars".to_string(), value))
        }
        Some(name) => {
            let mut outer = overrides["sidecar_env_vars"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            let mut inner = outer
                .get(name)
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            apply(
                &mut inner,
                set,
                unset,
                &format!("sidecar '{}' overrides in this environment", name),
            )?;
            if inner.is_empty() {
                outer.remove(name);
            } else {
                outer.insert(name.to_string(), serde_json::Value::Object(inner));
            }
            let value = if outer.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::Object(outer)
            };
            Ok(("sidecar_env_vars".to_string(), value))
        }
    }
}

fn apply(
    map: &mut serde_json::Map<String, serde_json::Value>,
    set: &[(String, String)],
    unset: &[String],
    scope_desc: &str,
) -> Result<()> {
    for (k, v) in set {
        map.insert(k.clone(), serde_json::Value::String(v.clone()));
    }
    for k in unset {
        if map.remove(k).is_none() {
            return Err(QuomeError::Usage(format!("'{}' is not set in {}", k, scope_desc)));
        }
    }
    Ok(())
}

/// Effective (app ∪ env, env wins) rows for the list view, sorted by key.
pub fn effective_rows(
    app_env_vars: &serde_json::Value,
    override_env_vars: &serde_json::Value,
) -> Vec<(String, String, &'static str)> {
    let mut rows: std::collections::BTreeMap<String, (String, &'static str)> = Default::default();
    if let Some(map) = app_env_vars.as_object() {
        for (k, v) in map {
            rows.insert(k.clone(), (v.as_str().unwrap_or_default().to_string(), "app"));
        }
    }
    if let Some(map) = override_env_vars.as_object() {
        for (k, v) in map {
            rows.insert(k.clone(), (v.as_str().unwrap_or_default().to_string(), "env"));
        }
    }
    rows.into_iter().map(|(k, (v, s))| (k, v, s)).collect()
}
```

- [ ] **Step 4: Verify + gate + commit**

Run: `cargo test --lib env_vars && cargo fmt && cargo clippy -- -D warnings`
Expected: 7 tests pass.

```bash
git add src/commands/env_vars.rs src/commands/mod.rs
git commit -m "feat: env-var mutation helpers"
```

---

### Task 5: `env-vars` commands + docs

**Files:**
- Modify: `src/commands/env_vars.rs` (args + handlers)
- Modify: `src/commands/apps.rs` (`EnvVars` variant + dispatch)
- Modify: `src/ui.rs` (`EnvVarRow`)
- Create: `docs/reference/environments.md`; update `docs/reference/apps.md` + `docs/reference/README.md`

**Interfaces:**
- Consumes: Task 4 helpers; Task 1 resolver + `update_environment_overrides`; existing `list_bindings` (collision warning); raw app GET/PUT via `client.get::<serde_json::Value>` and `client.put`.

- [ ] **Step 1: Args + handlers**

`src/ui.rs`:

```rust
#[derive(Tabled)]
pub struct EnvVarRow {
    #[tabled(rename = "KEY")]
    pub key: String,
    #[tabled(rename = "VALUE")]
    pub value: String,
    #[tabled(rename = "SOURCE")]
    pub source: String,
}
```

`src/commands/env_vars.rs` additions:

```rust
#[derive(Subcommand)]
pub enum EnvVarsCommands {
    /// List env vars (effective view with --environment)
    List(EnvVarsListArgs),
    /// Set env vars (KEY=VALUE ...)
    Set(EnvVarsSetArgs),
    /// Remove env vars
    Unset(EnvVarsUnsetArgs),
}

#[derive(Parser)]
pub struct EnvVarsListArgs {
    #[arg(long)]
    app: Option<Uuid>,
    #[arg(long)]
    org: Option<Uuid>,
    /// Environment (name or UUID) — shows the effective merged set
    #[arg(long)]
    environment: Option<String>,
    /// Sidecar container name
    #[arg(long)]
    container: Option<String>,
    /// With --environment: show only the environment's own overrides
    #[arg(long)]
    overrides_only: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct EnvVarsSetArgs {
    /// KEY=VALUE pairs
    #[arg(required = true)]
    pairs: Vec<String>,
    #[arg(long)]
    environment: Option<String>,
    #[arg(long)]
    container: Option<String>,
    #[arg(long)]
    app: Option<Uuid>,
    #[arg(long)]
    org: Option<Uuid>,
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct EnvVarsUnsetArgs {
    /// Keys to remove
    #[arg(required = true)]
    keys: Vec<String>,
    #[arg(long)]
    environment: Option<String>,
    #[arg(long)]
    container: Option<String>,
    #[arg(long)]
    app: Option<Uuid>,
    #[arg(long)]
    org: Option<Uuid>,
    #[arg(long)]
    json: bool,
}
```

Handlers (shared plumbing mirrors envs.rs's `resolve_context`; reuse it via `crate::commands::envs::resolve_context` — make it `pub(crate)` there):

```rust
/// App-level env-var scope: the raw app row and the extracted maps.
async fn fetch_app_raw(
    client: &QuomeClient,
    org_id: Uuid,
    app_id: Uuid,
) -> Result<serde_json::Value> {
    client
        .get(&format!("/api/v1/orgs/{}/apps/{}", org_id, app_id))
        .await
}

fn app_scope_env_vars<'a>(
    app: &'a serde_json::Value,
    container: Option<&str>,
) -> Result<&'a serde_json::Value> {
    match container {
        None => Ok(&app["spec"]["env_vars"]),
        Some(name) => {
            let sidecars = app["spec"]["sidecars"].as_array();
            let found = sidecars
                .and_then(|arr| arr.iter().find(|s| s["name"].as_str() == Some(name)));
            found
                .map(|s| &s["env_vars"])
                .ok_or_else(|| QuomeError::Usage(format!("no sidecar '{}' on this app", name)))
        }
    }
}

pub async fn list(args: EnvVarsListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let (org_id, app_id) = crate::commands::envs::resolve_context(args.org, args.app, &config)?;
    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching env vars...");
    let app = fetch_app_raw(&client, org_id, app_id).await?;
    let app_vars = app_scope_env_vars(&app, args.container.as_deref())?.clone();

    let rows: Vec<(String, String, &'static str)> = match &args.environment {
        None => effective_rows(&app_vars, &serde_json::Value::Null),
        Some(env_ref) => {
            let env =
                crate::commands::envs::resolve_environment(&client, org_id, app_id, env_ref)
                    .await?;
            let ov = match args.container.as_deref() {
                None => env.config_overrides["env_vars"].clone(),
                Some(c) => env.config_overrides["sidecar_env_vars"][c].clone(),
            };
            if args.overrides_only {
                effective_rows(&serde_json::Value::Null, &ov)
            } else {
                effective_rows(&app_vars, &ov)
            }
        }
    };
    sp.finish_and_clear();

    if args.json {
        let arr: Vec<serde_json::Value> = rows
            .iter()
            .map(|(k, v, s)| serde_json::json!({"key": k, "value": v, "source": s}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("No env vars at this scope.");
        return Ok(());
    }
    ui::print_table(
        rows.into_iter()
            .map(|(key, value, source)| EnvVarRow {
                key,
                value,
                source: source.to_string(),
            })
            .collect::<Vec<_>>(),
    );
    println!(
        "Secret-shaped values belong in `quome apps bind` (secret-backed vars), not plaintext."
    );
    Ok(())
}

pub async fn set(args: EnvVarsSetArgs) -> Result<()> {
    let pairs = parse_pairs(&args.pairs)?;
    write_vars(
        args.org, args.app, args.environment, args.container, pairs, vec![], args.json,
    )
    .await
}

pub async fn unset(args: EnvVarsUnsetArgs) -> Result<()> {
    for k in &args.keys {
        validate_key(k).map_err(QuomeError::Usage)?;
    }
    write_vars(
        args.org, args.app, args.environment, args.container, vec![], args.keys, args.json,
    )
    .await
}

async fn write_vars(
    org: Option<Uuid>,
    app: Option<Uuid>,
    environment: Option<String>,
    container: Option<String>,
    set: Vec<(String, String)>,
    unset: Vec<String>,
    json: bool,
) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;
    let (org_id, app_id) = crate::commands::envs::resolve_context(org, app, &config)?;
    let client = QuomeClient::new(Some(&token), None)?;

    // Best-effort collision warning: a binding with the same env var name
    // shadows the plain var at deploy time.
    if !set.is_empty() {
        if let Ok(bindings) = client.list_bindings(org_id, app_id).await {
            for (k, _) in &set {
                if bindings.iter().any(|b| &b.env_var_name == k) {
                    eprintln!(
                        "warning: '{}' is also a resource binding — the binding may shadow \
                         this value at deploy",
                        k
                    );
                }
            }
        }
    }

    let changed: Vec<String>;
    let scope_name: String;
    match &environment {
        None => {
            let sp = ui::spinner("Updating app env vars...");
            let mut app_raw = fetch_app_raw(&client, org_id, app_id).await?;
            let mut spec = app_raw["spec"].take();
            changed = mutate_spec_env_vars(&mut spec, container.as_deref(), &set, &unset)?;
            let _: serde_json::Value = client
                .put(
                    &format!("/api/v1/orgs/{}/apps/{}", org_id, app_id),
                    &serde_json::json!({ "spec": spec }),
                )
                .await?;
            sp.finish_and_clear();
            scope_name = "app".to_string();
        }
        Some(env_ref) => {
            let env =
                crate::commands::envs::resolve_environment(&client, org_id, app_id, env_ref)
                    .await?;
            let sp = ui::spinner("Updating environment overrides...");
            let (key, value) =
                merged_overrides_map(&env.config_overrides, container.as_deref(), &set, &unset)?;
            changed = set
                .iter()
                .map(|(k, _)| k.clone())
                .chain(unset.iter().cloned())
                .collect();
            client
                .update_environment_overrides(
                    org_id,
                    app_id,
                    env.id,
                    &serde_json::json!({ key: value }),
                )
                .await?;
            sp.finish_and_clear();
            scope_name = format!("environment '{}'", env.name);
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "changed": changed, "scope": scope_name
            }))?
        );
    } else {
        println!(
            "Updated {} on {} — applies on the next deploy",
            changed.join(", "),
            scope_name
        );
    }
    Ok(())
}
```

Wire `AppsCommands::EnvVars { #[command(subcommand)] command: crate::commands::env_vars::EnvVarsCommands }` + dispatch, mirroring `Envs`.

- [ ] **Step 2: Docs**

- `docs/reference/environments.md` (new): the whole `envs` tree + `env-vars`, one example per verb, the write-semantics notes (opaque spec / merge-patch / applies-on-next-deploy), the gate behavior for promote, the destructive-delete warning.
- `docs/reference/apps.md`: bindings `--environment` now accepts names; point env-var questions at environments.md.
- `docs/reference/README.md`: index the new page.

- [ ] **Step 3: Gate + commit**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`

```bash
git add src/commands/env_vars.rs src/commands/apps.rs src/ui.rs docs/reference
git commit -m "feat(apps): env-vars list, set, unset"
```

---

### Task 6: Verification + PR

- [ ] **Step 1: Full gate**

`cargo fmt --check && cargo clippy -- -D warnings && cargo test` — all green.

- [ ] **Step 2: Live verification — USER APPROVAL REQUIRED FIRST**

Per the standing rule, present this command list to the user and run only after explicit approval (against a user-designated test app; names below are placeholders they may change):

```bash
quome apps envs --app <TEST_APP>
quome apps envs create scratch --branch scratch --app <TEST_APP>
quome apps env-vars set CLI_TEST_VAR=hello --app <TEST_APP>
quome apps env-vars set CLI_TEST_VAR=scoped --environment scratch --app <TEST_APP>
quome apps env-vars --environment scratch --app <TEST_APP>   # expect SOURCE env for CLI_TEST_VAR
quome apps env-vars unset CLI_TEST_VAR --environment scratch --app <TEST_APP>
quome apps env-vars unset CLI_TEST_VAR --app <TEST_APP>
quome apps envs config set memory=1Gi --environment scratch --app <TEST_APP>
quome apps envs config show --environment scratch --app <TEST_APP>
quome apps envs config unset memory --environment scratch --app <TEST_APP>
quome apps envs promote scratch --from <DEFAULT_ENV> --app <TEST_APP>   # only if source has a deploy
quome apps envs delete scratch --app <TEST_APP>
```

Bindings `--environment <name>` retrofit is covered by the env-scoped set/unset above plus one `bind --environment scratch` if the user approves it.

- [ ] **Step 3: Push + PR (stacked)**

```bash
git push -u origin feat/envs-and-env-vars
gh pr create --repo quome-cloud/quome-cli --base feat/bindings-and-static-deploy \
  --title "feat: environments + env-var management" --body "<command surface, write-safety notes (opaque spec, merge-patch), verification transcript; note: retarget to main after #6 merges>"
```
After quome-cli#6 merges: `gh pr edit <n> --base main` and rebase.
