# Quome CLI

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)

## Overview

This is the command line interface for [Quome](https://quome.com). Use it to manage your Quome infrastructure, applications, and deployments directly from the terminal.

The Quome CLI allows you to:

- Authenticate using API keys from the Quome dashboard
- Create and manage organizations
- Deploy and monitor applications
- Manage secrets and environment variables
- View logs and audit events
- Generate and manage API keys

## Installation

### From Source (Cargo)

```bash
cargo install --git https://github.com/quome-cloud/quome-cli.git
```

### Build from Source

```bash
git clone https://github.com/quome-cloud/quome-cli.git
cd quome-cli
cargo build --release
# Binary will be at ./target/release/quome
```

### Homebrew (coming soon)

```bash
brew install quome
```

## Quick Start

```bash
# Login to Quome
quome login

# Link your current directory to an organization and app
quome link

# View your applications
quome apps list

# View application logs
quome logs

# Check who you're logged in as
quome whoami
```

## Authentication

The Quome CLI uses API keys for authentication. You'll need to generate an API key from the Quome web interface before using the CLI.

### Getting Your API Key

1. Log in to the [Quome Dashboard](https://quome.com)
2. Navigate to your organization settings
3. Go to **API Keys** section
4. Click **Create API Key**
5. Copy the generated key (it's only shown once!)

### Interactive Login

```bash
quome login
```

You'll be prompted to enter your API key. The key is stored securely in `~/.quome/config.json`.

### Non-Interactive Login

```bash
quome login --token your-api-key
```

### Using Environment Variables

For CI/CD pipelines, you can use environment variables instead of logging in:

```bash
export QUOME_TOKEN=your-api-key
export QUOME_ORG=your-org-id
export QUOME_APP=your-app-id

quome logs  # Uses environment variables
```

### Logout

```bash
quome logout
```

## Directory Linking

Link your project directory to a Quome organization and application. This allows you to run commands without specifying `--org` and `--app` flags.

```bash
# Interactive selection
quome link

# Direct linking
quome link --org <org-id> --app <app-id>

# Unlink current directory
quome unlink
```

Linking is stored per-directory in `~/.quome/config.json`.

## Commands Reference

### Authentication

#### `quome login`

Authenticate with Quome using an API key generated from the [Quome Dashboard](https://quome.com).

```bash
quome login [OPTIONS]

Options:
  -t, --token <TOKEN>  API key (will prompt if not provided)
```

#### `quome logout`

Log out and clear stored credentials.

```bash
quome logout
```

#### `quome whoami`

Display current user information and linked context.

```bash
quome whoami [OPTIONS]

Options:
      --json  Output as JSON
```

---

### Organizations

#### `quome orgs list`

List all organizations you belong to.

```bash
quome orgs list [OPTIONS]

Options:
      --json  Output as JSON
```

#### `quome orgs create`

Create a new organization.

```bash
quome orgs create <NAME> [OPTIONS]

Arguments:
  <NAME>  Organization name

Options:
      --json  Output as JSON
```

#### `quome orgs get`

Get details of an organization.

```bash
quome orgs get [OPTIONS]

Options:
  -i, --id <ID>  Organization ID (uses linked org if not provided)
      --json     Output as JSON
```

---

### Organization Members

#### `quome members list`

List members of an organization.

```bash
quome members list [OPTIONS]

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

#### `quome members add`

Add a user to an organization.

```bash
quome members add <USER_ID> [OPTIONS]

Arguments:
  <USER_ID>  User ID to add

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

---

### Applications

#### `quome apps list`

List all applications in an organization.

```bash
quome apps list [OPTIONS]

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

#### `quome apps create`

Create a new application.

```bash
quome apps create <NAME> [OPTIONS]

Arguments:
  <NAME>  Application name

Options:
  -d, --description <DESC>  Application description
      --image <IMAGE>       Container image (e.g., nginx:latest)
      --port <PORT>         Container port [default: 80]
      --org <ORG>           Organization ID (uses linked org if not provided)
      --json                Output as JSON
```

#### `quome apps get`

Get application details.

```bash
quome apps get [OPTIONS]

Options:
  -i, --id <ID>   Application ID (uses linked app if not provided)
      --org <ORG> Organization ID (uses linked org if not provided)
      --json      Output as JSON
```

#### `quome apps update`

Update an application.

```bash
quome apps update [OPTIONS]

Options:
  -i, --id <ID>             Application ID (uses linked app if not provided)
      --name <NAME>         New name
      --description <DESC>  New description
      --org <ORG>           Organization ID (uses linked org if not provided)
      --json                Output as JSON
```

#### `quome apps delete`

Delete an application.

```bash
quome apps delete <ID> [OPTIONS]

Arguments:
  <ID>  Application ID

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
  -f, --force      Skip confirmation prompt
```

---

### Deployments

#### `quome deployments list`

List deployments for an application.

```bash
quome deployments list [OPTIONS]

Options:
      --app <APP>  Application ID (uses linked app if not provided)
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

#### `quome deployments get`

Get deployment details including events.

```bash
quome deployments get <ID> [OPTIONS]

Arguments:
  <ID>  Deployment ID

Options:
      --app <APP>  Application ID (uses linked app if not provided)
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

---

### Logs

#### `quome logs`

View application logs.

```bash
quome logs [OPTIONS]

Options:
      --app <APP>      Application ID (uses linked app if not provided)
      --org <ORG>      Organization ID (uses linked org if not provided)
  -n, --limit <LIMIT>  Number of log entries [default: 100]
      --json           Output as JSON
```

---

### Secrets

#### `quome secrets list`

List all secrets in an organization.

```bash
quome secrets list [OPTIONS]

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

#### `quome secrets set`

Create or update a secret.

```bash
quome secrets set <NAME> <VALUE> [OPTIONS]

Arguments:
  <NAME>   Secret name
  <VALUE>  Secret value

Options:
  -d, --description <DESC>  Secret description
      --org <ORG>           Organization ID (uses linked org if not provided)
      --json                Output as JSON
```

#### `quome secrets get`

Get a secret value.

```bash
quome secrets get <NAME> [OPTIONS]

Arguments:
  <NAME>  Secret name

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

#### `quome secrets delete`

Delete a secret.

```bash
quome secrets delete <NAME> [OPTIONS]

Arguments:
  <NAME>  Secret name

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
  -f, --force      Skip confirmation prompt
```

---

### API Keys

#### `quome keys list`

List API keys for an organization.

```bash
quome keys list [OPTIONS]

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

#### `quome keys create`

Create a new API key.

```bash
quome keys create [OPTIONS]

Options:
      --expires-days <DAYS>  Days until expiration (0 = never) [default: 0]
      --org <ORG>            Organization ID (uses linked org if not provided)
      --json                 Output as JSON
```

**Important:** The API key is only displayed once upon creation. Store it securely.

#### `quome keys delete`

Delete an API key.

```bash
quome keys delete <ID> [OPTIONS]

Arguments:
  <ID>  API key ID

Options:
      --org <ORG>  Organization ID (uses linked org if not provided)
  -f, --force      Skip confirmation prompt
```

---

### Events

#### `quome events`

View organization audit events.

```bash
quome events [OPTIONS]

Options:
      --org <ORG>      Organization ID (uses linked org if not provided)
  -n, --limit <LIMIT>  Number of events [default: 50]
      --json           Output as JSON
```

---

## Configuration

### User Configuration

User credentials and linked directories are stored in `~/.quome/config.json`:

```json
{
  "user": {
    "token": "your-api-key",
    "id": "user-uuid",
    "email": "you@example.com"
  },
  "linked": {
    "/path/to/project": {
      "org_id": "org-uuid",
      "org_name": "My Organization",
      "app_id": "app-uuid",
      "app_name": "My App"
    }
  }
}
```

### Settings File

You can customize API endpoints and URLs using a `settings.json` file. The CLI looks for settings in this order:

1. `./settings.json` (local, in current directory)
2. `~/.quome/settings.json` (global)
3. Built-in defaults

Example `settings.json`:

```json
{
  "api_url": "https://demo.quome.cloud",
  "docs_url": "https://documentation.demo.quome.cloud",
  "website_url": "https://quome.com"
}
```

| Setting | Description | Default |
|---------|-------------|---------|
| `api_url` | API base URL | `https://demo.quome.cloud` |
| `docs_url` | Documentation URL | `https://documentation.demo.quome.cloud` |
| `website_url` | Main website URL | `https://quome.com` |

This is useful for:
- Pointing to different environments (staging, production)
- Self-hosted Quome installations
- Local development

## Environment Variables

| Variable | Description |
|----------|-------------|
| `QUOME_TOKEN` | API key (overrides stored key) |
| `QUOME_ORG` | Organization ID (overrides linked org) |
| `QUOME_APP` | Application ID (overrides linked app) |
| `QUOME_API_URL` | API base URL (default: `https://demo.quome.cloud`) |

## JSON Output

All commands support `--json` flag for machine-readable output:

```bash
quome apps list --json | jq '.[] | .name'
```

## Exit Codes

| Code | Description |
|------|-------------|
| 0 | Success |
| 1 | Error (see stderr for details) |

## Development

### Prerequisites

- Rust 1.70 or later
- Cargo

### Building

```bash
cargo build
```

### Running Tests

```bash
cargo test
```

### Running Locally

```bash
cargo run -- login
cargo run -- apps list
```

### Linting

```bash
cargo clippy
cargo fmt
```

## Contributing

We welcome contributions! Please see our contributing guidelines for more information.

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

## Support

- [Documentation](https://documentation.demo.quome.cloud)
- [Issue Tracker](https://github.com/quome-cloud/quome-cli/issues)
