# Secrets: `secrets list|set|get|delete`

Secrets are encrypted values stored in your org's own cloud secret manager and injected into apps at deploy time. The CLI addresses them **by name**.

## `quome secrets list`

```
Usage: quome secrets list [OPTIONS]

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

```console
$ quome secrets list
╭──────────────┬──────────────────────────────────────┬──────────────────╮
│ NAME         │ ID                                   │ UPDATED          │
├──────────────┼──────────────────────────────────────┼──────────────────┤
│ DATABASE_URL │ 4e8f1c2d-...                         │ 2026-07-01 18:22 │
│ STRIPE_KEY   │ 9a0b1c2d-...                         │ 2026-06-28 10:05 │
╰──────────────┴──────────────────────────────────────┴──────────────────╯
```

Listing never returns values.

## `quome secrets set`

Create **or** update — `set` checks whether the name exists and does the right thing.

```
Usage: quome secrets set [OPTIONS] <NAME> <VALUE>

Arguments:
  <NAME>   Secret name
  <VALUE>  Secret value

Options:
  -d, --description <DESCRIPTION>  Secret description
      --org <ORG>                  Organization ID (uses linked org if not provided)
      --json                       Output as JSON
```

```console
$ quome secrets set DATABASE_URL "postgres://user:pass@host:5432/db"
✓ Created secret
  Name  DATABASE_URL
  ID    4e8f1c2d-...
```

> **Shell history warning:** the value is a command-line argument. For sensitive values, read from a file or variable instead of typing them inline:
>
> ```bash
> quome secrets set STRIPE_KEY "$(cat stripe-key.txt)"
> ```

## `quome secrets get`

Print a secret's decrypted value to stdout (and nothing else — safe to pipe).

```
Usage: quome secrets get [OPTIONS] <NAME>

Arguments:
  <NAME>  Secret name

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

```console
$ quome secrets get DATABASE_URL
postgres://user:pass@host:5432/db

$ export DATABASE_URL=$(quome secrets get DATABASE_URL)
```

Requires a key with read access to that secret; see [Authentication → Scopes](../authentication.md#scopes).

## `quome secrets delete`

```
Usage: quome secrets delete [OPTIONS] <NAME>

Arguments:
  <NAME>  Secret name

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
  -f, --force      Skip confirmation prompt
```

```console
$ quome secrets delete STRIPE_KEY
? Are you sure you want to delete secret 'STRIPE_KEY'? Yes
✓ Deleted secret
  Name  STRIPE_KEY
```

More workflows (bulk .env import patterns, rotation, CI): [Manage secrets like a pro](../tutorials/manage-secrets-like-a-pro.md).
