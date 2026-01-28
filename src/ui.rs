use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;
use tabled::settings::object::Rows;
use tabled::settings::disable::Remove;
use tabled::settings::{Alignment, Color, Modify, Panel, Style};
use tabled::{Table, Tabled};

/// Create a spinner for async operations
pub fn spinner(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap(),
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
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

/// Print a success panel with key-value details
pub fn print_success(title: &str, details: &[(&str, &str)]) {
    let header = format!("{} {}", "✓".green(), title.green().bold());
    print_panel(&header, details);
}

/// Print a detail panel with key-value details
pub fn print_detail(title: &str, details: &[(&str, &str)]) {
    print_panel(&title.bold().to_string(), details);
}

fn print_panel(header: &str, details: &[(&str, &str)]) {
    if details.is_empty() {
        println!("{}", header);
        return;
    }

    // Build rows as simple strings
    let rows: Vec<[String; 2]> = details
        .iter()
        .map(|(k, v)| [k.to_string(), v.to_string()])
        .collect();

    let table = Table::new(rows)
        .with(Style::rounded())
        .with(Remove::row(Rows::first())) // Remove the auto-generated "0", "1" header
        .with(Panel::header(header))
        .with(Modify::new(Rows::first()).with(Alignment::left()))
        .to_string();

    println!("{}", table);
}

// ============ Table Row Types ============

#[derive(Tabled)]
pub struct AppRow {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "NAME")]
    pub name: String,
    #[tabled(rename = "CREATED")]
    pub created: String,
}

#[derive(Tabled)]
pub struct OrgRow {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "NAME")]
    pub name: String,
    #[tabled(rename = "CREATED")]
    pub created: String,
}

#[derive(Tabled)]
pub struct SecretRow {
    #[tabled(rename = "NAME")]
    pub name: String,
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "UPDATED")]
    pub updated: String,
}

#[derive(Tabled)]
pub struct DeploymentRow {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "STATUS")]
    pub status: String,
    #[tabled(rename = "CREATED")]
    pub created: String,
}

#[derive(Tabled)]
pub struct KeyRow {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "CREATED")]
    pub created: String,
}

#[derive(Tabled)]
pub struct MemberRow {
    #[tabled(rename = "USER ID")]
    pub user_id: String,
    #[tabled(rename = "MEMBER ID")]
    pub member_id: String,
    #[tabled(rename = "JOINED")]
    pub joined: String,
}

#[derive(Tabled)]
pub struct EventRow {
    #[tabled(rename = "TIME")]
    pub time: String,
    #[tabled(rename = "TYPE")]
    pub event_type: String,
    #[tabled(rename = "ACTOR")]
    pub actor: String,
    #[tabled(rename = "RESOURCE")]
    pub resource: String,
}

#[derive(Tabled)]
pub struct DatabaseRow {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(rename = "NAME")]
    pub name: String,
    #[tabled(rename = "VERSION")]
    pub version: String,
    #[tabled(rename = "STATUS")]
    pub status: String,
    #[tabled(rename = "CREATED")]
    pub created: String,
}
