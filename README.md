# Quome CLI

**Deploy apps, manage secrets, and ship to HIPAA-ready infrastructure — without leaving your terminal.**

[![CI](https://github.com/quome-cloud/quome-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/quome-cloud/quome-cli/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/quome-cloud/quome-cli)](https://github.com/quome-cloud/quome-cli/releases/latest)
[![Homebrew](https://img.shields.io/badge/homebrew-quome--cloud%2Fquome-orange)](https://github.com/quome-cloud/homebrew-quome)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`quome` is the command line interface for [Quome](https://quome.studio), a serverless platform for teams that need production infrastructure — apps, Postgres, secrets, audit logs — inside their own isolated cloud project.

<!-- TODO: replace with a VHS-recorded demo GIF (https://github.com/charmbracelet/vhs) -->

```console
$ quome apps create hello-api --image nginx:1.27 --port 80
✓ Created application
  ID      7c9e6679-7425-40de-944b-e07fc1f90ae7
  Name    hello-api
  Status  pending

$ quome logs
── hello-api-00001-abc ──
2026-07-02 07:14:02 INFO  Server listening on port 80
```

## Install

```bash
brew tap quome-cloud/quome
brew trust quome-cloud/quome   # newer Homebrew requires trusting third-party taps once
brew install quome
```

<details>
<summary>Other install methods (Cargo, from source)</summary>

```bash
# Cargo
cargo install --git https://github.com/quome-cloud/quome-cli.git

# From source
git clone https://github.com/quome-cloud/quome-cli.git
cd quome-cli
cargo build --release   # binary at ./target/release/quome
```

</details>

## 60-second quickstart

```bash
# 1. Authenticate with an API key (qk_...) from the Quome dashboard
quome login

# 2. Link this directory to your organization and app — every command
#    after this knows your context, no flags needed
quome link

# 3. Ship something
quome apps create my-api --image ghcr.io/acme/my-api:latest
quome secrets set DATABASE_URL "postgres://..."
quome deployments create

# 4. Watch it run
quome logs
```

Full walkthrough with expected output: **[Getting started](docs/getting-started.md)**.

## Why quome CLI

- **Context-aware.** `quome link` binds a directory to an org + app once; after that, `quome logs` just works — no `--org`/`--app` flag soup. Override any time with flags or env vars.
- **Scriptable everywhere.** Every command takes `--json`. Pair with `jq`, drop `QUOME_TOKEN` into CI, and your deploy pipeline is three lines. See [Scripting & CI](docs/tutorials/scripting-and-ci.md).
- **Fast and dependency-free.** A single static Rust binary. No runtime, no Python env, no Node.

## Commands

| Command | What it does | Docs |
|---------|--------------|------|
| `quome login` / `logout` / `whoami` | Authenticate with your API key | [Session](docs/reference/session.md) |
| `quome link` / `unlink` | Bind the current directory to an org + app | [Link](docs/reference/link.md) |
| `quome apps …` | Create, inspect, update, delete applications | [Apps](docs/reference/apps.md) |
| `quome deployments …` | Trigger and inspect deployments | [Deployments](docs/reference/deployments.md) |
| `quome logs` | View application logs by revision | [Logs](docs/reference/logs.md) |
| `quome secrets …` | Manage encrypted secrets | [Secrets](docs/reference/secrets.md) |
| `quome db …` | Managed Postgres (DBaaS) | [Databases](docs/reference/databases.md) |
| `quome orgs …` | Organizations | [Orgs](docs/reference/orgs.md) |
| `quome members …` | Members and invites | [Members](docs/reference/members.md) |
| `quome keys …` | API keys | [Keys](docs/reference/keys.md) |
| `quome events` | Organization audit trail | [Events](docs/reference/events.md) |
| `quome upgrade` | Self-update via Homebrew | [Upgrade](docs/reference/upgrade.md) |

## Documentation

**Guides**

- [Getting started](docs/getting-started.md) — install → login → link → first deploy, end to end
- [Authentication](docs/authentication.md) — API keys, scopes, CI tokens
- [Configuration](docs/configuration.md) — config files, env vars, precedence
- [Troubleshooting](docs/troubleshooting.md) — every error message, decoded

**Tutorials**

- [Deploy your first app](docs/tutorials/deploy-your-first-app.md) — container image → live URL in 5 minutes
- [Deploy from GitHub](docs/tutorials/deploy-from-github.md) — connect a repo, deploy on push
- [Manage secrets like a pro](docs/tutorials/manage-secrets-like-a-pro.md) — secrets workflows for teams
- [Scripting & CI](docs/tutorials/scripting-and-ci.md) — `--json`, `jq` recipes, GitHub Actions

**Reference** — [all commands, all flags](docs/reference/README.md)

## Contributing

Bug reports and PRs welcome — see [CONTRIBUTING.md](CONTRIBUTING.md). The short version:

```bash
./scripts/setup.sh    # install git hooks
cargo build && cargo test
cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

Every push to `main` auto-publishes a release and updates the Homebrew formula.

## License

MIT — see [LICENSE](LICENSE).

---

If `quome` saves you time, a ⭐ helps other people find it.
