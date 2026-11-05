# Auth tools for agents (part 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give agents in the `quome` MCP server a small set of `auth_*` tools to build sign-in against an org's sandbox auth, test it without a browser, and promote it to prod.

**Architecture:** Each tool asks the platform API for a short-lived token for the org's auth instance (`POST /orgs/{org}/auth/token?env=`), then calls that instance's standard MCP endpoint (`mcp_url` from the token response) with one JSON-RPC `tools/call`. Platform-side concerns (which apps are linked, External clients, promote) go through the platform's `/auth/*` routes instead. Tokens are cached per environment and scope set until 30 s before expiry, like `TokenSource`.

**Tech Stack:** Rust 2021, `rmcp` 2.2 (server), `reqwest` 0.12, `serde_json`, `schemars`; tests with `wiremock`.

**Spec:** the platform's per-org auth design (kept in the platform repo; not reproduced here). This public repo never names that engine; see Global Constraints.

## Global Constraints

- **Public repo.** Code, comments, docs and commit messages never name internal services or repositories. The identity engine is "the Quome Cloud identity service"; its per-org instance is "the org's auth instance". Example hosts in tests use `apps.example` / `MockServer::uri()`.
- Tool names are exactly: `auth_status`, `auth_apps`, `auth_config`, `auth_roles`, `auth_users`, `auth_agents`, `auth_promote`, `auth_test_token`, `auth_outbox`. No others.
- `env` is `"sandbox"` or `"prod"`; it defaults to `"sandbox"` everywhere.
- Scopes requested from the broker, by action: reads `["auth_read"]`; sandbox writes `["auth_read","auth_sandbox_write"]`; prod writes `["auth_read","auth_prod_write"]`. Never request more than the action needs.
- The instance's MCP is stateless JSON-RPC 2.0 over HTTP POST: no `initialize`, no session id. Request `{"jsonrpc":"2.0","id":<n>,"method":"tools/call","params":{"name":<tool>,"arguments":<obj>}}`, headers `Authorization: Bearer <token>`, `MCP-Protocol-Version: 2025-06-18`, `Content-Type: application/json`. Result `{"result":{"content":[{"type":"text","text":…}],"structuredContent":<obj>?,"isError":bool}}` or `{"error":{"code","message"}}`.
- Secrets (client secrets, test tokens) are returned to the agent once, in the tool result, and never logged.
- CI gates: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo build --release`. Conventional commits (`feat(mcp): …`).
- `lists_exactly_the_brief_tools` in `src/cloud/mcp.rs` must list every tool; `docs/reference/mcp.md` gets one row per tool; `README.md:96` names the family.

## Review Focus

1. **The org's prod auth doesn't exist yet** (nothing linked, never promoted): a read-only tool on `prod` must say so plainly (`auth_not_started`), not fail with a raw 404. Test in Task 2.
2. **A stale delegation without the auth scopes**: the broker answers 403 `capability_missing`; the tool must tell the agent to run `quome connect` again and allow auth access. Test in Task 1.
3. **A sandbox-only tool called on prod** (`auth_test_token`, `auth_outbox`): the instance refuses with `sandbox_only`; the tool must pass that reason through readably. Test in Task 5.
4. **Token expiry mid-session**: the cache must refresh after `expires_in - 30 s`, and a 401 from the instance must drop the cached token and retry once. Test in Task 1.
5. **An instance MCP error with structured data** (a partial config apply returns minted secrets): the tool must surface `structuredContent` so the secrets aren't lost. Test in Task 4.

---

### Task 1: Broker token and instance MCP client

**Files:**
- Create: `src/cloud/auth_instance.rs`
- Modify: `src/cloud/api.rs` (add `auth_token`), `src/cloud/mod.rs` (module)
- Test: inline `#[cfg(test)]` in both files

