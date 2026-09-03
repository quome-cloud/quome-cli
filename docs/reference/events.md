# Events: `events`

> **Dashboard-only.** This is organization administration. An API key authenticates as an org-scoped service account and can never hold the org-level permission these commands need, so the CLI refuses them up front (`… is organization administration, which an API key cannot do`). Do this in the dashboard: **Settings → Security → Audit**. `QUOME_ALLOW_ADMIN_COMMANDS=1` re-enables the commands against a control plane that still resolves keys to a user.

The organization audit trail — who did what, when. Useful for compliance reviews and "what changed?" debugging.

```
Usage: quome events [OPTIONS]

Options:
      --org <ORG>      Organization ID (uses linked org if not provided)
  -n, --limit <LIMIT>  Number of events to fetch (max 100) [default: 50]
      --json           Output as JSON
```

```console
$ quome events -n 5
╭──────────────────┬────────────────┬───────────────────────────────╮
│ TIME             │ ACTION         │ RESOURCE                      │
├──────────────────┼────────────────┼───────────────────────────────┤
│ 2026-07-02 07:20 │ app.deployed   │ 7c9e6679-... (app)            │
│ 2026-07-02 07:14 │ app.created    │ 7c9e6679-... (app)            │
│ 2026-07-01 18:22 │ secret.updated │ 4e8f1c2d-... (secret)         │
│ 2026-07-01 09:03 │ apikey.created │ 7f2ac9e1-... (api_key)        │
│ 2026-06-30 16:40 │ member.invited │ sam@acme.com (invite)         │
╰──────────────────┴────────────────┴───────────────────────────────╯
```

Viewing the audit trail requires an admin or owner role in the organization.

## JSON mode

`--json` includes the full event objects — actor, IP address, and structured details:

```bash
# All secret-related events
quome events -n 100 --json | jq '.[] | select(.action | startswith("secret."))'

# Actions by count
quome events -n 100 --json | jq -r '.[].action' | sort | uniq -c | sort -rn
```
