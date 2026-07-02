use clap::Parser;
use colored::Colorize;
use uuid::Uuid;

use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui;

#[derive(Parser)]
pub struct Args {
    /// Application ID (uses linked app if not provided)
    #[arg(long)]
    app: Option<Uuid>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Number of log entries to fetch
    #[arg(short = 'n', long, default_value = "200")]
    limit: u32,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

fn severity_color(severity: &str) -> colored::ColoredString {
    match severity.to_uppercase().as_str() {
        "DEBUG" => "DEBUG".dimmed(),
        "INFO" | "DEFAULT" | "NOTICE" => "INFO ".blue(),
        "WARNING" | "WARN" => "WARN ".yellow(),
        "ERROR" | "CRITICAL" | "ALERT" | "EMERGENCY" => "ERROR".red(),
        other => other.normal(),
    }
}

pub async fn execute(args: Args) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let app_id = match args.app {
        Some(id) => id,
        None => config.require_linked_app()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching logs...");
    let logs = client.get_logs(org_id, app_id, Some(args.limit)).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&logs)?);
        return Ok(());
    }

    if logs.revisions.is_empty() {
        println!("No logs found.");
        return Ok(());
    }

    // Logs are grouped by Cloud Run revision; print each group as a stream
    for revision in &logs.revisions {
        println!("{}", format!("── {} ──", revision.revision_name).dimmed());
        for entry in &revision.logs {
            let severity = entry.severity.as_deref().unwrap_or("INFO");
            println!(
                "{} {} {}",
                entry
                    .timestamp
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
                    .dimmed(),
                severity_color(severity),
                entry.message
            );
        }
    }

    Ok(())
}