**Interfaces:**
- Produces:
  - `CloudApi::auth_token(&self, env: &str, scope: &[&str]) -> Result<AuthToken, QuomeError>`, `POST /orgs/{org}/auth/token?env=<env>` with body `{"scope":[…]}`. `AuthToken { access_token: String, expires_in: u64, issuer: String, mcp_url: String }`.
  - `AuthInstances::new(api: Arc<…>, timing: Timing)` held on `QuomeMcp` as `auth: Arc<AuthInstances>`.
  - `AuthInstances::call(&self, env: &str, scope: Scope, tool: &str, args: serde_json::Value) -> Result<serde_json::Value, String>` where `enum Scope { Read, Write }` picks the scope set from the Global Constraints by `env`. Returns `structuredContent` when present, else the parsed JSON of `content[0].text`, else `{"text": …}`.
  - `AuthInstances::issuer(&self, env) -> Result<String, String>` (from the cached token response).
  - `fn auth_hint(e: &str) -> String` maps: `403`+`capability_missing`/`role_required` → "Run `quome connect` again and allow auth access (prod changes need an org owner or admin)."; `404`+`auth_not_started` → "This org's prod auth hasn't started: link an app or promote first."; `503`+`auth_starting` → "The auth instance is waking up; try again in a few seconds."; `sandbox_only` → "This tool works only on the sandbox environment."; otherwise the original text.
- Token cache: `HashMap<(String /*env*/, Scope), (AuthToken, Instant)>` under a `tokio::sync::Mutex`, refreshed when `Instant::now() >= fetched + expires_in - 30 s`. A 401 from the instance evicts the entry and retries the call once.

- [ ] **Step 1: Write failing tests** (wiremock):
  - `auth_token_posts_scope_and_env`: the mock expects `POST /api/v1/orgs/{ORG}/auth/token?env=prod` with body `{"scope":["auth_read","auth_prod_write"]}` and returns `{"access_token":"T1","token_type":"Bearer","expires_in":300,"issuer":"<mock>","mcp_url":"<mock>/mcp"}`; assert the parsed struct.
  - `call_sends_jsonrpc_with_bearer_and_version`: the `/mcp` mock asserts headers and the `tools/call` body for `status`, returns a result with `structuredContent: {"mode":"sandbox"}`; assert the returned value.
  - `call_prefers_structured_then_text_json`: a result with only `content[0].text = "{\"a\":1}"` returns `{"a":1}`.
  - `call_maps_tool_error_through_hint`: `isError: true` with text `"sandbox_only: test tokens are sandbox-only"` returns `Err` containing "only on the sandbox".
  - `token_cached_until_skew_then_refreshed`: with `expires_in: 31`, two calls use one token fetch; after advancing (inject a clock via `Timing` or `tokio::time::pause`) past 1 s, the next call fetches again.
  - `instance_401_evicts_and_retries_once`: first `/mcp` answers 401, the second 200; exactly two token fetches and two `/mcp` calls.
  - `broker_403_capability_missing_hint`: the token route answers 403 `{"reason":"capability_missing"}`; `call` returns `Err` containing "quome connect".
- [ ] **Step 2:** `cargo test auth_instance api::tests::auth_token` → FAIL (not defined).
- [ ] **Step 3:** Implement `auth_token` (reuse `json`/`org_path`/`send`), `auth_instance.rs` (a reqwest client built like `CloudApi::with_timeout`, `REQUEST_TIMEOUT` 30 s), the cache, and `auth_hint`. Wire `auth: Arc<AuthInstances>` into `QuomeMcp::new`/`with_timing`.
- [ ] **Step 4:** tests PASS; `cargo clippy --all-targets -- -D warnings`; `cargo fmt`.
- [ ] **Step 5:** Commit `feat(mcp): broker token and client for the org's auth instance`.

### Task 2: `auth_status` and `auth_apps`

**Files:**
- Modify: `src/cloud/api.rs` (add `auth_apps`, `auth_link_app`, `auth_unlink_app`, `auth_register_external`), `src/cloud/mcp.rs`
- Test: `src/cloud/mcp.rs` tests

