# Terminal UI Enhancement Design

## Overview

Enhance quome-cli terminal output to feel polished and delightful, similar to Python's Rich library. Replace manual string formatting with proper tables, add spinners for async operations, and use panels for structured output.

## Dependencies

Add to `Cargo.toml`:

```toml
indicatif = "0.17"
tabled = { version = "0.17", features = ["ansi"] }
```

- **indicatif** - Spinners and progress bars (used by cargo, rustup)
- **tabled** - Unicode tables with styling, auto-sizing columns

Keep existing `colored` crate for inline text styling.

## Implementation

### 1. UI Module (`src/ui.rs`)

Create a new module with reusable helpers:

```rust
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use tabled::{Table, Tabled, settings::{Style, Panel, Modify, Color}};
use tabled::settings::object::Rows;

/// Create a spinner for async operations
pub fn spinner(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap()
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    pb
}

/// Print a success panel with key-value details
pub fn print_success(title: &str, details: &[(&str, &str)]) {
    let rows: Vec<[String; 2]> = details
        .iter()
        .map(|(k, v)| [k.to_string(), v.to_string()])
        .collect();

    let table = Table::new(rows)
        .with(Style::rounded())
        .with(Panel::header(format!("{} {}", "✓".green(), title.green().bold())))
        .to_string();

    println!("{}", table);
}

/// Print an error panel
pub fn print_error(title: &str, message: &str) {
    let rows = vec![[message.to_string()]];
    let table = Table::new(rows)
        .with(Style::rounded())
        .with(Panel::header(format!("{} {}", "✗".red(), title.red().bold())))
        .to_string();

    println!("{}", table);
}

/// Print a styled table from any Tabled data
pub fn print_table<T: Tabled>(rows: Vec<T>) {
    if rows.is_empty() {
        return;
    }
    let table = Table::new(rows)
        .with(Style::rounded())
        .with(Modify::new(Rows::first()).with(Color::BOLD))
        .to_string();
    println!("{}", table);
}
```

### 2. Table Row Structs

Define `Tabled` structs for each list command. Example for apps:

```rust
#[derive(Tabled)]
pub struct AppRow {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "NAME")]
    pub name: String,
    #[tabled(rename = "CREATED")]
    pub created: String,
}
```

Similar structs for: `SecretRow`, `DeploymentRow`, `OrgRow`, `KeyRow`, `MemberRow`, `EventRow`, `LogRow`.

### 3. Command Updates

#### List Commands

Before:
```rust
println!("{:<36}  {:<20}  {:<20}", "ID".bold(), "NAME".bold(), "CREATED".bold());
println!("{}", "-".repeat(78));
for app in response.apps {
    println!("{:<36}  {:<20}  {:<20}", app.id, app.name, ...);
}
```

After:
```rust
let sp = ui::spinner("Fetching applications...");
let response = client.list_apps(org_id).await?;
sp.finish_and_clear();

let rows: Vec<AppRow> = response.apps.iter().map(|app| AppRow {
    id: app.id.to_string(),
    name: app.name.clone(),
    created: app.created_at.format("%Y-%m-%d %H:%M").to_string(),
}).collect();

if rows.is_empty() {
    println!("No applications found.");
} else {
    ui::print_table(rows);
}
```

#### Create/Update Commands

Before:
```rust
println!("{} Created application:", "Success!".green().bold());
println!("  {} {}", "ID:".dimmed(), app.id);
println!("  {} {}", "Name:".dimmed(), app.name);
```

After:
```rust
ui::print_success("Created application", &[
    ("ID", &app.id.to_string()),
    ("Name", &app.name),
    ("Created", &app.created_at.format("%Y-%m-%d %H:%M").to_string()),
]);
```

#### Detail Commands (get)

Use panels with the resource name as header:

```rust
let sp = ui::spinner("Fetching application...");
let app = client.get_app(org_id, app_id).await?;
sp.finish_and_clear();

ui::print_detail(&app.name, &[
    ("ID", &app.id.to_string()),
    ("Description", app.description.as_deref().unwrap_or("-")),
    ("Created", &app.created_at.format("%Y-%m-%d %H:%M").to_string()),
    ("Updated", &app.updated_at.format("%Y-%m-%d %H:%M").to_string()),
]);
```

### 4. Files to Modify

| File | Changes |
|------|---------|
| `Cargo.toml` | Add indicatif, tabled dependencies |
| `src/lib.rs` or `src/main.rs` | Add `mod ui;` |
| `src/ui.rs` | New file with helpers |
| `src/commands/apps.rs` | Tables, spinners, panels |
| `src/commands/secrets.rs` | Tables, spinners, panels |
| `src/commands/deployments.rs` | Tables, spinners, panels |
| `src/commands/orgs.rs` | Tables, spinners, panels |
| `src/commands/keys.rs` | Tables, spinners, panels |
| `src/commands/members.rs` | Tables, spinners, panels |
| `src/commands/events.rs` | Tables, spinners |
| `src/commands/logs.rs` | Spinners (keep current log format) |
| `src/commands/login.rs` | Spinner, success panel |
| `src/commands/link.rs` | Spinner, success panel |
| `src/commands/whoami.rs` | Detail panel |

### 5. Visual Examples

**List output:**
```
╭──────────────────────────────────────┬──────────────────┬──────────────────╮
│ ID                                   │ NAME             │ CREATED          │
├──────────────────────────────────────┼──────────────────┼──────────────────┤
│ 3f2a1b4c-5d6e-7f8a-9b0c-1d2e3f4a5b6c │ my-app           │ 2024-01-15 10:30 │
│ 8a7b6c5d-4e3f-2a1b-0c9d-8e7f6a5b4c3d │ another-app      │ 2024-01-14 09:15 │
╰──────────────────────────────────────┴──────────────────┴──────────────────╯
```

**Success panel:**
```
╭───────────────────────────────────────────╮
│ ✓ Created application                     │
├─────────────┬─────────────────────────────┤
│ ID          │ 3f2a1b4c-5d6e-7f8a-9b0c...  │
│ Name        │ my-app                      │
│ Created     │ 2024-01-15 10:30            │
╰─────────────┴─────────────────────────────╯
```

**Spinner (animated):**
```
⠋ Fetching applications...
```

## Notes

- Keep `--json` output unchanged (raw JSON, no styling)
- Logs command keeps current format (timestamp + level + message) but adds spinner
- Empty states remain simple text: "No applications found."
