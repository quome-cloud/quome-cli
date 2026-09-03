# Members: `members list|invite`

> **Dashboard-only.** This is organization administration. An API key authenticates as an org-scoped service account and can never hold the org-level permission these commands need, so the CLI refuses them up front (`… is organization administration, which an API key cannot do`). Do this in the dashboard: **Settings → Members**. `QUOME_ALLOW_ADMIN_COMMANDS=1` re-enables the commands against a control plane that still resolves keys to a user.

Membership changes happen through **email invites** — there is intentionally no "add user by ID". The invitee accepts from their dashboard (or signs up first) and appears in `members list` once redeemed.

## `quome members list`

```
Usage: quome members list [OPTIONS]

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

```console
$ quome members list
╭────────────────┬─────────────────────┬────────┬──────────────────╮
│ NAME           │ EMAIL               │ ROLE   │ JOINED           │
├────────────────┼─────────────────────┼────────┼──────────────────┤
│ Jane Developer │ jane@acme.com       │ owner  │ 2026-05-01 09:30 │
│ Sam Reviewer   │ sam@acme.com        │ member │ 2026-06-12 14:02 │
╰────────────────┴─────────────────────┴────────┴──────────────────╯
```

Roles: `owner`, `admin`, `member`.

## `quome members invite`

```
Usage: quome members invite [OPTIONS] <EMAIL>

Arguments:
  <EMAIL>  Email address to invite

Options:
      --role <ROLE>  Role for the invited member (member or admin) [default: member]
      --org <ORG>    Organization ID (uses linked org if not provided)
      --json         Output as JSON
```

```console
$ quome members invite sam@acme.com --role admin
✓ Invited member
  Email    sam@acme.com
  Role     admin
  Expires  2026-07-09 07:14
```

Invites expire; re-run the command to send a fresh one. Changing an existing member's role or removing members is done from the dashboard.
