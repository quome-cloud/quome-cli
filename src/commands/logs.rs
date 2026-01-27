use clap::Parser;
use colored::Colorize;
use uuid::Uuid;

use crate::api::models::LogLevel;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {
    /// Application ID (uses linked app if not provided)
    #[arg(long)]
    app: Option<Uuid>,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Number of log entries to fetch
    #[arg(short = 'n', long, default_value = "100")]
    limit: u32,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

fn level_color(level: &LogLevel) -> colored::ColoredString {
    match level {
        LogLevel::Debug => "DEBUG".dimmed(),
        LogLevel::Info => "INFO ".blue(),
        LogLevel::Warn => "WARN ".yellow(),
        LogLevel::Error => "ERROR".red(),
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
    let logs = client.get_logs(org_id, app_id, Some(args.limit)).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&logs)?);
    } else {
        if logs.is_empty() {
            println!("No logs found.");
            return Ok(());
        }

        for entry in logs {
            println!(
                "{} {} {}",
                entry
                    .timestamp
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
                    .dimmed(),
                level_color(&entry.level),
                entry.message
            );
        }
    }

    Ok(())
}
