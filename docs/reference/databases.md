# Databases: `db list|create|get|update|delete`

Managed PostgreSQL instances (DBaaS) provisioned inside your org's cloud project — private IP, backups, optional HA.

## `quome db list`

```
Usage: quome db list [OPTIONS]

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

```console
$ quome db list
╭──────────────────────────────────────┬─────────┬─────────┬─────────────┬─────────┬──────────────────╮
│ ID                                   │ NAME    │ VERSION │ TIER        │ STATUS  │ CREATED          │
├──────────────────────────────────────┼─────────┼─────────┼─────────────┼─────────┼──────────────────┤
│ 6ba7b810-9dad-11d1-80b4-00c04fd430c8 │ main-db │ PG 17   │ db-f1-micro │ running │ 2026-06-15 12:00 │
╰──────────────────────────────────────┴─────────┴─────────┴─────────────┴─────────┴──────────────────╯
```

Statuses: `pending` → `provisioning` → `running`, plus `updating`, `stopped`, `failed`, `deleting`.

## `quome db create`

```
Usage: quome db create [OPTIONS] <NAME>

Arguments:
  <NAME>  Database name

Options:
      --description <DESCRIPTION>  Database description
      --version <VERSION>          PostgreSQL major version [default: 17]
      --tier <TIER>                Instance tier (e.g., db-f1-micro) [default: db-f1-micro]
      --storage-gb <STORAGE_GB>    Storage in GB [default: 10]
      --ha                         Enable high availability
      --org <ORG>                  Organization ID (uses linked org if not provided)
      --json                       Output as JSON
```

```console
$ quome db create main-db --version 17 --storage-gb 20
✓ Created database
  ID      6ba7b810-...
  Name    main-db
  Status  pending
```

Provisioning a Postgres instance takes several minutes — watch with `quome db get <id>` until `running`.

## `quome db get`

```
Usage: quome db get [OPTIONS] <ID>

Arguments:
  <ID>  Database ID

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

```console
$ quome db get 6ba7b810-9dad-11d1-80b4-00c04fd430c8
┌ main-db ────────────────────────────┐
│ ID          6ba7b810-...            │
│ Name        main-db                 │
│ Status      running                 │
│ PostgreSQL  v17                     │
│ Tier        db-f1-micro             │
│ Storage     20 GB                   │
│ HA          false                   │
│ Private IP  10.12.0.5               │
│ Created     2026-06-15 12:00:41     │
└─────────────────────────────────────┘
```

Connection credentials are retrieved from the dashboard (they're gated by org policy, not exposed via the CLI yet).

## `quome db update`

```
Usage: quome db update [OPTIONS] <ID>

Arguments:
  <ID>  Database ID

Options:
      --description <DESCRIPTION>  New description
      --tier <TIER>                New instance tier
      --storage-gb <STORAGE_GB>    New storage in GB
      --ha <HA>                    Enable or disable high availability [possible values: true, false]
      --org <ORG>                  Organization ID (uses linked org if not provided)
      --json                       Output as JSON
```

```bash
quome db update 6ba7b810-... --tier db-custom-2-8192 --ha true
```

Tier and HA changes cause a maintenance operation (status `updating`); storage can only grow.

## `quome db delete`

```
Usage: quome db delete [OPTIONS] <ID>

Arguments:
  <ID>  Database ID

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
  -f, --force      Skip confirmation prompt
```

```console
$ quome db delete 6ba7b810-9dad-11d1-80b4-00c04fd430c8
? Are you sure you want to delete database 6ba7b810-...? Yes
✓ Deleted database
  ID  6ba7b810-...
```

**This destroys the instance and its data.** There is no CLI undelete — make sure backups exist first.