**Interfaces:**
- Consumes: Task 1.
- Platform routes: `GET /orgs/{org}/auth/apps?env=` → `{"items":[{client_id, env, app_id|null, label:"on_quome"|"external", redirect_uris}]}`; `POST /orgs/{org}/auth/apps/{service_id}/link` body `{client_id?, redirect_path?, allowed_roles?}`; `DELETE …/link`; `POST /orgs/{org}/auth/external-clients` body `{client_id, kind:"public"|"confidential", redirect_uris}` → includes `client_secret` once.
- `auth_status` args `{}` → for each env: `{"env", "started": bool, "issuer"?, "mode"?, "clients", "users", "roles", "apps": [...]}`. `started: false` when the broker answers `auth_not_started`; sandbox is ensured (the broker starts it).
- `auth_apps` args `{action: "list"|"link"|"unlink"|"register_external"|"set_access", env?, app?: String (app name), client_id?, redirect_path?, allowed_roles?: Vec<String>, kind?, redirect_uris?: Vec<String>}`:
  - `list` → platform `GET /auth/apps`.
  - `link`/`unlink` (prod only; resolve `app` name → service id via `list_apps`) → platform routes.
  - `register_external` → platform route; the result includes the one-time secret with a note "Store this secret now; it is not shown again."
  - `set_access` → instance: `clients_list`, find `client_id`, `client_upsert` resending every field of the current client with `allowed_roles` replaced (update replaces all fields). Write scope for `env`.

- [ ] **Step 1: Failing tests:** `auth_status_reports_both_envs` (prod token route → 404 `auth_not_started`; sandbox → status result) asserts `started` per env and the apps list; `auth_apps_link_resolves_app_name` asserts `POST /auth/apps/<id>/link` with the body; `auth_apps_register_external_returns_secret_once`; `auth_apps_set_access_resends_client_fields` asserts `client_upsert` arguments equal the listed client's fields except `allowed_roles`.
- [ ] **Step 2:** run → FAIL. **Step 3:** implement. **Step 4:** PASS, clippy, fmt. **Step 5:** Commit `feat(mcp): auth_status and auth_apps`.

### Task 3: `auth_users`, `auth_roles`, `auth_agents`

**Files:** `src/cloud/mcp.rs` (+ arg structs), tests inline.

**Interfaces** (all instance calls; read actions use `Scope::Read`, others `Scope::Write`):
- `auth_users` `{action: "list"|"create"|"disable", env?, email?, name?, principal?, after?: i64, limit?: u32}` → `users_list {after, limit}` / `user_create {email, name}` / `user_disable {principal}`. On create in sandbox, append `"next": "Read the sign-in email with auth_outbox."`.
- `auth_roles` `{action: "list"|"create"|"delete"|"assign"|"unassign", env?, name?, principal?, confirm?: bool}` → `roles_list` / `role_create {name}` / `role_delete {name, confirm}` / `role_assign {principal, role}` / `role_unassign {principal, role}`.
- `auth_agents` `{action: "create"|"revoke", env?, client_id, name?, scopes?: Vec<String>}` → `agent_create {client_id, name, scopes}` (result includes the one-time secret and the same "Store this secret now" note) / `agent_revoke {client_id}`.

- [ ] **Step 1: Failing tests:** one per tool asserting the exact instance tool name and arguments for each action, the scope requested per action (read vs write via the token route body), and `auth_users create` on sandbox carrying the `next` hint.
- [ ] **Steps 2–5** as above. Commit `feat(mcp): auth_users, auth_roles, auth_agents`.

### Task 4: `auth_config` and `auth_promote`

**Files:** `src/cloud/api.rs` (`auth_promote`), `src/cloud/mcp.rs`, tests inline.

