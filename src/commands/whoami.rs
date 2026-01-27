use clap::Parser;

use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui;

#[derive(Parser)]
pub struct Args {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(args: Args) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching user info...");
    let user = client.get_current_user().await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&user)?);
    } else {
        let mut details = vec![
            ("ID", user.id.to_string()),
            ("Name", user.name.clone()),
            ("Email", user.email.clone()),
        ];

        // Add linked context if any
        if let Some(linked) = config.get_linked()? {
            details.push(("Organization", linked.org_name.clone()));
            if let Some(ref app_name) = linked.app_name {
                details.push(("Application", app_name.clone()));
            }
        }

        let details_ref: Vec<(&str, &str)> = details
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();

        ui::print_detail(&user.name, &details_ref);
    }

    Ok(())
}
