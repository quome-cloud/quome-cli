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
