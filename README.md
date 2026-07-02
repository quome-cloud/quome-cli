# Quome CLI

Command line interface for the [Quome](https://quome.studio) platform.

## What you can do

- Authenticate with an API key from the Quome dashboard
- Manage organizations, members (via invites), and API keys
- Create, inspect, and delete applications (image- or git-sourced)
- Trigger and inspect deployments
- View application logs (grouped by Cloud Run revision)
- Manage secrets and databases (DBaaS)
- View organization audit events

## Installation

### Homebrew (Recommended)

```bash
brew tap quome-cloud/quome
brew install quome
```

### From Source (Cargo)

```bash
cargo install --git https://github.com/quome-cloud/quome-cli.git
```

### Build from Source

```bash
git clone https://github.com/quome-cloud/quome-cli.git
cd quome-cli
cargo build --release
# Binary at ./target/release/quome
```

## Quick start

```bash
# Login with an API key (qk_...) from the Quome dashboard
quome login

# Link your current directory to an organization and app
quome link

# View your applications
quome apps list

# Trigger a deployment
quome deployments create

# Tail recent logs
quome logs

# Check who you're logged in as
quome whoami
```

## Authentication

The CLI authenticates with org-scoped API keys (`qk_...`), sent as an `X-API-Key` header.

1. Log in to the Quome dashboard
2. Organization settings → **API Keys** → **Create API Key**
3. Copy the key (shown only once) and run `quome login`

The token is stored in `~/.quome/config.json`. `QUOME_TOKEN` overrides it for CI use.

## Commands

| Command | Description |
|---------|-------------|
| `quome login` / `logout` / `whoami` | Session management |
| `quome link` / `unlink` | Bind the current directory to an org + app |
| `quome orgs list\|create\|get` | Organizations |
| `quome members list\|invite` | Org members (adds happen via email invites) |
| `quome keys list\|create\|delete` | Org API keys |
| `quome apps list\|create\|get\|update\|delete` | Applications |
| `quome deployments list\|get\|create` | Deployments (`create` triggers a deploy) |
| `quome logs` | Application logs |
| `quome secrets list\|set\|get\|delete` | Secrets |
| `quome db list\|create\|get\|update\|delete` | Managed Postgres (DBaaS) |
| `quome events` | Organization audit events |
| `quome upgrade` | Upgrade to the latest version via Homebrew |

Every command accepts `--json` for machine-readable output, and `--org` / `--app` to override the linked context. `QUOME_ORG` / `QUOME_APP` env vars work too.

## Configuration

- `~/.quome/config.json` — token + per-directory org/app links
- `~/.quome/settings.json` or `./settings.json` — `api_url` override
- `QUOME_API_URL` — env var override for the API base URL (default `https://quome.studio`)
- `QUOME_DEBUG=1` — print raw API responses

## Development

```bash
./scripts/setup.sh    # install git hooks
cargo build
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo test
```

CI runs fmt, clippy, build, and tests on every push and PR. Every push to `main` also auto-bumps the version, builds macOS binaries, publishes a GitHub release, and updates the Homebrew formula.
