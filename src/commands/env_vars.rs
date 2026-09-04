//! Env-var mutation helpers. The write rules are incident-shaped
//! (quome-fastapi #2608/#2609): app-level writes mutate ONLY `env_vars`
//! inside an otherwise-opaque spec Value; per-env writes produce exactly one
//! touched top-level config_overrides key (full sub-map, or Null to drop it).

use clap::{Parser, Subcommand};
use uuid::Uuid;

use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::{QuomeError, Result};
use crate::ui::{self, EnvVarRow};

const RESERVED_PREFIX: &str = "QUOME_";

/// Grammar-only check ([A-Za-z_][A-Za-z0-9_]*), no reserved-prefix rejection.
/// `unset` uses this alone — a grandfathered `QUOME_`-prefixed key that
/// somehow got set must still be removable.
fn validate_key_grammar(key: &str) -> std::result::Result<(), String> {
    let mut chars = key.chars();
    let head_ok = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_');
    let tail_ok = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !head_ok || !tail_ok {
        return Err(format!(
            "'{}' is not a valid env var name ([A-Za-z_][A-Za-z0-9_]*)",
            key
        ));
    }
    Ok(())
}

pub fn validate_key(key: &str) -> std::result::Result<(), String> {
    validate_key_grammar(key)?;
    if key.starts_with(RESERVED_PREFIX) {
        return Err(format!(
            "'{}' uses the platform-reserved {} prefix",
            key, RESERVED_PREFIX
        ));
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

/// Sidecar container names declared on an app spec, in spec order. Empty if
/// the spec has no `sidecars` array at all.
pub fn sidecar_names(spec: &serde_json::Value) -> Vec<String> {
    spec.get("sidecars")
        .and_then(|s| s.as_array())
        .into_iter()
        .flatten()
        .filter_map(|s| s["name"].as_str().map(String::from))
        .collect()
}

/// Validate a sidecar name against a spec, erroring with the same message
/// shape whether the caller is about to mutate the spec in place
/// (`mutate_spec_env_vars`) or just needs a pre-flight check before a
/// per-env `config_overrides` write (`write_vars`).
fn check_sidecar_exists(spec: &serde_json::Value, name: &str) -> Result<()> {
    if spec.get("sidecars").and_then(|s| s.as_array()).is_none() {
        return Err(QuomeError::Usage("this app has no sidecars".into()));
    }
    let known = sidecar_names(spec);
    if known.iter().any(|n| n == name) {
        Ok(())
    } else {
        Err(QuomeError::Usage(format!(
            "no sidecar '{}' — this app has: {}",
            name,
            known.join(", ")
        )))
    }
}

/// Mutate ONLY the env-var map at the requested scope inside an opaque spec.
/// Returns the changed keys in argument order (set keys, then unset keys).
pub fn mutate_spec_env_vars(
    spec: &mut serde_json::Value,
    container: Option<&str>,
    set: &[(String, String)],
    unset: &[String],
) -> Result<Vec<String>> {
    // Guard against non-object specs that would panic on IndexMut.
    if !spec.is_object() {
        return Err(QuomeError::Usage(
            "the app's spec is not an object — refusing to modify it".into(),
        ));
    }

    let scope_desc = container
        .map(|c| format!("sidecar '{}'", c))
        .unwrap_or_else(|| "the app spec".to_string());

    let map_slot: &mut serde_json::Value = match container {
        None => &mut spec["env_vars"],
        Some(name) => {
            check_sidecar_exists(spec, name)?;
            let sidecars = spec
                .get_mut("sidecars")
                .and_then(|s| s.as_array_mut())
                .expect("check_sidecar_exists verified this array exists");
            let sc = sidecars
                .iter_mut()
                .find(|s| s["name"].as_str() == Some(name))
                .expect("check_sidecar_exists verified this name is present");
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
            return Err(QuomeError::Usage(format!(
                "'{}' is not set in {}",
                k, scope_desc
            )));
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
            let rendered = v
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| v.to_string());
            rows.insert(k.clone(), (rendered, "app"));
        }
    }
    if let Some(map) = override_env_vars.as_object() {
        for (k, v) in map {
            let rendered = v
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| v.to_string());
            rows.insert(k.clone(), (rendered, "env"));
        }
    }
    rows.into_iter().map(|(k, (v, s))| (k, v, s)).collect()
}

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
    /// Application ID (uses linked app if not provided)
    #[arg(long)]
    app: Option<Uuid>,
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,
    /// Environment (name or UUID) — shows the effective merged set
    #[arg(long)]
    environment: Option<String>,
    /// Sidecar container name
    #[arg(long)]
    container: Option<String>,
    /// With --environment: show only the environment's own overrides
    #[arg(long, requires = "environment")]
    overrides_only: bool,
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct EnvVarsSetArgs {
    /// KEY=VALUE pairs
    #[arg(required = true)]
    pairs: Vec<String>,
    /// Environment (name or UUID) — write to this environment's overrides
    /// instead of the app spec
    #[arg(long)]
    environment: Option<String>,
    /// Sidecar container name
    #[arg(long)]
    container: Option<String>,
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
pub struct EnvVarsUnsetArgs {
    /// Keys to remove
    #[arg(required = true)]
    keys: Vec<String>,
    /// Environment (name or UUID) — remove from this environment's overrides
    /// instead of the app spec
    #[arg(long)]
    environment: Option<String>,
    /// Sidecar container name
    #[arg(long)]
    container: Option<String>,
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

pub async fn execute(command: EnvVarsCommands) -> Result<()> {
    match command {
        EnvVarsCommands::List(args) => list(args).await,
        EnvVarsCommands::Set(args) => set(args).await,
        EnvVarsCommands::Unset(args) => unset(args).await,
    }
}

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
            let found =
                sidecars.and_then(|arr| arr.iter().find(|s| s["name"].as_str() == Some(name)));
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
            let env = crate::commands::envs::resolve_environment(&client, org_id, app_id, env_ref)
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
        args.org,
        args.app,
        args.environment,
        args.container,
        pairs,
        vec![],
        args.json,
    )
    .await
}

