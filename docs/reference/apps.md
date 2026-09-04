# Apps: `apps list|create|get|update|delete`

Applications are the deployable unit: a container (from an image or a GitHub repo) running in your org's isolated cloud project with a URL, logs, and deployment history.

## `quome apps list`

```
Usage: quome apps list [OPTIONS]

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

```console
$ quome apps list
╭──────────────────────────────────────┬────────┬─────────┬───────────────────────────┬──────────────────╮
│ ID                                   │ NAME   │ STATUS  │ URL                       │ CREATED          │
├──────────────────────────────────────┼────────┼─────────┼───────────────────────────┼──────────────────┤
│ 7c9e6679-7425-40de-944b-e07fc1f90ae7 │ my-api │ running │ https://my-api-acme.q.run │ 2026-07-02 07:14 │
╰──────────────────────────────────────┴────────┴─────────┴───────────────────────────┴──────────────────╯
```

Statuses: `pending` → `provisioning` → `running`, plus `stopped`, `failed`, `deleting`.

## `quome apps create`

```
Usage: quome apps create [OPTIONS] <NAME>

Arguments:
  <NAME>  Application name (lowercase letters, digits, hyphens)

Options:
  -d, --description <DESCRIPTION>  Application description
      --image <IMAGE>              Container image (e.g., nginx:1.27) — creates an image-sourced app
      --repo <REPO>                GitHub repository as owner/name — creates a git-sourced app
      --branch <BRANCH>            Git branch (used with --repo) [default: main]
      --port <PORT>                Container port [default: 8080]
      --org <ORG>                  Organization ID (uses linked org if not provided)
      --json                       Output as JSON
```

Exactly one source is required — `--image` or `--repo` (they conflict):

```bash
# From a container image
quome apps create my-api --image ghcr.io/acme/my-api:v1.2.0 --port 3000

# From a GitHub repo (built and deployed on push to the branch)
quome apps create my-api --repo acme/my-api --branch main
```

```console
$ quome apps create my-api --image ghcr.io/acme/my-api:v1.2.0 --port 3000
✓ Created application
  ID      7c9e6679-...
  Name    my-api
  Status  pending
```

Names must match `^[a-z0-9][a-z0-9-]*[a-z0-9]$` — lowercase, digits, hyphens, no leading/trailing hyphen. Git-sourced apps require the Quome GitHub App to be installed on the repo (dashboard → integrations).

Tutorials: [Deploy your first app](../tutorials/deploy-your-first-app.md) · [Deploy from GitHub](../tutorials/deploy-from-github.md)

## `quome apps get`

```
Usage: quome apps get [OPTIONS]

Options:
  -i, --id <ID>    Application ID (uses linked app if not provided)
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

```console
$ quome apps get
┌ my-api ─────────────────────────────────────┐
│ ID       7c9e6679-...                       │
│ Name     my-api                             │
│ Status   running                            │
│ Source   image                              │
│ Image    ghcr.io/acme/my-api:v1.2.0         │
│ URL      https://my-api-acme.q.run          │
│ Created  2026-07-02 07:14:02                │
│ Updated  2026-07-02 07:16:41                │
└─────────────────────────────────────────────┘
```

Git-sourced apps show `Repo` and branch instead of `Image`. `--json` includes the full spec.

## `quome apps update`

```
Usage: quome apps update [OPTIONS]

Options:
  -i, --id <ID>                    Application ID (uses linked app if not provided)
      --description <DESCRIPTION>  New description
      --branch <BRANCH>            New deploy branch (git-sourced apps)
      --org <ORG>                  Organization ID (uses linked org if not provided)
      --json                       Output as JSON
```

```bash
quome apps update --branch release   # switch which branch deploys
```

App names are immutable; richer spec changes (env vars, resources, domains) are dashboard territory today.

## `quome apps delete`

```
Usage: quome apps delete [OPTIONS] <ID>

Arguments:
  <ID>  Application ID

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
  -f, --force      Skip confirmation prompt
```

```console
$ quome apps delete 7c9e6679-7425-40de-944b-e07fc1f90ae7
? Are you sure you want to delete application 7c9e6679-...? Yes
✓ Deleted application
  ID  7c9e6679-...
```

Deletion tears down the app's infrastructure asynchronously — the app shows `deleting` until it's gone.

## Bindings

