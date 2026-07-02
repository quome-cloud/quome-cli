# Session: `login`, `logout`, `whoami`

## `quome login`

Authenticate with an API key and store it in `~/.quome/config.json`.

```
Usage: quome login [OPTIONS]

Options:
  -t, --token <TOKEN>  API key (will prompt if not provided)
```

Interactive (recommended locally — the key never lands in shell history):

```console
$ quome login
? API Key: ********
✓ Logged in
  Email    you@example.com
  User ID  a1b2c3d4-...
```

Non-interactive:

```bash
quome login --token qk_AbC123...
```

If you're already logged in, `login` shows the current identity and asks before replacing it. The key is validated against the API before it's saved — a bad key fails here, not on your next command.

> **CI tip:** skip `login` entirely and set `QUOME_TOKEN` — see [Scripting & CI](../tutorials/scripting-and-ci.md).

## `quome logout`

```console
$ quome logout
Success! Logged out successfully.
```

Removes the token from `~/.quome/config.json`. It does **not** revoke the key server-side — use [`quome keys delete`](keys.md) for that.

## `quome whoami`

Show who you're logged in as, plus the linked org/app for the current directory.

```
Usage: quome whoami [OPTIONS]

Options:
      --json  Output as JSON
```

```console
$ quome whoami
┌ Jane Developer ─────────────────────┐
│ ID            a1b2c3d4-...          │
│ Name          Jane Developer        │
│ Email         you@example.com      │
│ Organization  acme                  │
│ Application   my-api                │
└─────────────────────────────────────┘
```

`--json` prints the raw user object (no linked context):

```console
$ quome whoami --json | jq .email
"you@example.com"
```
