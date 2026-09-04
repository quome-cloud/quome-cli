# Environments + env-var management

Date: 2026-09-04
Status: approved
Branch: `feat/envs-and-env-vars` (stacked on `feat/bindings-and-static-deploy` —
reuses `list_all_pages` and retrofits the bindings `--environment` flags;
lands after that PR merges)

## Problem

The CLI is environment-blind and cannot touch env vars. There is no way to
list an app's environments (users fish UUIDs out of the dashboard to use
`bind --environment`), no way to read or edit plain env vars at either app or
environment scope, and no create/delete/promote. All the backend surfaces
exist; this is CLI-only work — **no backend changes**.

Backend facts the design is built on (verified in quome-fastapi):

- App-level plain env vars live INSIDE the app spec (`spec.env_vars`), edited
  only via `PUT /api/v1/orgs/{org}/apps/{app}` with `{"spec": ...}`. The
  backend's `AppSpecCreate` strips keys it doesn't know — by design
  ("self-cleaning", 2026-08 env-config incident #2608/#2609). A typed CLI
  spec model would re-create that incident from the client side whenever the
  CLI is older than the backend.
- Per-env plain vars live in `app_environments.config_overrides.env_vars`
  (sidecars: `config_overrides.sidecar_env_vars[<container>]`), edited via
  `PATCH .../environments/{env}` with `{"config_overrides": ...}`. The server
  merge-patches config_overrides at the TOP level only (a key's value
  replaces wholesale; `null` deletes the key) — so editing one var requires a
  client-side read-merge-write of the relevant sub-map.
- Effective deploy-time vars = app spec merged with env overrides,
  env-wins-per-key (`app/services/app_env_config.py::merge_env_overrides`,
  consumed by the deploy workflow).
- Environments: `GET/POST /apps/{app}/environments` (paginated envelope;
  create body `{name, deploy_branch?, auto_deploy=true,
  copy_vars_from_environment_id?}`, name validated as a slug),
  `DELETE .../environments/{env}`, `PATCH` (merge-patch as above),
  `POST .../environments/{env}/promote` with
  `{from_environment_id, gate_ack?}` — the gate (both `confirm` AND
  `approval`, which is treated as confirm until the platform's phase-2
  approval flow lands) is type-the-target-environment-name: `gate_ack` must
  equal the target env's `name` or the server 403s with an instructive
  message.
- `spec.sidecars` is a LIST of `{name, env_vars, ...}` objects — sidecar
  lookup is by the `name` field, not a map key.

## Command surface

```
quome apps envs [--app <id>] [--json]
quome apps envs create <name> [--branch <b>] [--no-auto-deploy]
                       [--copy-vars-from <env>] [--app] [--json]
quome apps envs delete <env> [--app] [--force] [--json]
quome apps envs promote <target-env> --from <source-env>
                       [--gate-ack <name>] [--app] [--json]
quome apps envs config [show] --environment <env> [--app] [--json]
quome apps envs config set KEY=VALUE [KEY=VALUE ...] --environment <env> [--app]
quome apps envs config unset KEY [KEY ...] --environment <env> [--app]

quome apps env-vars [--app] [--environment <env>] [--container <name>]
                    [--overrides-only] [--json]
quome apps env-vars set KEY=VALUE [KEY=VALUE ...]
                    [--environment <env>] [--container <name>] [--app]
quome apps env-vars unset KEY [KEY ...]
                    [--environment <env>] [--container <name>] [--app]
```

Every `<env>` / `--environment` value accepts an environment NAME or UUID.

### Environment resolution (shared)

One resolver: fetch the app's environments (paginated), match by exact `name`
first, then by UUID parse. Env names are slug-validated and unique per app —
no ambiguity. Unknown ref errors with "no environment '<ref>' — see
`quome apps envs`". The resolver also RETROFITS the existing
`bind`/`unbind --environment` flags from `Option<Uuid>` to name-or-UUID
(same-branch change to `bindings.rs`).

### `envs` (list)

Table in server (pipeline) order: `NAME SLUG DEFAULT BRANCH AUTO STATUS ID`.
`--json` prints the raw rows.

### `envs create / delete`

- `create <name>`: POST with `{name, deploy_branch, auto_deploy,
  copy_vars_from_environment_id}` (the copy-from ref goes through the
  resolver). Slug validation is server-side; 4xx details pass through.
- `delete <env>`: `inquire::Confirm` prompt naming the blast radius ("deletes
  this environment's deployment target and any dedicated resources
  provisioned for it") unless `--force`. DELETE, then success line.

### `envs promote`

`promote <target> --from <source>` resolves both refs, POSTs
`{from_environment_id, gate_ack}`. Gate handling: if `--gate-ack` was not
provided and the server 403s with the gate message, prompt interactively
("This environment is gated. Type '<target-name>' to confirm:") and retry
once with the typed value; non-interactive contexts (no TTY) get the 403
verbatim plus a hint to pass `--gate-ack <target-name>`. Success prints the
target env row and a note that the target now deploys the source's exact
image digest (no rebuild).

### `envs config` (build/runtime overrides, passthrough)

`--environment` is required on all three verbs — overrides only exist on
environment rows. Operates on `config_overrides` keys OTHER than `env_vars` /
`sidecar_env_vars` (those belong to `env-vars`; `config set env_vars=...` is
rejected client-side). `show` prints the env's current overrides (those two
keys elided); `set K=V...` / `unset K...` read-merge-write the top-level keys
via the server's merge-patch (`null` deletes). Values parse as JSON scalars
when possible (`2` → number, `true` → bool), else strings. NO client-side
knowledge of the resource ladder — server 422s pass through verbatim. This is
deliberately a thin passthrough; the dashboard remains the guided UX.

### `env-vars` (list)

- Without `--environment`: the app's `spec.env_vars` (or, with
  `--container`, that sidecar's `env_vars`). Columns `KEY VALUE`.
- With `--environment`: the EFFECTIVE set — app-level merged with the env's
  overrides, env-wins-per-key, mirroring `merge_env_overrides` — columns
  `KEY VALUE SOURCE` (`app` | `env`). `--overrides-only` restricts to the
  env's own rows.
- Footer note (non-JSON mode): secret-shaped values belong in
  `quome apps bind` (secret-backed vars), not plaintext env vars.
- `--json` prints a `{key, value, source}` array.

### `env-vars set / unset` — the incident-shaped writes

- **App-level** (no `--environment`): GET the app; take `spec` as an OPAQUE
  `serde_json::Value`; mutate only `spec["env_vars"]` (or the matching
  sidecar object's `env_vars` when `--container` is set, located by its
  `name` field — unknown container errors listing the spec's sidecar names);
  PUT `{"spec": <the whole untouched Value>}`. The CLI never defines a typed
  spec model, so it can never strip fields the backend knows about — the
  backend's own schema remains the single cleaner.
- **Per-env** (`--environment`): GET the environment; read
  `config_overrides.env_vars` (or `.sidecar_env_vars[<container>]`);
  merge/remove keys client-side; PATCH
  `{"config_overrides": {"env_vars": <full merged map>}}` (or the
  sidecar_env_vars twin — sending ONLY the touched top-level key, never the
  rest of config_overrides). Removing the last key sends `null` for that
  top-level key so it's dropped entirely.
- KEY grammar client-side: `^[A-Za-z_][A-Za-z0-9_]*$` (POSIX env-var shape)
  plus a rejection of the platform-reserved `QUOME_` prefix, mirroring the
  backend's `reject_reserved_env_keys` — the server stays authoritative for
  anything narrower. `KEY=VALUE` splits on the FIRST `=` (values may contain
  `=`); `set` requires ≥1 pair, `unset` ≥1 key. `unset` of an absent
  key at that scope is an error naming the scope it looked in.
- Best-effort collision warning on `set`: if KEY equals an existing
  binding's `env_var_name` (one `list_bindings` call; failures ignored),
  warn that the binding may shadow the plain var at deploy.
- Every successful write prints the changed keys and "applies on the next
  deploy".

## Code layout

- `src/commands/envs.rs` — `envs` subcommand tree (list/create/delete/
  promote/config) + the shared env resolver (`resolve_environment`) exported
  for `bindings.rs` and `env_vars.rs`.
- `src/commands/env_vars.rs` — `env-vars` list/set/unset, the opaque-spec
  mutation helpers, and the effective-merge rendering.
- `src/api/environments.rs` — typed client methods: `list_environments`
  (paginated), `create_environment`, `delete_environment`,
  `update_environment` (config_overrides PATCH), `promote_environment`; a
  typed `AppEnvironment` model (id, name, slug, is_default, deploy_branch,
  auto_deploy, status, sort_order, config_overrides as `serde_json::Value`).
- `src/commands/apps.rs` — two new `AppsCommands` variants (`Envs`,
  `EnvVars`) delegating to the new modules; `bindings.rs` swaps its
  `--environment: Option<Uuid>` for the resolver.
- `src/ui.rs` — `EnvRow`, `EnvVarRow` table structs.
- Docs: `docs/reference/environments.md` (new) + updates to
  `docs/reference/apps.md` (bindings `--environment` now takes names) and
  the README index.

## Testing

- Unit: KEY=VALUE parsing (first-`=` split, key grammar), opaque-spec
  mutation on fixture JSON (env_vars added/removed; sidecar located by name;
  unknown container error; REST OF SPEC BYTE-IDENTICAL — assert on the full
  Value), config_overrides merge/unset incl. last-key-null, effective-merge
  + SOURCE labeling, env resolution (name hit, UUID passthrough, miss),
  config-set rejection of env_vars keys, JSON-scalar value parsing.
- Live verification: command list presented for explicit approval first
  (standing rule — no org writes without it), exercised against a
  user-designated test app.

## Out of scope

- The platform's phase-2 approval flow for promote (CLI already forwards
  `gate_ack`; nothing more exists to integrate with today).
- Editing non-env-var spec fields (ports, sidecar images, volumes) from the
  CLI.
- Backend changes of any kind.
