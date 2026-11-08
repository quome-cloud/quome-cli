//! Playbooks: task recipes the Quome Cloud API renders for an agent.
//! Types for the list and render routes, and the tool names this binary serves.
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlaybookArg {
    pub name: String,
    pub description: String,
    pub required: bool,
}

/// One playbook in the list answer. Unknown extra fields are ignored.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct Playbook {
    pub name: String,
    pub area: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub arguments: Vec<PlaybookArg>,
    #[serde(default)]
    pub requires_tools: Vec<String>,
    /// Reserved: the API always answers true today. No logic depends on it.
    #[serde(default = "yes")]
    pub allowed: bool,
}

fn yes() -> bool {
    true
}

/// The MCP tool names this binary serves, sent so the API lists only the
/// playbooks whose required tools are all present.
pub const TOOL_NAMES: &[&str] = &[
    "guide",
    "create_sandbox",
    "list_sandboxes",
    "stop_sandbox",
    "exec",
    "write_file",
    "read_file",
    "list_files",
    "preview",
    "deploy_app",
    "app_status",
    "app_logs",
    "delete_app",
    "sync_folder",
    "list_app_catalog",
    "install_app",
    "list_installed_apps",
    "open_app",
    "find_app_tools",
    "describe_app_tool",
    "call_app_tool",
    "list_app_keys",
    "set_app_key",
    "uninstall_app",
    "delete_app_data",
    "email_create_address",
    "email_list_addresses",
    "email_disable",
    "email_list_messages",
    "email_read",
    "email_wait",
    "auth_status",
    "auth_apps",
    "auth_users",
    "auth_roles",
    "auth_agents",
    "auth_config",
    "auth_promote",
    "auth_test_token",
    "auth_outbox",
    "git_repos",
    "git_read",
    "git_diff",
    "git_commit",
    "git_pr",
    "git_clone_url",
    "cache_remember",
    "cache_recall",
    "cache_forget",
    "cache_get",
    "cache_set",
    "cache_del",
    "cache_pod",
    "playbook",
];

#[cfg(test)]
mod tests {
    use super::super::api::CloudApi;
    use super::super::auth::tests::profile;
    use super::super::auth::TokenSource;
    use super::super::keys::AgentKey;
    use super::*;
    use crate::errors::QuomeError;
    use std::collections::BTreeMap;
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ORG: &str = "11111111-1111-1111-1111-111111111111";

    async fn setup() -> (MockServer, CloudApi) {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"token_endpoint": format!("{}/oauth/token", s.uri())}),
            ))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "AT", "token_type": "Bearer", "expires_in": 3600
            })))
            .mount(&s)
            .await;
        let p = profile(&s.uri());
        let api = CloudApi::new(&p, TokenSource::new(&p, AgentKey::generate(), "vrc".into()));
        (s, api)
    }

    #[tokio::test]
    async fn list_sends_own_tool_names() {
        let (s, api) = setup().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/orgs/{ORG}/agent-playbooks")))
            .and(query_param("tools", TOOL_NAMES.join(",")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"name":"ship","area":"apps","title":"Ship","description":"d",
                 "arguments":[{"name":"app","description":"which","required":true}],
                 "requires_tools":["deploy_app"],"requires_capabilities":["apps_operate"],
                 "allowed":true,"future_field":1}
            ])))
            .expect(1)
            .mount(&s)
            .await;
        let list = api.agent_playbooks(TOOL_NAMES).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "ship");
        assert!(list[0].arguments[0].required);
        assert_eq!(list[0].requires_tools, vec!["deploy_app"]);
    }

    #[tokio::test]
    async fn render_returns_text() {
        let (s, api) = setup().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/v1/orgs/{ORG}/agent-playbooks/ship/render"
            )))
            .and(body_json(serde_json::json!({"arguments":{"app":"web"}})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"name":"ship","text":"# Ship web"})),
            )
            .mount(&s)
            .await;
        let args = BTreeMap::from([("app".to_string(), "web".to_string())]);
        assert_eq!(
            api.render_playbook("ship", &args).await.unwrap(),
            "# Ship web"
        );
    }

    #[tokio::test]
    async fn render_404_is_playbook_unknown_error() {
        let (s, api) = setup().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/v1/orgs/{ORG}/agent-playbooks/nope/render"
            )))
            .respond_with(ResponseTemplate::new(404).set_body_json(
                serde_json::json!({"detail":"no such playbook","reason":"playbook_unknown"}),
            ))
            .mount(&s)
            .await;
        let e = api
            .render_playbook("nope", &BTreeMap::new())
            .await
            .unwrap_err();
        assert!(matches!(e, QuomeError::NotFound(_)), "{e:?}");
        assert!(e.to_string().contains("playbook_unknown"), "{e}");
    }

    #[tokio::test]
    async fn render_422_and_403_are_api_errors() {
        let (s, api) = setup().await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/agent-playbooks/a/render")))
            .respond_with(
                ResponseTemplate::new(422)
                    .set_body_json(serde_json::json!({"detail":[{"loc":["body"],"msg":"bad"}]})),
            )
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/orgs/{ORG}/agent-playbooks/b/render")))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(serde_json::json!({"detail":"x","reason":"capability_missing"})),
            )
            .mount(&s)
            .await;
        let e = api
            .render_playbook("a", &BTreeMap::new())
            .await
            .unwrap_err();
        assert!(e.to_string().contains("422"), "{e}");
        let e = api
            .render_playbook("b", &BTreeMap::new())
            .await
            .unwrap_err();
        assert!(e.to_string().contains("capability_missing"), "{e}");
    }
}
