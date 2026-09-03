# Session: `login`, `logout`, `whoami`

## `quome login`

Authenticate with an API key, resolve the organization and service account behind it, and store all of it in `~/.quome/config.json`.

```
Usage: quome login [OPTIONS]

Options:
  -t, --token <TOKEN>       API key (qk_...). Lands in shell history — prefer the prompt, --token-file, or stdin.
      --token-file <PATH>   Read the API key from a file (first line; the file is not deleted).
      --stdin               Read the API key from stdin (automatic when stdin is not a terminal, e.g. a pipe).
      --browser             Approve a new key in your browser instead of pasting one (nothing to copy).
```

Browser (nothing to paste — the dashboard creates the key and hands it to the CLI over loopback):

```console
$ quome login --browser
Opening your browser to approve a new API key (quome-cli@Jims-MacBook)…
If nothing opened, use this URL:
  https://quome.studio/cli/authorize?kind=api_key&state=…&port=51234&code_challenge=…&key_name=quome-cli%40Jims-MacBook

✓ Logged in
  Key              qk_AbC123Xy…
  Organization     acme (0d9f4a3b-…)
  Service account  9c1e6679-…
  Scopes           *
```

You pick the organization in the browser (Settings → admin capability required there; TOTP if enrolled) and click **Create key**. The key is minted only when the CLI redeems the one-time code, so an approval you abandon creates nothing. It appears on the dashboard's API Keys page as `quome-cli@<hostname>`. If the control plane doesn't offer browser login, the CLI says so and falls back to the paste path.

Interactive (recommended locally — the key never lands in shell history; asterisks show the paste landing):

```console
$ quome login
? API key: ************************************
✓ Logged in
  Key              qk_AbC123Xy…
  Organization     acme (0d9f4a3b-…)
  Service account  9c1e6679-…
  Scopes           *
```

Without a paste-able terminal:

```bash
pbpaste | quome login                 # macOS clipboard → stdin
quome login --token-file ~/.quome-key # first line of a file
quome login --token qk_AbC123...      # scripts only: shell history
```

If you're already logged in, an interactive `login` shows the current key and asks before replacing it; scripted logins replace it silently. The key is validated against the API (`GET /api-keys/self`) before it's saved — a bad key fails here, with the reason, not on your next command. A rejected key is deleted, expired, or badged **Legacy (non-functional)** on the dashboard's API Keys page.

Logins made by CLI versions before 0.2.6 stored a user id/email instead of the org; they still work, but run `quome login` again to record the org (that is what lets commands run without `quome link`).

> **CI tip:** skip `login` entirely and set `QUOME_TOKEN` — see [Scripting & CI](../tutorials/scripting-and-ci.md).

## `quome logout`

```console
$ quome logout
Success! Logged out successfully.
```

Removes the token from `~/.quome/config.json`. It does **not** revoke the key server-side — use [`quome keys delete`](keys.md) for that.

## `quome whoami`

What the current key resolves to, plus the linked context of the current directory.

```
Usage: quome whoami [OPTIONS]

Options:
      --json  Output as JSON
```

```console
$ quome whoami
┌ API key ─────────────────────────────────────┐
│ Key              qk_AbC123Xy…                │
│ Organization     acme (0d9f4a3b-…)           │
│ Service account  9c1e6679-…                  │
│ Scopes           *                           │
│ Linked org       acme                        │
│ Linked app       my-api                      │
└──────────────────────────────────────────────┘
```

`--json` prints the raw `GET /api-keys/self` response (`org_id`, `service_account_id`, `scopes`, and on newer control planes `org_name` / `org_slug`).
