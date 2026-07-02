# Authentication

The CLI authenticates every request with an **org-scoped API key** sent as an `X-API-Key` header. Keys start with `qk_`.

## Getting a key

1. Log in to the [Quome dashboard](https://quome.studio)
2. Organization settings → **API Keys** → **Create API Key**
3. Copy the key immediately — it is shown exactly once

You can also mint keys from the CLI itself (if you're already logged in with an admin-capable key):

```console
$ quome keys create ci-deployer --scopes "read:app write:app" --expires-days 90
✓ Created API key
  ID    e58ed763-...
  Name  ci-deployer
  Key   qk_AbC123...

  Save this key - it won't be shown again!
```

## How the CLI stores and finds your token

Precedence, highest first:

1. `QUOME_TOKEN` environment variable
2. `~/.quome/config.json` (written by `quome login`)

`quome login` validates the key by calling the API before saving it. `quome logout` removes it from the config file (it does not revoke the key — use `quome keys delete` for that).

## Scopes

A key's scopes control what it can do:

- `*` — full access (the default when creating keys)
- Space- or comma-separated grants like `read:secret write:app` — `read` < `write` < `admin`; a `write` grant implies `read`

For CI, create a dedicated key with the narrowest scopes that work and an expiry:

```bash
quome keys create github-actions --scopes "write:app read:secret" --expires-days 90
```

## Keys are org-scoped

An API key belongs to one organization and only works for that org's resources. If you work across multiple orgs, `quome link` per project directory (or `QUOME_ORG`) selects which org a command targets — but the *key* must belong to that org. Logging in with a different org's key is the way to switch. Targeting an org the key doesn't belong to fails with `You don't have access to this organization` (see [Troubleshooting](troubleshooting.md#api-errors)).

## Why `whoami` shows the org owner's email

An org API key is the *organization's* credential, not yours personally — the platform resolves it to the org's owner identity. So after `quome login` with a key someone else's org owner created, `quome whoami` shows the owner's email, even though you generated the key. This is expected. Per-person attribution in the audit trail comes from dashboard sessions; CLI actions via an org key are attributed to the key (visible in `quome events`).

## Security notes

- Keys are stored in plain text in `~/.quome/config.json` — standard practice for CLI tools (same as `~/.aws/credentials`), but treat the file accordingly.
- Prefer `QUOME_TOKEN` injected from your CI provider's secret store over committing anything.
- Rotate: create a new key, verify, then `quome keys delete <old-id>`.
- The CLI never sends your key anywhere except the configured API base URL over HTTPS.
