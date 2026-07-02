# Command reference

Every command, every flag. All examples assume you've [logged in](../getting-started.md) and, where relevant, [linked a directory](link.md).

## Global conventions

- **`--json`** — every read/write command supports it; prints the raw API object(s), perfect for `jq`. See [Scripting & CI](../tutorials/scripting-and-ci.md).
- **`--org <UUID>` / `--app <UUID>`** — override the linked context for one invocation. Precedence: flag → `QUOME_ORG`/`QUOME_APP` env → linked directory.
- **`--force` / `-f`** — destructive commands (`delete`) prompt for confirmation unless you pass this.
- **Exit codes** — `0` on success, `1` on any error (message on stderr).

## Commands

| Page | Commands |
|------|----------|
| [Session](session.md) | `login`, `logout`, `whoami` |
| [Link](link.md) | `link`, `unlink` |
| [Orgs](orgs.md) | `orgs list`, `orgs create`, `orgs get` |
| [Members](members.md) | `members list`, `members invite` |
| [Keys](keys.md) | `keys list`, `keys create`, `keys delete` |
| [Apps](apps.md) | `apps list`, `apps create`, `apps get`, `apps update`, `apps delete` |
| [Deployments](deployments.md) | `deployments list`, `deployments get`, `deployments create` |
| [Logs](logs.md) | `logs` |
| [Secrets](secrets.md) | `secrets list`, `secrets set`, `secrets get`, `secrets delete` |
| [Databases](databases.md) | `db list`, `db create`, `db get`, `db update`, `db delete` |
| [Events](events.md) | `events` |
| [Upgrade](upgrade.md) | `upgrade` |
