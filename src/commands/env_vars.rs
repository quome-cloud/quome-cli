//! Env-var mutation helpers. The write rules are incident-shaped
//! (quome-fastapi #2608/#2609): app-level writes mutate ONLY `env_vars`
//! inside an otherwise-opaque spec Value; per-env writes produce exactly one
//! touched top-level config_overrides key (full sub-map, or Null to drop it).

use crate::errors::{QuomeError, Result};

const RESERVED_PREFIX: &str = "QUOME_";

#[allow(dead_code)]
pub fn validate_key(key: &str) -> std::result::Result<(), String> {
    let mut chars = key.chars();
    let head_ok = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_');
    let tail_ok = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !head_ok || !tail_ok {
        return Err(format!(
            "'{}' is not a valid env var name ([A-Za-z_][A-Za-z0-9_]*)",
            key
        ));
    }
    if key.starts_with(RESERVED_PREFIX) {
        return Err(format!(
            "'{}' uses the platform-reserved {} prefix",
            key, RESERVED_PREFIX
        ));
    }
    Ok(())
}

#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub fn effective_rows(
    app_env_vars: &serde_json::Value,
    override_env_vars: &serde_json::Value,
) -> Vec<(String, String, &'static str)> {
    let mut rows: std::collections::BTreeMap<String, (String, &'static str)> = Default::default();
    if let Some(map) = app_env_vars.as_object() {
        for (k, v) in map {
            rows.insert(
                k.clone(),
                (v.as_str().unwrap_or_default().to_string(), "app"),
            );
        }
    }
    if let Some(map) = override_env_vars.as_object() {
        for (k, v) in map {
            rows.insert(
                k.clone(),
                (v.as_str().unwrap_or_default().to_string(), "env"),
            );
        }
    }
    rows.into_iter().map(|(k, (v, s))| (k, v, s)).collect()
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
}
