# delete_app and sync_folder Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two MCP tools an agent asked for: `delete_app` so it can clean up an app it deployed, and `sync_folder` so it can put a local folder into a sandbox the same way `deploy_app` takes one.

**Architecture:** Both are thin tools over client calls that already exist or map onto one existing Quome Cloud route. `delete_app` follows `uninstall_app`'s confirm-or-preview shape over `DELETE /services/{id}`. `sync_folder` reuses `pack_folder`'s walk and ignore rules (`.dockerignore`, read in `src/cloud/pack.rs`) and `CloudApi::upload` (chunked PUT per file) behind `check_workspace_path`.

**Tech Stack:** Rust, rmcp `#[tool]`, wiremock tests in `src/cloud/mcp.rs`.

**Spec:** none; the request is the two tool descriptions below.

## Global Constraints

- This repo is public: no internal repo names, no dev cluster URLs, no references to any other codebase in code, tests, docs or commit messages. Say "Quome Cloud".
- Destructive tools preview without `confirm: true` and act only with it, exactly like `uninstall_app` (`AppConfirmArgs` pattern: `{app|name, confirm?}`).
- Sandbox writes go only under `/workspace/`: every destination path passes `check_workspace_path` (`src/cloud/mcp.rs:153`).
- `sync_folder` honours the same ignore rules as `deploy_app`'s `pack_folder` (`.dockerignore` at the folder root; `Dockerfile` and `.dockerignore` themselves are never ignored) and the same size cap on the whole folder; a file is uploaded with `CloudApi::upload` (8 MiB chunks) one at a time, in walk order.
- `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` clean. Build with `CARGO_TARGET_DIR=/Users/jimschwoebel/Desktop/quome-cli/target`.

## Review Focus

1. `sync_folder` with `dest` of `/workspace` or a path with `..` → refused before any request (Task 1 test).
2. A folder with no files after ignores → `{"files": 0, "bytes": 0}` and no upload request (Task 1 test).
3. `delete_app` for a name that doesn't exist → error, no DELETE sent (Task 1 test).
4. `delete_app` without `confirm` → preview that names the app and says it is irreversible, no DELETE sent (Task 1 test).
5. A failed upload mid-folder → the result says which file failed and how many were written before it, not a bare error (Task 1 test).

---

### Task 1: `delete_app` and `sync_folder`

**Files:**
- Modify: `src/cloud/api.rs` (one new call), `src/cloud/pack.rs` (expose the walk as a file list), `src/cloud/mcp.rs` (two tools, args structs, tests), `docs/reference/mcp.md` (two rows), `docs/reference/apps.md` if it lists MCP tools (check)

**Interfaces:**
- `api.rs`: `pub async fn delete_service(&self, id: &str) -> Result<Service, QuomeError>` → `DELETE {org}/services/{id}` (202, body is the service).
- `pack.rs`: `pub fn list_folder(dir: &Path) -> Result<Vec<(String, PathBuf)>, QuomeError>` — the relative path (forward slashes) and absolute path of every file `pack_folder` would include, in the same order, with the same ignore rules and the same size cap error. Refactor `pack_folder` to build on it (no duplicated walk). Unlike `pack_folder`, it does **not** require a `Dockerfile`.
- `mcp.rs`:
  - `DeleteAppArgs { name: String, confirm: Option<bool> }` (doc comments like `AppConfirmArgs`).
  - `delete_app` tool, description: `"Delete a Quome Cloud app you deployed: its service, URL and running container. Without confirm: true it only previews; with it, deletes. This cannot be undone."` Without confirm → `{"app": name, "id": app.id, "deleted": false, "warning": "Deleting removes the app, its URL and its running container. This cannot be undone.", "next": "Ask the user, then call delete_app again with confirm: true."}`. With confirm → `call!(api.delete_service(&app.id))` → `{"app": name, "id": app.id, "deleted": true, "status": service.status}`. Unknown name → `Err("No app named {name:?}.")` (same text as `app_status`).
  - `SyncFolderArgs { sandbox: String, folder: String, dest: Option<String> }` — `dest` default `/workspace`, must pass `check_workspace_path` after joining (so `/workspace` alone is allowed as the *root* but each file path `/workspace/<rel>` is checked; `dest` itself may be `/workspace` or deeper).
  - `sync_folder` tool, description: `"Upload a local folder into a sandbox under dest (default /workspace), honouring the folder's .dockerignore. Existing files are overwritten; files in the sandbox that are not in the folder are left alone. Returns the file and byte counts."` Result: `{"sandbox": id, "dest": dest, "files": n, "bytes": total}`; on a failed upload: `Err(format!("sync_folder stopped at {rel:?} after {written} file(s): {e}"))`.
- Tests (wiremock, in the existing `#[cfg(test)]` module; use `start()`, `c.call`, `docker_dir()` or a `tempfile::TempDir` you fill):
  - `delete_app_previews_without_confirm` — services list has `app1`; `delete_app {name}` → `deleted == false`, warning present; assert no DELETE mock was hit (don't mount one; wiremock answers 404 and the result would error).
  - `delete_app_deletes_with_confirm` — mount `DELETE …/services/app1` → 202 with a service body status `deleting`; result `deleted == true`, `status == "deleting"`.
  - `delete_app_unknown_name_sends_nothing` — `Err` contains `No app named`.
  - `sync_folder_uploads_each_file_under_dest` — temp dir with `a.txt`, `sub/b.txt`, `.dockerignore` containing `ignored.txt`, and `ignored.txt`; mount `PUT …/sandboxes/sb1/workspace-files` matching `path=/workspace/app/a.txt` and `/workspace/app/sub/b.txt`; call with `dest: "/workspace/app"` → `files == 2`, bytes = sum; assert the ignored path was never requested (`s.received_requests()` filter).
  - `sync_folder_refuses_bad_dest` — `dest: "/etc"` and `dest: "/workspace/../x"` → `Err` containing `Refusing`, no requests.
  - `sync_folder_empty_folder` — only `.dockerignore` ignoring `*` → `files == 0`, no PUT.
  - `sync_folder_reports_the_failing_file` — second PUT answers 500 → `Err` contains `stopped at "sub/b.txt" after 1 file`.

- [ ] **Step 1: Write the seven failing tests**; run `CARGO_TARGET_DIR=… cargo test cloud::mcp::tests::delete_app cloud::mcp::tests::sync_folder` → FAIL (unknown tools).
- [ ] **Step 2: Implement** `delete_service`, `list_folder` (+ `pack_folder` on top of it), the two args structs and tools, and register them where the tool list is enumerated (grep `"uninstall_app"` for the places a tool name appears, e.g. a tool-name list in tests).
- [ ] **Step 3: Run the tests** → PASS; then `cargo test`, clippy, fmt.
- [ ] **Step 4: Docs** — `docs/reference/mcp.md`: a `delete_app` row after `app_status` and a `sync_folder` row after `write_file`, in the table's style; mention in the `deploy_app`/sandbox prose if there is a natural sentence. Check `README.md`'s tool list if it has one.
- [ ] **Step 5: Commit** — `feat(mcp): delete_app and sync_folder`.
