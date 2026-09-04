# Environments: `apps envs` + `apps env-vars`

Pipeline environments (e.g. dev/staging/prod) let one app have several deploy
targets, each with its own branch, build/runtime overrides, and env-var
overrides. Apps that never opted into multiple environments have none —
everything below still works, it just operates at app scope. Every command
that takes an environment reference (`--environment`, `envs delete <env>`,
`envs promote <target> --from <source>`, `envs create --copy-vars-from
<env>`) accepts either the environment's **name** or its **UUID**, resolved
name-first (so a name that happens to look like a UUID is still matched by
name). An unresolvable reference errors with a pointer to `quome apps envs`.

## `quome apps envs`

```
Usage: quome apps envs <COMMAND>

Commands:
  list     List the app's environments (pipeline order)
  create   Create an environment
  delete   Delete an environment (tears down its deploy target)
  promote  Promote a source environment's exact image to a target environment
  config   Show or edit an environment's build/runtime override keys
```

```console
$ quome apps envs
NAME     SLUG     DEFAULT  BRANCH   AUTO  STATUS  ID
prod     prod     yes      main     yes   active  9b2f0a34-...
staging  staging           staging  yes   active  3c1d2e4f-...
```

Rows print in server (pipeline) order. `--json` prints the raw rows.

### `quome apps envs create`

```
Usage: quome apps envs create [OPTIONS] <NAME>

Arguments:
  <NAME>  Environment name (lowercase slug)

Options:
      --branch <BRANCH>                  Deploy branch for this environment
      --no-auto-deploy                   Disable auto-deploy on push
      --copy-vars-from <COPY_VARS_FROM>  Copy plain env vars from another environment (name or UUID)
      --app <APP>                        Application ID (uses linked app if not provided)
      --org <ORG>                        Organization ID (uses linked org if not provided)
      --json                             Output as JSON
```

```bash
quome apps envs create staging --branch staging
quome apps envs create qa --branch qa --copy-vars-from staging --no-auto-deploy
```

`--copy-vars-from <name|UUID>` copies that environment's plain env vars (not
secrets/bindings) into the new one at creation time. Name validation
(lowercase slug) is server-side; 4xx details pass through verbatim.

### `quome apps envs delete`

```
Usage: quome apps envs delete [OPTIONS] <ENV>

Arguments:
  <ENV>  Environment to delete (name or UUID)

Options:
      --app <APP>  Application ID (uses linked app if not provided)
      --org <ORG>  Organization ID (uses linked org if not provided)
  -f, --force      Skip confirmation prompt
      --json       Output as JSON
```

```console
$ quome apps envs delete staging
? Delete environment 'staging'? This tears down its deployment target and any
  dedicated resources provisioned for it. Yes
✓ Deleted environment
  Name  staging
```

**Destructive.** Deletion tears down the environment's deploy target and any
resources dedicated to it — this cannot be undone. The command prompts for
confirmation unless `--force` is given. The app's **default** environment can
never be deleted (the server refuses it, and the CLI checks first so the
error is immediate).

### `quome apps envs promote`

```
Usage: quome apps envs promote [OPTIONS] --from <FROM> <TARGET>

Arguments:
  <TARGET>  Target environment (name or UUID)

Options:
      --from <FROM>          Source environment (name or UUID)
      --gate-ack <GATE_ACK>  Gate acknowledgement: the TARGET environment's name (for gated envs / CI)
      --app <APP>            Application ID (uses linked app if not provided)
      --org <ORG>            Organization ID (uses linked org if not provided)
      --json                 Output as JSON
```

```bash
quome apps envs promote prod --from staging
```

Promotion deploys the **source's exact image digest** to the target — no
rebuild. Some environments are gated: promoting into them requires typing the
target environment's name as an acknowledgement.

- Interactively (a TTY, no `--gate-ack` passed): on a gate denial the CLI
  prompts `This environment is gated. Type the environment name to confirm:`
  and retries once with the typed value.
- Non-interactively (CI, piped stdin): the gate denial passes through as an
  error with a hint to pass `--gate-ack <target-name>` up front.

### `quome apps envs config`

```
Usage: quome apps envs config [OPTIONS] [COMMAND]

Commands:
  show   Show the environment's override keys (default)
  set    Set override keys (KEY=VALUE ...)
  unset  Remove override keys

Options:
      --environment <ENVIRONMENT>  Environment (name or UUID) — required
      --app <APP>                  Application ID (uses linked app if not provided)
      --org <ORG>                  Organization ID (uses linked org if not provided)
      --json                       Output as JSON
```

`envs config` is a thin passthrough to the environment's **build/runtime**
override keys (`config_overrides`) — memory, CPU, resource tier, and similar
knobs. `--environment` is required on all three verbs (overrides only exist
on environment rows; clap can't mark a global flag required, so this is
enforced at runtime). The CLI has no client-side knowledge of the resource
ladder — server 422s pass through verbatim; the dashboard remains the guided
UX for choosing values.

```bash
quome apps envs config show --environment staging
quome apps envs config set memory=1Gi cpu=1 --environment staging
quome apps envs config unset memory --environment staging
```

