# Orgs: `orgs list|create|get`

## `quome orgs list`

```
Usage: quome orgs list [OPTIONS]

Options:
      --json  Output as JSON
```

```console
$ quome orgs list
╭──────────────────────────────────────┬──────┬──────┬──────────────────╮
│ ID                                   │ NAME │ SLUG │ CREATED          │
├──────────────────────────────────────┼──────┼──────┼──────────────────┤
│ 0d9f4a3b-1c2d-4e5f-8a9b-0c1d2e3f4a5b │ acme │ acme │ 2026-05-01 09:30 │
╰──────────────────────────────────────┴──────┴──────┴──────────────────╯
```

## `quome orgs create`

```
Usage: quome orgs create [OPTIONS] <NAME>

Arguments:
  <NAME>  Organization name

Options:
      --slug <SLUG>                URL-safe slug (derived from name if not provided)
  -d, --description <DESCRIPTION>  Organization description
      --json                       Output as JSON
```

```console
$ quome orgs create "Acme Labs" --description "R&D org"
✓ Created organization
  ID    3f8e...
  Name  Acme Labs
  Slug  acme-labs
```

The slug is derived from the name (`"Acme Labs"` → `acme-labs`) unless you pass `--slug`.

> **Note:** new organizations are provisioned into your own cloud project. If your account hasn't completed the GCP setup wizard, the API returns a 403 explaining what to do — finish setup in the [dashboard](https://quome.studio) first.

## `quome orgs get`

```
Usage: quome orgs get [OPTIONS]

Options:
  -i, --id <ID>  Organization ID (uses linked org if not provided)
      --json     Output as JSON
```

```console
$ quome orgs get
┌ acme ───────────────────────────────┐
│ ID               0d9f4a3b-...       │
│ Name             acme               │
│ Slug             acme               │
│ Cloud            gcp                │
│ Cloud connected  true               │
│ Created          2026-05-01 09:30:12│
└─────────────────────────────────────┘
```
