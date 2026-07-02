# Tutorial: Manage secrets like a pro

**Goal:** clean secrets workflows — creating, reading, rotating, and migrating from `.env` files — without values leaking into shell history or logs.
**You need:** the CLI logged in and linked ([getting started](../getting-started.md)).

Secrets live in your organization's own cloud secret manager, encrypted at rest, and are injected into apps at deploy time. The CLI addresses them by name.

## The basics

```console
$ quome secrets set DATABASE_URL "postgres://user:pass@host:5432/db"
✓ Created secret
  Name  DATABASE_URL
  ID    4e8f1c2d-...

$ quome secrets list
╭──────────────┬──────────────┬──────────────────╮
│ NAME         │ ID           │ UPDATED          │
├──────────────┼──────────────┼──────────────────┤
│ DATABASE_URL │ 4e8f1c2d-... │ 2026-07-02 08:10 │
╰──────────────┴──────────────┴──────────────────╯

$ quome secrets get DATABASE_URL
postgres://user:pass@host:5432/db
```

`set` is create-or-update — run it again with a new value and it updates in place. `get` prints the bare value to stdout, nothing else, so it composes:

```bash
export DATABASE_URL=$(quome secrets get DATABASE_URL)
psql "$(quome secrets get DATABASE_URL)"
```

## Keep values out of shell history

A value typed inline is in your history file forever. Three better patterns:

```bash
# From a file
quome secrets set TLS_CERT "$(cat cert.pem)"

# From a password manager
quome secrets set STRIPE_KEY "$(op read 'op://prod/stripe/key')"

# From a prompt (nothing on the command line at all)
read -s VALUE && quome secrets set STRIPE_KEY "$VALUE" && unset VALUE
```

## Migrate a `.env` file

Turn an existing `.env` into Quome secrets in one loop:

```bash
while IFS='=' read -r key value; do
  [[ "$key" =~ ^#.*$ || -z "$key" ]] && continue
  quome secrets set "$key" "$value"
done < .env
```

And export the org's secrets back into a local `.env` (for local dev against real config):

```bash
quome secrets list --json | jq -r '.[].name' | while read -r name; do
  echo "$name=$(quome secrets get "$name")"
done > .env.quome
```

## Rotation

Rotation is just `set` with a new value, then a redeploy so running apps pick it up:

```bash
quome secrets set DATABASE_URL "$NEW_URL"
quome deployments create   # running apps get the new value on the next revision
```

## Least-privilege keys for automation

Don't let a log-shipping script hold a key that can delete apps. Mint scoped keys:

```bash
# Read-only on secrets, nothing else
quome keys create secret-reader --scopes "read:secret" --expires-days 30
```

Anything using that key can `secrets get` but not `set`, `delete`, or touch other resources. Scope grammar: [Authentication](../authentication.md#scopes).

## Audit who touched what

Every secret operation lands in the org audit trail:

```bash
quome events -n 100 --json | jq '.[] | select(.action | startswith("secret."))'
```

## Rules of thumb

1. One name per environment concern (`DATABASE_URL`, not `PROD_STUFF_JSON`)
2. Values via `$(cat ...)` or `read -s` — never typed inline
3. Scoped, expiring keys for anything automated
4. Rotate by `set` + redeploy; verify with `quome events`
