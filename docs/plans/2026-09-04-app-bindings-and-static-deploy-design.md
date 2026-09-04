# App resource bindings + static-site deploy

Date: 2026-09-04
Status: approved
Repos: quome-cli (this spec); quome-fastapi follow-up in "Cross-repo cleanup"

## Problem

Two gaps between the CLI and the dashboard:

1. **No binding management.** `quome secrets` manages secret *values* only.
   Attaching a secret (or database/bucket/cache) to an app as an env var —
   an `AppBinding` row — is dashboard-only. CLI users can create a secret
   but not wire it into an app.
2. **No static-site deploys.** The platform's static deployment API
   (manifest → signed PUT URLs → finalize → poll) is drivable only by a
   vestigial Python CLI living inside the quome-fastapi monorepo
   (`quome-cli/` there — two commands, `deploy` and `whoami`). The Rust
   CLI's `deployments trigger` covers git-sourced apps only. The Python dir
   is slated for removal once this port ships.

## Part A — `quome apps bind / bindings / unbind`

### Command surface

```
quome apps bindings [--app <uuid>] [--org <uuid>] [--json]

quome apps bind --env-var <NAME>
                (--secret <name|uuid> | --database <name|uuid> |
                 --bucket <name|uuid> | --cache <name|uuid>)
                [--app <uuid>] [--org <uuid>]
                [--environment <uuid>] [--preview]
                [--container <name>] [--json]

quome apps unbind (<binding-id> | --env-var <NAME> [--environment <uuid>])
                  [--app <uuid>] [--org <uuid>] [--force] [--json]
```

- Lives in the `apps` namespace: bindings are app-scoped configuration and
  the API is generic across resource types — one home covers them all.
- `--app`/`--org` follow the existing convention: `Option<Uuid>`, falling
  back to the linked context (`Config::require_linked_app` /
  `require_linked_org`, `src/config.rs:189-205`).
- Exactly one resource flag per `bind` (clap `ArgGroup`, required). The
  value is a UUID when it parses as one, otherwise a name resolved via that
  type's list endpoint (secrets/databases/buckets/caches are all
  name-unique per org). Ambiguity is impossible; a name that matches
  nothing is a clear "no <type> named '<value>'" error listing how to see
  candidates (`quome secrets list`, etc.).
- `event_subscription` is a valid server-side `ResourceType` but is NOT
  offered — those bindings are created by the events flows, not manually.

### Server contract (quome-fastapi, existing — no backend changes)

| Call | Notes |
|---|---|
| `GET /api/v1/orgs/{org}/apps/{app}/bindings` | returns binding rows; rows whose `env_var_name` is the reserved selection sentinel are already filtered server-side |
| `POST /api/v1/orgs/{org}/apps/{app}/bindings` | body `{resource_type, resource_id, env_var_name, container_name?, environment_id?, allow_in_preview}` |
| `DELETE /api/v1/orgs/{org}/apps/{app}/bindings/{binding_id}` | 204 |

Server-side rules the CLI mirrors for fast, friendly errors (the server
remains the authority — the CLI never suppresses a server error):

- `env_var_name` must match `^[A-Z][A-Z0-9_]*$` (max 255). The CLI
  validates before calling and suggests the upper-snake-cased form of what
  the user typed.
- `--preview` (`allow_in_preview: true`) is only valid WITHOUT
  `--environment` (env-scoped overrides are never injected into previews).
  The CLI rejects the combination client-side with the server's rationale.
- Both list and mutate are org-admin gated server-side (`verify_org_admin`)
  on top of app view/update. A 403 gets the hint
  `"managing bindings requires org admin"`.
- `environment_id` is accepted only when the platform has app environments
  enabled; a server 403 mentioning environments passes through verbatim.

### Output

`bindings` renders the standard `ui` table:

```
ENV VAR             TYPE      RESOURCE              SCOPE        BINDING ID
DATABASE_PASSWORD   secret    prod-db-password      app          9b2f…
STAGING_DEBUG_KEY   secret    deploy-bot-ssh-key    env:staging  4c81…
ASSETS_BUCKET       bucket    assets-bucket         preview      77aa…
```

- RESOURCE is the resolved name (one list call per distinct resource type
  present, then joined in-memory; falls back to the raw UUID if the
  resource row is gone).
- SCOPE: `app` (no environment, no preview), `env:<name>` (environment_id
  set; env name resolved via the app-environments list, falling back to
  the UUID), `preview` (allow_in_preview).
- `--json` emits the raw API rows (with a `resource_name` field added),
  consistent with the other commands' `--json` behavior.

`bind` prints the created row in the same one-line table form; `unbind`
confirms (`Removed binding DATABASE_PASSWORD (secret prod-db-password)`)
and prompts before deleting unless `--force`, matching `secrets delete`.