A binding injects a secret, database, storage bucket, or cache into the app as an environment variable. Bindings are managed with `quome apps bind` / `quome apps bindings` / `quome apps unbind`, and are **org-admin gated** server-side on top of ordinary app permissions — an API key without org-admin gets a 403 with the hint "(managing bindings requires org admin)".

### `quome apps bind`

```
Usage: quome apps bind [OPTIONS] --env-var <ENV_VAR> <--secret <SECRET>|--database <DATABASE>|--bucket <BUCKET>|--cache <CACHE>>

Options:
      --env-var <ENV_VAR>          Environment variable name (must match ^[A-Z][A-Z0-9_]*$)
      --secret <SECRET>            Secret to bind (name or UUID)
      --database <DATABASE>        Database to bind (name or UUID)
      --bucket <BUCKET>            Storage bucket to bind (name or UUID)
      --cache <CACHE>              Cache to bind (name or UUID)
      --app <APP>                  Application ID (uses linked app if not provided)
      --org <ORG>                  Organization ID (uses linked org if not provided)
      --environment <ENVIRONMENT>  Bind only for one app environment (environment UUID)
      --preview                    Also inject into PR preview deploys (app-level bindings only)
      --container <CONTAINER>      Target container for multi-container apps
      --json                       Output as JSON
```

Exactly one resource flag is required (`--secret`, `--database`, `--bucket`, or `--cache`) — they're mutually exclusive. Each accepts a resource name or a UUID; a name is resolved against that resource type's list endpoint (paginated in full, not just the first page), and an unrecognized name errors with a hint: `quome secrets list` / `quome db list` for secrets and databases, the Storage/Caches page in the dashboard for buckets and caches (those have no CLI list command yet).

```bash
quome apps bind --env-var DATABASE_PASSWORD --secret prod-db-password
```

```console
$ quome apps bind --env-var DATABASE_PASSWORD --secret prod-db-password
✓ Created binding
  Env var     DATABASE_PASSWORD
  Resource    secret prod-db-password
  Binding ID  9b2f0a34-...
```

By default a binding applies to the app everywhere. Two scope flags narrow that, and are themselves mutually exclusive:

- `--environment <ID>` scopes the binding to one app environment (dev/staging/prod) — it never injects into other environments or into PR previews.
- `--preview` additionally injects the (app-level) binding into PR preview deploys. It can't be combined with `--environment`, since environment-scoped overrides are never injected into previews.

### `quome apps bindings`

```
Usage: quome apps bindings [OPTIONS]

Options:
      --app <APP>  Application ID (uses linked app if not provided)
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

```console
$ quome apps bindings
╭────────────────────┬────────┬───────────────────┬────────────┬───────────────────────────────────────╮
│ ENV VAR             │ TYPE   │ RESOURCE           │ SCOPE      │ BINDING ID                              │
├────────────────────┼────────┼───────────────────┼────────────┼───────────────────────────────────────┤
│ DATABASE_PASSWORD   │ secret │ prod-db-password   │ app        │ 9b2f0a34-...                            │
│ ASSETS_BUCKET       │ bucket │ app-assets         │ env:staging│ 3c1d2e4f-...                            │
╰────────────────────┴────────┴───────────────────┴────────────┴───────────────────────────────────────╯
```

`SCOPE` reads `app` (everywhere), `preview` (app-level, also injected into PR previews), or `env:<name>` (one app environment; falls back to the raw environment ID if environment names can't be resolved). `RESOURCE` shows the bound resource's name, falling back to its raw ID if the resource has since been deleted.

### `quome apps unbind`

```
Usage: quome apps unbind [OPTIONS] [BINDING_ID]

Arguments:
  [BINDING_ID]  Binding ID to remove (or use --env-var)

Options:
      --env-var <ENV_VAR>          Resolve the binding by env var name instead of ID
      --environment <ENVIRONMENT>  Disambiguate --env-var to one environment's binding
      --app <APP>                  Application ID (uses linked app if not provided)
      --org <ORG>                  Organization ID (uses linked org if not provided)
  -f, --force                      Skip confirmation prompt
      --json                       Output as JSON
```

Remove by binding ID, or by `--env-var NAME` (pass `--environment` too if the same env var is bound at both app scope and an environment scope — the CLI never guesses between them and instead lists the candidates and asks for the binding ID).

```console
$ quome apps unbind --env-var DATABASE_PASSWORD
? Remove binding 9b2f0a34-... (DATABASE_PASSWORD → secret)? Yes
✓ Removed binding
  Env var     DATABASE_PASSWORD
  Binding ID  9b2f0a34-...
```
