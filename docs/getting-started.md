# Getting started

From zero to a deployed app in about five minutes. Every command below shows the output you should expect (IDs and timestamps will differ).

## 1. Install

**Homebrew (macOS):**

```bash
brew tap quome-cloud/quome
brew trust quome-cloud/quome   # one-time: newer Homebrew requires trusting third-party taps
brew install quome
```

**Cargo (any platform with Rust 1.70+):**

```bash
cargo install --git https://github.com/quome-cloud/quome-cli.git
```

Verify:

```console
$ quome --version
quome 0.2.1
```

## 2. Get an API key

The CLI authenticates with org-scoped API keys (they start with `qk_`). Every key is backed by a service account in one organization.

1. Log in to the [Quome dashboard](https://quome.studio)
2. **Settings → API Keys** → **Create key**
3. Choose **Full access** and copy the key — it's shown only once

More detail (scopes, what a key can and can't do, CI keys): [Authentication](authentication.md).

## 3. Log in

```console
$ quome login
? API key: ************************************
✓ Logged in
  Key              qk_AbC123Xy…
  Organization     acme (0d9f4a3b-…)
  Service account  9c1e6679-…
  Scopes           *
```

`quome login` validates the key against the API, learns which organization it belongs to, and stores both in `~/.quome/config.json`. The prompt masks the key with asterisks so you can see a paste land.

No terminal to paste into? Pipe it (`pbpaste | quome login`), point at a file (`quome login --token-file key.txt`), or set `QUOME_TOKEN` (see [Configuration](configuration.md)).

> A key rejected here is deleted, expired, or badged **Legacy (non-functional)** on the API Keys page — create a new one.

## 4. Link your directory

Your key already names its organization, so commands work right after `login`. `quome link` additionally binds the current directory to an **app**, so app-scoped commands (`logs`, `deployments`) don't need `--app`:

```console
$ quome link
? Select application: (Skip - don't link an app)
✓ Linked
  Organization  acme
```

Check your context any time:

```console
$ quome whoami
┌ API key ─────────────────────────────────────┐
│ Key              qk_AbC123Xy…                │
│ Organization     acme (0d9f4a3b-…)           │
│ Service account  9c1e6679-…                  │
│ Scopes           *                           │
│ Linked org       acme                        │
└──────────────────────────────────────────────┘
```

## 5. Deploy your first app

Create an app from any public container image:

```console
$ quome apps create hello --image nginx:1.27 --port 80
✓ Created application
  ID      7c9e6679-7425-40de-944b-e07fc1f90ae7
  Name    hello
  Status  pending
```

Quome provisions the app in your organization's isolated cloud project. Watch it come up:

```console
$ quome apps list
╭──────────────────────────────────────┬───────┬─────────┬──────────────────────────┬──────────────────╮
│ ID                                   │ NAME  │ STATUS  │ URL                      │ CREATED          │
├──────────────────────────────────────┼───────┼─────────┼──────────────────────────┼──────────────────┤
│ 7c9e6679-7425-40de-944b-e07fc1f90ae7 │ hello │ running │ https://hello-acme.q.run │ 2026-07-02 07:14 │
╰──────────────────────────────────────┴───────┴─────────┴──────────────────────────┴──────────────────╯
```

When `STATUS` is `running`, open the URL. That's a deployed app.

## 6. Look around

```bash
quome logs --app <app-id>          # application logs, grouped by revision
quome deployments list --app <id>  # deployment history
quome events                       # org audit trail
quome secrets set API_KEY hunter2  # your first secret
```

## Where to go next

- [Deploy your first app](tutorials/deploy-your-first-app.md) — the tutorial version of step 5, with more depth
- [Deploy from GitHub](tutorials/deploy-from-github.md) — connect a repo and deploy on push
- [Scripting & CI](tutorials/scripting-and-ci.md) — automate everything with `--json`
- [Command reference](reference/README.md) — every command, every flag