`unbind --env-var NAME` resolves via the bindings list scoped to the same
`--environment` value (or app-level when omitted). If the name matches
more than one row (possible across scopes), the CLI lists the matches with
their binding ids and exits nonzero asking for the id — it never guesses.

### Code layout

- `src/commands/bindings.rs` — the three subcommand arg structs + handlers
  (new file; `apps.rs` gains three `AppsCommands` variants delegating to
  it, keeping `apps.rs` from growing past its current single-screen-per-
  command shape).
- `src/api/apps.rs` — `list_bindings`, `create_binding`, `delete_binding`.
- `src/api/models.rs` — `AppBinding`, `CreateBindingRequest`,
  `BindingResourceType` (serde snake_case enum: `secret`, `database`,
  `bucket`, `cache`, `event_subscription` for deserialization
  completeness).
- `src/ui.rs` — `BindingRow` table impl following `SecretRow`.
- `docs/reference/apps.md` — new Bindings section documenting all three
  subcommands with examples.

## Part B — `quome deploy` (static sites)

Behavior-parity port of the Python CLI's `deploy` command.

### Command surface

```
quome deploy [<directory>] [--app <slug|uuid>] [--org <uuid>]
             [--create] [--json]
```

- `<directory>` defaults to `.`. Must contain a root `index.html` (hard
  error otherwise, same as the Python original).
- `--app` accepts a slug or UUID; falls back to the linked app. `--create`
  creates the app when the slug doesn't resolve (mirrors the Python
  `_resolve_app(create=...)`).
- Top-level command (`quome deploy`), not under `apps` — it's the
  highest-frequency action for static-site users and the Python CLI
  established the name.

### Flow (server contract, existing — no backend changes)

1. Walk the directory; build the manifest (relative path, byte size,
   content-type inferred by extension — port the Python `manifest.py`
   table). Skip dotfiles/dirs. Enforce the server's per-file and total
   size limits client-side by passing sizes through and surfacing the
   server's 4xx verbatim.
2. `POST /api/v1/orgs/{org}/apps/{app}/static/deployments` with the
   manifest → `{deploy_id, upload_urls: {path: signed_put_url}}`.
3. PUT each file directly to its signed URL with the manifest
   content-type, concurrently (bounded, 8 at a time — the Python original
   used a thread pool), with a progress bar via the crate the CLI already
   uses for host downloads.
4. `POST .../static/deployments/{deploy_id}/finalize`.
5. Poll `GET .../static/deployments` for the deploy's terminal state
   (`active` | `failed`) — same cadence as the Python original: 2s
   interval, 180s budget — then print the live URL (or the failure
   detail).

Copy-on-write deploys (`--base <deploy-id>` on finalize) are a documented
follow-up — the server supports it; v1 is parity with the Python CLI,
which always uploads the full directory.

### Auth note

Static deploy permissions resolve to the admin level server-side, so the
key needs `*` or `admin:app` scope (quome-fastapi `specs.md`, Static Sites
section). The 403 hint names that requirement.

### Code layout

- `src/commands/deploy.rs` — command + orchestration.
- `src/manifest.rs` — directory walk + content-type table (ported from
  the Python `manifest.py`, including its extension map).
- `src/api/static_sites.rs` — the three API calls (create with manifest,
  finalize, list/poll) + models.
- `docs/reference/deploy.md` — new reference page.

## Testing

- Unit: manifest walk (fixture dir with nested files, dotfile exclusion,
  index.html requirement), env-var pattern validation, resource-flag
  arg-group rejection, name-vs-UUID resolution branching, scope rendering.
- API-layer tests follow the repo's existing mock pattern (`src/api/*`
  functions are thin; the commands' resolution/error logic carries the
  tests).
- Manual verification against dev: bind/list/unbind on a test app with a
  test secret; `quome deploy` of a two-file fixture site to a fresh app,
  confirming the printed URL serves; re-deploy over it.

## Cross-repo cleanup (quome-fastapi, after Part B ships)

- Delete the monorepo's Python `quome-cli/` directory.
- Rewrite the `specs.md` Static Sites paragraph that documents
  `pip install ./quome-cli` to point at the Rust CLI's `quome deploy`.
- Historical plan/spec docs referencing the Python CLI stay untouched.
- Gate: the Rust `quome deploy` has been released and manually verified
  against dev.

## Out of scope

- `event_subscription` binds (created by event flows).
- Copy-on-write `--base` deploys (server-ready; CLI follow-up).
- Backend changes of any kind — both features ride existing endpoints.
- Binding *updates* (the API has no PATCH; the dashboard does
  delete+recreate, and `unbind` + `bind` covers it).
