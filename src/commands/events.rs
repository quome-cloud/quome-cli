use clap::Parser;
use uuid::Uuid;

use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui::{self, EventRow};

#[derive(Parser)]
pub struct Args {
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Number of events to fetch (max 100)
    #[arg(short = 'n', long, default_value = "50")]
    limit: u32,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(args: Args) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching audit events...");
    let response = client.list_audit_logs(org_id, Some(args.limit)).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.items)?);
    } else {
        if response.items.is_empty() {
            println!("No events found.");
            return Ok(());
        }

        let rows: Vec<EventRow> = response
            .items
            .iter()
            .map(|event| {
                let resource = match (&event.resource_type, &event.resource_id) {
                    (Some(rt), Some(rid)) => format!("{} ({})", rid, rt),
                    (Some(rt), None) => rt.clone(),
                    _ => "-".to_string(),
                };
                EventRow {
                    time: event.created_at.format("%Y-%m-%d %H:%M").to_string(),
                    action: event.action.clone(),
                    resource,
                }
            })
            .collect();

        ui::print_table(rows);
    }

    Ok(())
}
