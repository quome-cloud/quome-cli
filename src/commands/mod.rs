pub mod apps;
pub mod bindings;
pub mod databases;
pub mod deploy;
pub mod deployments;
pub mod envs;
pub mod events;
pub mod host;
pub mod keys;
pub mod link;
pub mod login;
pub mod logout;
pub mod logs;
pub mod members;
pub mod orgs;
pub mod secrets;
pub mod unlink;
pub mod upgrade;
pub mod whoami;

/// Commands that administer the organization itself (its members, other
/// keys, its list of orgs, the audit trail) are gated on an organization-level
/// permission that an API key can never hold: keys authenticate as an
/// org-scoped service account and are deliberately unable to escalate to org
/// administration. Fail before the request so the user gets the reason and
/// the place to go, not a 401/403.
pub fn dashboard_only(what: &str) -> crate::errors::QuomeError {
    let dashboard = crate::settings::Settings::load()
        .unwrap_or_default()
        .get_api_url();
    crate::errors::QuomeError::Usage(format!(
        "{what} is organization administration, which an API key cannot do \
         (keys act as an org-scoped service account and never as an org admin). \
         Use the dashboard: {dashboard}/settings"
    ))
}

/// `QUOME_ALLOW_ADMIN_COMMANDS=1` keeps the org-admin commands callable for
/// anyone running the CLI against a control plane that still resolves keys
/// to a user (self-hosted / pre-2026-08 builds). Default off.
pub fn dashboard_only_override() -> bool {
    std::env::var("QUOME_ALLOW_ADMIN_COMMANDS").is_ok_and(|v| v == "1")
}