pub async fn unset(args: EnvVarsUnsetArgs) -> Result<()> {
    // Grammar only, not the reserved-prefix rejection: a grandfathered
    // QUOME_-prefixed key that somehow got set must still be removable.
    for k in &args.keys {
        validate_key_grammar(k).map_err(QuomeError::Usage)?;
    }
    write_vars(
        args.org,
        args.app,
        args.environment,
        args.container,
        vec![],
        args.keys,
        args.json,
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

    let (changed, scope_name): (Vec<String>, String) = match &environment {
        None => {
            let sp = ui::spinner("Updating app env vars...");
            let mut app_raw = fetch_app_raw(&client, org_id, app_id).await?;
            let mut spec = app_raw["spec"].take();
            let changed = mutate_spec_env_vars(&mut spec, container.as_deref(), &set, &unset)?;
            let _: serde_json::Value = client
                .put(
                    &format!("/api/v1/orgs/{}/apps/{}", org_id, app_id),
                    &serde_json::json!({ "spec": spec }),
                )
                .await?;
            sp.finish_and_clear();
            (changed, "app".to_string())
        }
        Some(env_ref) => {
            let env = crate::commands::envs::resolve_environment(&client, org_id, app_id, env_ref)
                .await?;
            // The backend merges config_overrides.sidecar_env_vars by name and
            // silently ignores any name it doesn't recognize, so an unknown
            // --container here would otherwise report success while writing a
            // dead override. Validate against the app's actual sidecars first.
            if let Some(name) = container.as_deref() {
                let app_raw = fetch_app_raw(&client, org_id, app_id).await?;
                check_sidecar_exists(&app_raw["spec"], name)?;
            }
            let sp = ui::spinner("Updating environment overrides...");
            let (key, value) =
                merged_overrides_map(&env.config_overrides, container.as_deref(), &set, &unset)?;
            let changed = set
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
            (changed, format!("environment '{}'", env.name))
        }
    };

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
    fn grammar_only_check_allows_reserved_prefix() {
        // unset uses the grammar-only check so a grandfathered QUOME_-prefixed
        // key can still be removed, unlike set's full validate_key.
        assert!(validate_key_grammar("QUOME_LEGACY").is_ok());
        assert!(validate_key("QUOME_LEGACY").is_err());
        assert!(validate_key_grammar("1BAD").is_err());
        assert!(validate_key_grammar("BAD-DASH").is_err());
    }

    #[test]
    fn sidecar_names_lists_known_containers() {
        assert_eq!(
            sidecar_names(&spec_fixture()),
            vec!["worker".to_string(), "cache-warm".to_string()]
        );
        assert_eq!(
            sidecar_names(&json!({"env_vars": {}})),
            Vec::<String>::new()
        );
    }

    #[test]
    fn check_sidecar_exists_errors_on_unknown_name() {
        let spec = spec_fixture();
        assert!(check_sidecar_exists(&spec, "worker").is_ok());
        let err = check_sidecar_exists(&spec, "nope").unwrap_err();
        assert!(err.to_string().contains("worker"), "{err}");
        assert!(err.to_string().contains("cache-warm"), "{err}");

        let no_sidecars = json!({"env_vars": {}});
        let err = check_sidecar_exists(&no_sidecars, "anything").unwrap_err();
        assert!(err.to_string().contains("no sidecars"), "{err}");
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
        assert_eq!(
            spec["sidecars"][0]["env_vars"],
            json!({"W": "1", "W2": "2"})
        );
        assert_eq!(
            spec["sidecars"][0]["unknown_future_field"],
            json!({"nested": true})
        );
        // Sidecar without env_vars gets one created.
        let mut spec2 = spec_fixture();
        mutate_spec_env_vars(
            &mut spec2,
            Some("cache-warm"),
            &[("A".into(), "1".into())],
            &[],
        )
        .unwrap();
        assert_eq!(spec2["sidecars"][1]["env_vars"], json!({"A": "1"}));
        // Unknown container errors, naming the candidates.
        let err = mutate_spec_env_vars(&mut spec, Some("nope"), &[("A".into(), "1".into())], &[])
            .unwrap_err();
        assert!(err.to_string().contains("worker"), "{err}");
    }

    #[test]
    fn unset_of_absent_key_errors_with_scope() {
        let mut spec = spec_fixture();
        let err = mutate_spec_env_vars(&mut spec, None, &[], &["MISSING".to_string()]).unwrap_err();
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
        let (_, val) =
            merged_overrides_map(&overrides, None, &[], &["A".to_string(), "B".to_string()])
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

    #[test]
    fn non_object_spec_errors_instead_of_panicking() {
        for mut bad in [
            serde_json::json!("str"),
            serde_json::json!(3),
            serde_json::json!([1]),
            serde_json::Value::Null,
        ] {
            let err = mutate_spec_env_vars(&mut bad, None, &[("A".into(), "1".into())], &[]);
            assert!(err.is_err(), "expected error for {bad}");
        }
    }

    #[test]
    fn effective_rows_renders_non_string_values_visibly() {
        let rows = effective_rows(&json!({"N": 5, "B": true}), &serde_json::Value::Null);
        assert_eq!(rows[0], ("B".to_string(), "true".to_string(), "app"));
        assert_eq!(rows[1], ("N".to_string(), "5".to_string(), "app"));
    }
}
