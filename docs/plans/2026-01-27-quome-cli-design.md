# Quome CLI Design

## Overview

A Rust-based CLI for the Quome platform, following patterns from railway-cli-example but adapted for REST API instead of GraphQL.

**Command name:** `quome`
**Language:** Rust
**API:** REST (OpenAPI 3.0)
**Scope:** Full feature parity with Quome API

---

## Project Structure

```
quome-cli/
├── src/
│   ├── main.rs              # Entry point, clap args, command routing
│   ├── client.rs            # REST HTTP client (reqwest)
│   ├── config.rs            # Auth tokens & linked org/app state
│   ├── errors.rs            # Custom error types (thiserror)
│   ├── table.rs             # Formatted table output
│   │
│   ├── commands/            # One module per command group
│   │   ├── mod.rs
│   │   ├── login.rs         # quome login
│   │   ├── logout.rs        # quome logout
│   │   ├── whoami.rs        # quome whoami
│   │   ├── link.rs          # quome link
│   │   ├── unlink.rs        # quome unlink
│   │   ├── orgs.rs          # quome orgs [list|create|get]
│   │   ├── members.rs       # quome members [list|add]
│   │   ├── apps.rs          # quome apps [list|create|get|update|delete]
│   │   ├── deploy.rs        # quome deploy, quome deployments
│   │   ├── logs.rs          # quome logs
│   │   ├── secrets.rs       # quome secrets [list|create|get|update|delete]
│   │   ├── keys.rs          # quome keys [list|create|delete]
│   │   └── events.rs        # quome events
│   │
│   └── api/                 # Typed API client layer
│       ├── mod.rs
│       ├── auth.rs          # Session endpoints
│       ├── users.rs         # User endpoints
│       ├── orgs.rs          # Organization endpoints
│       ├── apps.rs          # App & deployment endpoints
│       ├── secrets.rs       # Secrets endpoints
│       └── models.rs        # Request/response structs from OpenAPI
│
├── Cargo.toml
└── README.md
```

---

## Command Structure

```
quome <command> [subcommand] [options]

AUTHENTICATION:
  quome login                     # Browser-based OAuth or token prompt
  quome logout                    # Revoke session, clear local config
  quome whoami                    # Show current user and linked org/app

ORGANIZATIONS:
  quome orgs list                 # List all orgs you belong to
  quome orgs create <name>        # Create new organization
  quome orgs get [id]             # Get org details (uses linked org if no id)

LINKING:
  quome link                      # Interactive: select org and app to link
  quome unlink                    # Remove linked org/app from current directory

MEMBERS:
  quome members list              # List members in linked org
  quome members add <user_id>     # Add member to linked org

APPLICATIONS:
  quome apps list                 # List apps in linked org
  quome apps create <name>        # Create new app
  quome apps get [app_id]         # Get app details
  quome apps update <app_id>      # Update app config
  quome apps delete <app_id>      # Delete app

DEPLOYMENTS:
  quome deploy                    # Trigger deployment for linked app
  quome deployments list          # List deployments for linked app
  quome deployments get <id>      # Get deployment details
  quome logs                      # View deployment logs

SECRETS:
  quome secrets list              # List secrets in linked org
  quome secrets set <name> <val>  # Create/update secret
  quome secrets get <name>        # Get secret value
  quome secrets delete <name>     # Delete secret

API KEYS:
  quome keys list                 # List API keys for linked org
  quome keys create [--expires]   # Create new API key
  quome keys delete <id>          # Revoke API key

EVENTS:
  quome events                    # List recent org events

GLOBAL FLAGS:
  --json                          # Output as JSON
  --org <id>                      # Override linked org
  --app <id>                      # Override linked app
```

---

## Configuration

**Config file location:** `~/.quome/config.json`

```json
{
  "user": {
    "token": "eyJhbG...",
    "id": "uuid",
    "email": "user@example.com"
  },
  "linked": {
    "/Users/jim/projects/myapp": {
      "org_id": "uuid",
      "org_name": "My Team",
      "app_id": "uuid",
      "app_name": "myapp"
    }
  }
}
```

**Environment variable overrides:**
- `QUOME_TOKEN` - API token (for CI/scripts)
- `QUOME_ORG` - Override linked org
- `QUOME_APP` - Override linked app
- `QUOME_API_URL` - Custom API base URL (default: production)

**Authentication flow:**
1. `quome login` opens browser to Quome login page (or prompts for token in headless mode)
2. User authenticates, receives JWT token
3. Token stored in `~/.quome/config.json`
4. All subsequent requests use `Authorization: Bearer <token>` header

**Token refresh:**
- Before requests, check if token is expired
- If expired, use `POST /api/v1/auth/sessions/renew` to get fresh token
- Update stored token automatically

---

## Dependencies

```toml
[package]
name = "quome-cli"
version = "0.1.0"
edition = "2021"

[dependencies]
# CLI framework
clap = { version = "4.5", features = ["derive"] }

# Async runtime
tokio = { version = "1.48", features = ["full"] }

# HTTP client
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# Error handling
anyhow = "1.0"
thiserror = "2.0"

# Interactive prompts
inquire = "0.7"

# Terminal output
colored = "2.0"

# Utilities
dirs = "5.0"
open = "5.0"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1.0", features = ["serde"] }

[profile.release]
lto = "fat"
opt-level = "z"
panic = "abort"
```

---

## API Client Pattern

```rust
// src/client.rs
pub struct QuomeClient {
    http: reqwest::Client,
    base_url: String,
}

impl QuomeClient {
    pub fn new(token: Option<&str>, base_url: &str) -> Result<Self>;

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T>;
    async fn post<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T>;
    async fn put<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T>;
    async fn delete(&self, path: &str) -> Result<()>;
}
```

---

## Error Handling

```rust
#[derive(Error, Debug)]
pub enum QuomeError {
    #[error("Not logged in. Run `quome login` first.")]
    NotLoggedIn,

    #[error("No linked organization. Run `quome link` to connect.")]
    NoLinkedOrg,

    #[error("No linked application. Run `quome link` to connect.")]
    NoLinkedApp,

    #[error("Unauthorized. Your session may have expired. Run `quome login`.")]
    Unauthorized,

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("API error: {0}")]
    ApiError(String),

    #[error("Rate limited. Please wait and try again.")]
    RateLimited,

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
```

---

## Output Formatting

- **Default:** Human-readable with colors and tables
- **`--json` flag:** Raw JSON for scripting/piping

```rust
pub fn print_apps(apps: Vec<App>, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&apps)?);
    } else {
        println!("{:<36}  {:<20}  {:<20}", "ID", "NAME", "CREATED");
        println!("{}", "-".repeat(78));
        for app in apps {
            println!("{:<36}  {:<20}  {:<20}",
                app.id, app.name, app.created_at.format("%Y-%m-%d %H:%M"));
        }
    }
    Ok(())
}
```

---

## Reference

- **OpenAPI Spec:** `quome_service/openapi.yaml`
- **Pattern Reference:** `railway-cli-example/`
