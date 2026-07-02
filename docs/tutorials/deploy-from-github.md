# Tutorial: Deploy from GitHub

**Goal:** connect a GitHub repo so every push to `main` builds and deploys automatically — plus manual deploys of any branch.
**Time:** ~10 minutes. **You need:** a repo with a `Dockerfile`, and the Quome GitHub App installed on it.

## 0. One-time: install the GitHub App

Quome builds your repo via a GitHub App integration (dashboard → **Integrations** → **GitHub** → install and select your repos). If the app isn't installed on the repo, `apps create --repo` will fail with a clear API error telling you so.

## 1. Create a git-sourced app

```console
$ quome apps create my-api --repo acme/my-api --branch main
✓ Created application
  ID      3d2c1b0a-...
  Name    my-api
  Status  pending
```

That's the entire configuration: repo + branch. Quome clones, builds the `Dockerfile` at the repo root, and deploys.

```console
$ quome apps get -i 3d2c1b0a-...
┌ my-api ─────────────────────────────────────┐
│ ID       3d2c1b0a-...                       │
│ Name     my-api                             │
│ Status   running                            │
│ Source   git                                │
│ Repo     acme/my-api                        │
│ URL      https://my-api-acme.q.run          │
└─────────────────────────────────────────────┘
```

## 2. Deploy on push

Nothing to set up — it's on by default for the configured branch:

```console
$ git commit -am "feat: add /healthz" && git push origin main
# a minute later...

$ quome deployments list --app 3d2c1b0a-...
╭──────────────────────────────────────┬─────────────┬────────┬──────────────────╮
│ ID                                   │ STATUS      │ BRANCH │ CREATED          │
├──────────────────────────────────────┼─────────────┼────────┼──────────────────┤
│ 8a7b6c5d-...                         │ in_progress │ main   │ 2026-07-02 08:02 │
│ 9b1deb4d-...                         │ success     │ main   │ 2026-07-02 07:45 │
╰──────────────────────────────────────┴─────────────┴────────┴──────────────────╯
```

Inspect any build, including its per-stage events:

```console
$ quome deployments get 8a7b6c5d-... --app 3d2c1b0a-...
┌ Deployment ─────────────────────────┐
│ ID       8a7b6c5d-...               │
│ Status   success                    │
│ Branch   main                       │
│ Commit   4f2a91c...                 │
└─────────────────────────────────────┘

Events
  08:02:14 • Build started
  08:03:51 • Image pushed
  08:04:33 • Revision serving traffic
```

## 3. Deploy a different branch manually

Testing a feature branch without merging:

```bash
quome deployments create --app 3d2c1b0a-... --branch feature/new-auth
```

## 4. Change the tracked branch

Cutting over from `main` to `release`:

```bash
quome apps update -i 3d2c1b0a-... --branch release
```

Pushes to `release` now deploy; pushes to `main` don't.

## Troubleshooting

- **Create fails mentioning the GitHub App** → the app isn't installed on that repo, or the repo isn't in its selected-repositories list. Fix in dashboard → Integrations.
- **Build fails** → `quome deployments get <id>` shows the failure reason and build events; `quome logs` shows runtime errors after a successful build.

## Next

- Inject config and credentials → [Manage secrets like a pro](manage-secrets-like-a-pro.md)
- Gate deploys on tests in GitHub Actions → [Scripting & CI](scripting-and-ci.md)
