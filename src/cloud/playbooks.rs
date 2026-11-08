//! Playbooks: task recipes the Quome Cloud API renders for an agent.
//! Types for the list and render routes, and the tool names this binary serves.
use super::clients::{ClientId, SkillFile};
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

/// Lowercase kebab form of a playbook name, valid as a skill name and folder.
fn skill_slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Collapse every run of whitespace and control characters to one space.
fn one_line(s: &str) -> String {
    s.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn yaml_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

const DESCRIPTION_MAX: usize = 500;

/// Render the thin SKILL.md for one playbook. Playbook text comes from the
/// server: it is flattened to one line and quoted, never trusted as YAML.
pub fn render_skill(pb: &Playbook, client: ClientId) -> SkillFile {
    let slug = skill_slug(&pb.name);
    let dir_name = match client {
        ClientId::Claude => slug.clone(),
        _ => format!("quome-{slug}"),
    };
    let title = one_line(&pb.title);
    let title = title.trim_end_matches(['.', ' ']);
    let desc = one_line(&pb.description);
    let mut lower = title.to_string();
    if let Some(first) = lower.chars().next() {
        lower = first.to_lowercase().collect::<String>() + &lower[first.len_utf8()..];
    }
    let mut description = format!("{title} \u{2014} {desc} Use when the person asks to {lower}.");
    if description.chars().count() > DESCRIPTION_MAX {
        description = description
            .chars()
            .take(DESCRIPTION_MAX - 1)
            .collect::<String>()
            + "\u{2026}";
    }
    let call_name: String = pb
        .name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    // Codex, OpenCode and Antigravity key the skill by folder name; Claude's
    // plugin namespace supplies the prefix, so its name is the bare slug.
    let contents = format!(
        "---\nname: {}\ndescription: {}\n---\n\nCall the `quome` MCP tool `playbook` with name `{}` and the person's request as `what` (and `resource` if they named one).\nFollow the text it returns.\n",
        dir_name,
        yaml_quote(&description),
        call_name
    );
    SkillFile { dir_name, contents }
}

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

    fn pb() -> Playbook {
        Playbook {
            name: "run-code".into(),
            area: "sandboxes".into(),
            title: "Run code in a sandbox".into(),
            description: "Start a sandbox and run a command.".into(),
            arguments: vec![],
            requires_tools: vec![],
            allowed: true,
        }
    }

    #[test]
    fn skill_frontmatter_and_body() {
        let s = render_skill(&pb(), ClientId::Codex);
        assert_eq!(s.dir_name, "quome-run-code");
        let lines: Vec<&str> = s.contents.lines().collect();
        assert_eq!(lines[0], "---");
        assert_eq!(lines[1], "name: quome-run-code");
        assert!(
            lines[2].starts_with("description: \"Run code in a sandbox \u{2014} Start a sandbox")
        );
        assert!(lines[2].contains("Use when the person asks to run code in a sandbox."));
        assert_eq!(lines[3], "---");
        assert_eq!(lines.len(), 7);
        assert_eq!(
            lines[5],
            "Call the `quome` MCP tool `playbook` with name `run-code` and the person's request as `what` (and `resource` if they named one)."
        );
        assert_eq!(lines[6], "Follow the text it returns.");
    }

    #[test]
    fn skill_has_no_urls_or_ids() {
        for c in ClientId::ALL {
            let s = render_skill(&pb(), c).contents;
            assert!(!s.contains("http"));
            assert!(!s.contains("org_") && !s.contains("org-id"));
        }
    }

    #[test]
    fn claude_skill_lives_in_plugin_dir() {
        assert_eq!(render_skill(&pb(), ClientId::Claude).dir_name, "run-code");
        let s = render_skill(&pb(), ClientId::Claude).contents;
        assert!(s.contains("name: run-code\n"));
    }

    #[test]
    fn hostile_text_stays_one_quoted_line() {
        let mut p = pb();
        p.title = "Evil\n---\nname: x".into();
        p.description = "say \"hi\" \\ now\r\nmore".into();
        p.name = "Bad Name!\n`x`".into();
        let s = render_skill(&p, ClientId::Antigravity);
        assert_eq!(s.dir_name, "quome-bad-name-x");
        assert_eq!(s.contents.lines().count(), 7);
        assert!(s.contents.contains("\\\"hi\\\""));
    }
}
