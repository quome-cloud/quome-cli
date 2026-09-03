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

    let sp = ui::spinner("Resolving key...");
    let identity = client.get_api_key_self().await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&identity)?);
        return Ok(());
    }

    let prefix: String = token.chars().take(12).collect();
    let org = match (&identity.org_name, identity.org_id) {
        (Some(name), id) => format!("{} ({})", name, id),
        (None, id) => id.to_string(),
    };
    let scopes = if identity.scopes.is_empty() {
        "(none)".to_string()
    } else {
        identity.scopes.join(" ")
    };
    let mut details = vec![
        ("Key", format!("{}…", prefix)),
        ("Organization", org),
        ("Service account", identity.service_account_id.to_string()),
        ("Scopes", scopes),
    ];

    if let Some(linked) = config.get_linked()? {
        details.push(("Linked org", linked.org_name.clone()));
        if let Some(ref app_name) = linked.app_name {
            details.push(("Linked app", app_name.clone()));
        }
    }

    let details_ref: Vec<(&str, &str)> = details.iter().map(|(k, v)| (*k, v.as_str())).collect();
    ui::print_detail("API key", &details_ref);
    Ok(())
}
