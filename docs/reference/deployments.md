# Deployments: `deployments list|get|create`

A deployment is one attempt to roll out your app — triggered by a git push, the dashboard, or `deployments create`. These commands need an app context (linked, `--app`, or `QUOME_APP`).

## `quome deployments list`

```
Usage: quome deployments list [OPTIONS]

Options:
      --app <APP>  Application ID (uses linked app if not provided)
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

```console
$ quome deployments list
╭──────────────────────────────────────┬─────────────┬────────┬──────────────────╮
│ ID                                   │ STATUS      │ BRANCH │ CREATED          │
├──────────────────────────────────────┼─────────────┼────────┼──────────────────┤
│ 9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d │ success     │ main   │ 2026-07-02 07:20 │
│ 1ee7f2a4-5c6b-4d8e-9f0a-1b2c3d4e5f6a │ in_progress │ main   │ 2026-07-02 07:32 │
╰──────────────────────────────────────┴─────────────┴────────┴──────────────────╯
```

Statuses: `created` → `in_progress` → `success` | `failed` | `cancelled`.

## `quome deployments get`

```
Usage: quome deployments get [OPTIONS] <ID>

Arguments:
  <ID>  Deployment ID

Options:
      --app <APP>  Application ID (uses linked app if not provided)
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

```console
$ quome deployments get 9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d
┌ Deployment ─────────────────────────┐
│ ID       9b1deb4d-...               │
│ Status   success                    │
│ Created  2026-07-02 07:20:11        │
│ Branch   main                       │
│ Commit   4f2a91c...                 │
└─────────────────────────────────────┘

Events
  07:20:12 • Build started
  07:21:48 • Image pushed
  07:22:30 • Revision serving traffic
```

Failed deployments include a `Failure` row with the reason.

## `quome deployments create`

Trigger a deployment manually — the CLI equivalent of the dashboard's Deploy button.

```
Usage: quome deployments create [OPTIONS]

Options:
      --branch <BRANCH>  Git branch to deploy (git-sourced apps)
      --app <APP>        Application ID (uses linked app if not provided)
      --org <ORG>        Organization ID (uses linked org if not provided)
      --json             Output as JSON
```

```console
$ quome deployments create
✓ Deployment triggered
  ID      1ee7f2a4-...
  Status  created
```

For git-sourced apps, `--branch` deploys a branch other than the default. For image-sourced apps, it redeploys the configured image (useful after pushing a new build to the same tag).

The command returns immediately; poll `deployments get` or watch `quome logs` for progress. Scripted wait-for-success recipe: [Scripting & CI](../tutorials/scripting-and-ci.md#wait-for-a-deployment-to-finish).
