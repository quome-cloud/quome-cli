# Authentication

The CLI authenticates every request with an **org-scoped API key** sent as an `X-API-Key` header. Keys start with `qk_`.

Since 2026-08 every key is backed by a **service account** in its organization: the platform resolves the key to that service account, never to a person. That has three consequences worth knowing before you read on:

- A key can do everything the org's **resources** allow (apps, deployments, secrets, databases, logs) within its scopes.
- A key can **not administer the organization** — list or create orgs, manage members, mint other keys, or read the audit trail. Those are dashboard-only; the CLI tells you so instead of returning a 401.
- A key belongs to **exactly one** organization. `quome login` learns which one, so you don't need to find the org UUID in a dashboard URL.

## Getting a key

1. Log in to the [Quome dashboard](https://quome.studio)
2. **Settings → API Keys** → **Create key**
3. Pick **Full access** (scope `*`) for a developer or migration key, or "Choose permissions" for a narrowly scoped CI key
4. Copy the key — it is shown exactly once

The API Keys page shows each key's backing service account. A key badged **Legacy (non-functional)** has none and cannot authenticate — delete it and create a new one. (Older dashboards showed that badge wrongly on working keys past the 20th; that was fixed in the 2026-09-03 release.)

## Logging in

**No paste at all:** `quome login --browser` opens the dashboard, you pick the organization and click **Create key**, and the new key is handed straight to the CLI over a loopback callback (a one-time code + PKCE, the same handoff `quome host login` uses). Nothing to copy; the key is named `quome-cli@<your computer>` on the API Keys page so you can find and delete it later. You need the Settings → admin capability in that organization, and TOTP if you have it set up.

```console
$ quome login --browser
Opening your browser to approve a new API key (quome-cli@Jims-MacBook)…
✓ Logged in
  Key              qk_AbC123Xy…
  Organization     acme (0d9f4a3b-…)
  Service account  9c1e…
  Scopes           *
```

**With a key you already have:** `quome login` validates the key against the API (`GET /api-keys/self`) and stores it together with the org, service account, and scopes it resolves to:

```console
$ quome login
? API key: ************************************
✓ Logged in
  Key              qk_AbC123Xy…
  Organization     acme (0d9f4a3b-…)
  Service account  9c1e…
  Scopes           *
```

The prompt shows asterisks as you paste so you can see the key landed. Every other way in, for when there is no terminal to paste into:

| How | Command | Notes |
|---|---|---|
| Browser | `quome login --browser` | Approve in the dashboard; the key never passes through your clipboard |
| Prompt | `quome login` | Masked; never in shell history |
| Pipe | `pbpaste \| quome login` / `cat key.txt \| quome login` | Automatic when stdin isn't a terminal; `--stdin` forces it |
| File | `quome login --token-file ~/.quome-key` | Reads the first line |
| Flag | `quome login --token qk_…` | Ends up in shell history — for scripts only |
| Env | `QUOME_TOKEN=qk_…` | No `login` at all; overrides the stored key. Use in CI |

## How the CLI stores and finds your token

Precedence, highest first:

1. `QUOME_TOKEN` environment variable
2. `~/.quome/config.json` (written by `quome login`)

`quome logout` removes the stored key (it does not revoke it — do that on the dashboard's API Keys page).

## Scopes

A key's scopes control what it can do:

- `*` — full access to the org's resources (the dashboard's **Full access** option)
- Grants like `read:secret write:app` — `read` < `write` < `admin`; a `write` grant implies `read`. Operations on a *specific* resource additionally need a per-resource grant on the key's service account, made from the dashboard.

For CI, create a dedicated key with the narrowest scopes that work and an expiry, from the dashboard (**Settings → API Keys → Create key → Choose permissions**).

## Keys are org-scoped

An API key belongs to one organization and only works for that org's resources. `quome login` records the org, `quome link` uses it, and commands run against it by default even in an unlinked directory. To work in a second org, log in with a key from that org (one org = one key). Passing `--org` for a different org fails immediately with `This API key belongs to organization …`.

## What a key cannot do

`quome orgs`, `quome members`, `quome keys`, and `quome events` are organization administration. They are refused up front:

```console
$ quome keys list
error: Managing API keys is organization administration, which an API key cannot do
(keys act as an org-scoped service account and never as an org admin).
Use the dashboard: https://quome.studio/settings
```

This is deliberate: a leaked CI key can deploy, but it cannot invite members, mint itself new keys, or read the audit log. If you run the CLI against an older or self-hosted control plane that still resolves keys to a user, `QUOME_ALLOW_ADMIN_COMMANDS=1` re-enables those commands.

## Security notes

- Keys are stored in plain text in `~/.quome/config.json` — standard practice for CLI tools (same as `~/.aws/credentials`), but treat the file accordingly.
- Prefer `QUOME_TOKEN` injected from your CI provider's secret store over committing anything.
- Rotate: create a new key on the dashboard, `quome login` with it, verify, then delete the old key there.
- The CLI never sends your key anywhere except the configured API base URL over HTTPS.