`env_vars` and `sidecar_env_vars` are rejected here — those two keys belong
to `quome apps env-vars` below, and are elided from `config show`'s output so
the two commands never fight over the same keys.

## `quome apps env-vars`

Plain (non-secret) environment variables, at either app scope or one
environment's scope. Managed separately from `envs config` above, and from
[resource bindings](apps.md#bindings) (secret/database/bucket/cache-backed
env vars).

```
Usage: quome apps env-vars <COMMAND>

Commands:
  list   List env vars (effective view with --environment)
  set    Set env vars (KEY=VALUE ...)
  unset  Remove env vars
```

### `quome apps env-vars list`

```
Usage: quome apps env-vars list [OPTIONS]

Options:
      --app <APP>                  Application ID (uses linked app if not provided)
      --org <ORG>                  Organization ID (uses linked org if not provided)
      --environment <ENVIRONMENT>  Environment (name or UUID) — shows the effective merged set
      --container <CONTAINER>      Sidecar container name
      --overrides-only             With --environment: show only the environment's own overrides
      --json                       Output as JSON
```

```console
$ quome apps env-vars list
KEY        VALUE        SOURCE
LOG_LEVEL  info         app

$ quome apps env-vars list --environment staging
KEY        VALUE        SOURCE
LOG_LEVEL  debug        env
PORT       8080         app
```

- Without `--environment`: the app's own env vars (every row's `SOURCE` is
  `app`).
- With `--environment`: the **effective** deploy-time set — the app's vars
  merged with that environment's overrides, environment wins per key
  (`SOURCE` reads `app` or `env` accordingly). Add `--overrides-only` to see
  just the environment's own rows instead of the merge.
- `--container <name>` scopes to one sidecar's env vars instead of the main
  container's.
- Non-string values (numbers, booleans) render as their JSON text so they're
  never silently blank.
- `--json` prints a `[{"key", "value", "source"}, ...]` array.

The list footer reminds you that secret-shaped values belong in `quome apps
bind`, not here — this command only ever stores plaintext.

### `quome apps env-vars set` / `unset`

```
Usage: quome apps env-vars set [OPTIONS] <PAIRS>...

Arguments:
  <PAIRS>...  KEY=VALUE pairs

Options:
      --environment <ENVIRONMENT>
      --container <CONTAINER>
      --app <APP>
      --org <ORG>
      --json

Usage: quome apps env-vars unset [OPTIONS] <KEYS>...

Arguments:
  <KEYS>...  Keys to remove

Options:
      --environment <ENVIRONMENT>
      --container <CONTAINER>
      --app <APP>
      --org <ORG>
      --json
```

```bash
quome apps env-vars set LOG_LEVEL=debug PORT=8080
quome apps env-vars set LOG_LEVEL=debug --environment staging
quome apps env-vars set WORKER_CONCURRENCY=4 --container worker
quome apps env-vars unset PORT
```

```console
$ quome apps env-vars set LOG_LEVEL=debug --environment staging
Updated LOG_LEVEL on environment 'staging' — applies on the next deploy
```

Key grammar (checked client-side before any request): `^[A-Za-z_][A-Za-z0-9_]*$`,
and the platform-reserved `QUOME_` prefix is rejected. `KEY=VALUE` splits on
the **first** `=`, so values may themselves contain `=`. `unset` of a key
that isn't set at the given scope is an error naming the scope it looked in
(app spec, an environment's overrides, or a named sidecar's overrides).

If a key being set matches an existing [binding](apps.md#bindings)'s env var
name, the CLI prints a warning that the binding may shadow this plain value
at deploy — it still proceeds (best-effort check; a failed lookup is
silently ignored).

Every successful `set`/`unset` prints the changed keys and a reminder that
the change **applies on the next deploy** — there is no live reload.

#### Write semantics (why the writes look the way they do)

The write paths are shaped by a real incident (quome-fastapi #2608/#2609:
the backend's spec model silently stripped fields it didn't recognize when
older/newer clients round-tripped a full spec object).

- **App-level** (no `--environment`): the CLI fetches the app, treats
  `spec` as an **opaque** JSON value, mutates only `spec.env_vars` (or the
  named sidecar's `env_vars`, matched by its `name` field — an unknown
  container name errors listing the app's actual sidecar names), and PUTs
  the **whole untouched spec** back. The CLI never defines a typed spec
  model, so it can never strip a field it doesn't know about — the
  backend's own schema stays the single source of truth for cleanup.
- **Per-environment** (`--environment`): the CLI fetches the environment,
  reads `config_overrides.env_vars` (or `.sidecar_env_vars[<container>]`),
  merges/removes keys client-side, and sends a merge-patch containing
  **only that one touched top-level key** — never the rest of
  `config_overrides`. Removing the last key in a scope sends `null` for
  that key so it's dropped entirely rather than left as `{}`.

## See also

- [Apps](apps.md) — app CRUD and [resource bindings](apps.md#bindings)
  (secret/database/bucket/cache-backed env vars, as distinct from the plain
  vars managed here).
- [Deploy](deploy.md) / [Deployments](deployments.md) — env var and override
  changes take effect on the next deploy, not immediately.
