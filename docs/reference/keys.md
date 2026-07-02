# Keys: `keys list|create|delete`

Org-scoped API keys — the same kind the CLI itself logs in with. Mint scoped, expiring keys for CI instead of reusing your personal one.

## `quome keys list`

```
Usage: quome keys list [OPTIONS]

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

```console
$ quome keys list
╭──────────────────────────────────────┬──────────────┬──────────────┬──────────────────╮
│ ID                                   │ NAME         │ PREFIX       │ CREATED          │
├──────────────────────────────────────┼──────────────┼──────────────┼──────────────────┤
│ e58ed763-928c-4155-bee9-fdbaaadc15f3 │ jane-laptop  │ qk_AbC123Xy  │ 2026-05-01 09:31 │
│ 7f2ac9e1-0b3d-4c5e-9f8a-1b2c3d4e5f6a │ ci-deployer  │ qk_ZyX987Ba  │ 2026-06-20 11:15 │
╰──────────────────────────────────────┴──────────────┴──────────────┴──────────────────╯
```

Only the prefix is stored — full keys are shown once, at creation.

## `quome keys create`

```
Usage: quome keys create [OPTIONS] <NAME>

Arguments:
  <NAME>  Key name

Options:
  -d, --description <DESCRIPTION>    Key description
      --scopes <SCOPES>              Scopes ("*" or space-separated like "read:secret write:app") [default: *]
      --expires-days <EXPIRES_DAYS>  Days until expiration (0 = never expires) [default: 0]
      --org <ORG>                    Organization ID (uses linked org if not provided)
      --json                         Output as JSON
```

```console
$ quome keys create ci-deployer --scopes "write:app read:secret" --expires-days 90
✓ Created API key
  ID    7f2ac9e1-...
  Name  ci-deployer
  Key   qk_ZyX987Ba...

  Save this key - it won't be shown again!
```

Scope grammar: `*` for everything, or grants like `read:secret write:app` where `read` < `write` < `admin` (a `write` grant implies `read`). Details: [Authentication](../authentication.md).

In scripts, capture the key from JSON:

```bash
KEY=$(quome keys create ci-deployer --expires-days 90 --json | jq -r .key)
```

## `quome keys delete`

```
Usage: quome keys delete [OPTIONS] <ID>

Arguments:
  <ID>  API key ID

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
  -f, --force      Skip confirmation prompt
```

```console
$ quome keys delete 7f2ac9e1-0b3d-4c5e-9f8a-1b2c3d4e5f6a
? Are you sure you want to delete API key 7f2ac9e1-...? Yes
✓ Deleted API key
  ID  7f2ac9e1-...
```

Deletion revokes the key immediately — anything still using it starts getting 401s.
