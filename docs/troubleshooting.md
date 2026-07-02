# Troubleshooting

Every error the CLI prints, what it means, and how to fix it. Errors always go to stderr prefixed with `error:`.

## Auth & context errors

### `error: Not logged in. Run 'quome login' first.`

No token found. Run `quome login`, or set `QUOME_TOKEN` in CI.

### `error: Unauthorized. Your session may have expired. Run 'quome login'.`

The API rejected your key (HTTP 401). Causes, most common first:

1. The key was deleted or expired — check `quome keys list` from a working session or the dashboard, then `quome login` with a fresh key
2. The key belongs to a different org than the one you're targeting
3. You're pointing at the wrong API URL (`echo $QUOME_API_URL`)

### `error: No linked organization. Run 'quome link' to connect.`

The command needs an org and none was found. Fix any of these ways:

```bash
quome link                 # interactive, per-directory
quome apps list --org <uuid>
QUOME_ORG=<uuid> quome apps list
```

### `error: No linked application. Run 'quome link' to connect.`

Same as above but for the app context — `quome logs` and `quome deployments` need one. Re-run `quome link` and pick an app this time, or pass `--app <uuid>`.

## API errors

### `error: Not found: ...`

The resource doesn't exist *in the org you're targeting*. The most common cause is being linked to the wrong org — check with `quome whoami`.

### `error: API error: You don't have access to this organization`

API keys are **org-scoped** — a key only works for the organization it was created in. You're almost certainly targeting a different org than the key's own:

1. `quome whoami` — see which org the current directory is linked to
2. Compare with the org the key was created in (dashboard → that org → API Keys)
3. Fix by relinking (`quome link`), passing `--org <uuid>`, or logging in with a key from the right org

Note that `quome orgs list` can *show* orgs your key can't *act on*: it lists the orgs of the resolved user, while resource commands are gated to the key's own org. One org = one key.

### `error: API error: Authorization service unavailable. Please try again.`

The permission check for the target org couldn't be completed — each organization runs its own isolated authorization service, and the platform couldn't reach this org's. In order of likelihood:

1. You're targeting an org whose infrastructure (data plane) isn't fully provisioned yet — check the org's status in the dashboard
2. Your key belongs to a different org (fix as above — the wrong-org failure can surface as either of these two errors depending on the endpoint)
3. A transient outage of that org's authorization service — retry; if it persists, contact support

### `error: API error: ...` (anything else)

The API returned an error with details — the message after the colon is the server's explanation. Notable ones:

- **Organization creation requires GCP setup** (403 on `orgs create`) — new organizations are provisioned into your own cloud project; complete the GCP setup wizard in the dashboard first.
- **Validation errors** (422) — a field didn't pass validation; the message includes which one. E.g. app names must match `^[a-z0-9][a-z0-9-]*[a-z0-9]$` (lowercase, digits, hyphens, no leading/trailing hyphen).
- **Permission denied** (403) — your key's scopes don't cover the operation. See [Authentication → Scopes](authentication.md#scopes).

### `error: Rate limited. Please wait and try again.`

HTTP 429. Back off for a few seconds; in scripts, retry with exponential backoff.

## Homebrew

### `Error: Refusing to load formula ... from untrusted tap quome-cloud/quome`

Newer Homebrew requires a one-time trust of third-party taps:

```bash
brew trust quome-cloud/quome
brew install quome   # or brew upgrade quome
```

### `brew upgrade` doesn't pick up a new version

```bash
brew update && brew upgrade quome
```

Or just run `quome upgrade`, which does both.

## Seeing what the API actually said

```bash
QUOME_DEBUG=1 quome <command>
```

prints the raw response body to stderr — attach that (minus secrets) when filing an issue.

## Still stuck?

[Open an issue](https://github.com/quome-cloud/quome-cli/issues) with the command, the output of `quome --version`, and the `QUOME_DEBUG=1` output.
