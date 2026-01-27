use clap::Parser;
use colored::Colorize;
use uuid::Uuid;

use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Number of events to fetch
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
    let response = client.list_events(org_id, Some(args.limit)).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.events)?);
    } else {
        if response.events.is_empty() {
            println!("No events found.");
            return Ok(());
        }

        for event in response.events {
            let id_string = event.resource.id.to_string();
            let resource_name = event.resource.name.as_deref().unwrap_or(&id_string);

            println!(
                "{} {} {} {} on {} {}",
                event
                    .created_at
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
                    .dimmed(),
                event.actor.email.cyan(),
                event.event_type.yellow(),
                event.resource.resource_type.dimmed(),
                resource_name.bold(),
                format!("({})", event.resource.id).dimmed()
            );
        }
    }

    Ok(())
}