**Interfaces:**
- `auth_config` `{action: "export"|"diff"|"apply", env?, doc?: Value, prune?: bool, confirm_lockout?: bool}` → instance `config_export {}` / `config_diff {doc, prune, confirm_lockout}` / `config_apply {…}`. `apply` is a write; an error with `structuredContent.new_client_secrets` must be returned as `Err` text that INCLUDES the structured JSON (Review Focus 5).
- `auth_promote` `{apply?: bool, prune?: bool, confirm_lockout?: bool}` → platform `POST /orgs/{org}/auth/promote` body `{apply, prune, confirm_lockout}`; pass the response through. A 409 `lockout_confirmation_required` returns the risks with the sentence "Re-run with confirm_lockout: true to proceed."; 409 `prune_would_remove_tracked_clients` / `client_bound_elsewhere` / `client_id_taken_by_external` pass through their `client_ids`.

- [ ] **Step 1: Failing tests:** `auth_config_apply_surfaces_minted_secrets` (instance error result with `structuredContent.new_client_secrets`; assert the secret string appears in the `Err` text); `auth_promote_lockout_409_explains_confirm`; `auth_promote_apply_passes_body`.
- [ ] **Steps 2–5.** Commit `feat(mcp): auth_config and auth_promote`.

### Task 5: `auth_test_token`, `auth_outbox`, and `create_sandbox` auth args

**Files:** `src/cloud/mcp.rs`, `src/cloud/api.rs` (`create_sandbox` body), tests inline.

**Interfaces:**
- `auth_test_token` `{client_id, subject, act?, resource?, ttl_secs?}` → sandbox-only instance `test_token` (always `env: "sandbox"`, `Scope::Write`). Result passes through `access_token` (and `id_token`) with the note "Sandbox token: use it as `Authorization: Bearer …` against your app."
- `auth_outbox` `{action: "list"|"read", recipient?, limit?, id?}` → `outbox_list {recipient, limit}` / `outbox_read {id}` on sandbox, `Scope::Read`.
- `CreateSandboxArgs` gains `auth_client: Option<String>` ("Bind this sandbox to the org's sandbox auth as OAuth client <name>; its settings arrive as QUAUTH_ISSUER, QUAUTH_CLIENT_ID and QUAUTH_CLIENT_SECRET" — note: those env names are the platform's public contract, keep them) and `auth_redirect_path: Option<String>` (default `/auth/callback`). `CloudApi::create_sandbox` takes a `CreateSandbox { name, memory_gb, runtime, auth_client, auth_redirect_path }` struct and sends the two fields only when set.

- [ ] **Step 1: Failing tests:** `auth_test_token_is_sandbox_only_and_write_scope` (token route body has `auth_sandbox_write`; instance args match); `auth_outbox_read_passes_id`; `sandbox_only_refusal_is_readable` (instance `isError` text `sandbox_only: …` → hint, Review Focus 3); `create_sandbox_sends_auth_client_when_set` and `…omits_when_unset` (exact JSON bodies).
- [ ] **Steps 2–5.** Commit `feat(mcp): auth_test_token, auth_outbox, create_sandbox auth binding`.

### Task 6: Tool list, server instructions and docs

**Files:** `src/cloud/mcp.rs` (`tool_handler` instructions, `guide` description, `lists_exactly_the_brief_tools`), `docs/reference/mcp.md`, `README.md`.

- [ ] **Step 1:** Update `lists_exactly_the_brief_tools` with the nine `auth_*` names; run → FAIL until the list matches, then PASS.
- [ ] **Step 2:** `tool_handler` instructions mention "auth (sign-in for your apps: sandbox first, then promote)"; `guide` description says the guide includes auth recipes.
- [ ] **Step 3:** `docs/reference/mcp.md`: a `### Auth` section with one row per tool (args, what it does), the sandbox-first loop (create sandbox with `auth_client` → `auth_users create` → `auth_outbox` → `auth_test_token` → `auth_promote`), and the reconnect note: "Connections made before auth existed lack the `auth_read`/`auth_sandbox_write`/`auth_prod_write` scopes; run `quome connect` again. Prod changes need an org owner or admin." Use real tool output captured from the wiremock tests, not invented output. `README.md:96`: add "auth" to the tool families.
- [ ] **Step 4:** `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && cargo build --release`.
- [ ] **Step 5:** Commit `docs(mcp): auth tools`.
