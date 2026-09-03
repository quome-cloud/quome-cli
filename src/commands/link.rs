use clap::Parser;
use inquire::Select;
use uuid::Uuid;

use crate::client::QuomeClient;
use crate::config::{Config, LinkedContext};
use crate::errors::Result;
use crate::ui;

#[derive(Parser)]
pub struct Args {
    /// Organization ID (skips interactive selection)
    #[arg(long)]
    org: Option<String>,

    /// Application ID (skips interactive selection)
    #[arg(long)]
    app: Option<String>,
}

pub async fn execute(args: Args) -> Result<()> {
    let mut config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;

    // The org is decided by the key: an API key belongs to exactly one
    // organization and cannot list or read others, so there is nothing to
    // select. `--org` is accepted for scripts but must name that same org.
    let sp = ui::spinner("Resolving key...");
    let identity = client.get_api_key_self().await?;
    sp.finish_and_clear();

    let requested: Option<uuid::Uuid> = match args.org {
        Some(ref org_str) => Some(
            org_str
                .parse()
                .map_err(|_| crate::errors::QuomeError::Usage("Invalid organization ID".into()))?,
        ),
        None => None,
    };
    let org_id = link_org(identity.org_id, requested)?;
    let org_name = identity
        .org_name
        .clone()
        .unwrap_or_else(|| short_id(&org_id));

    // Get or select application (optional)
    let (app_id, app_name) = if let Some(ref app_str) = args.app {
        let app_id = app_str
            .parse()
            .map_err(|_| crate::errors::QuomeError::Usage("Invalid application ID".into()))?;

        let sp = ui::spinner("Fetching application...");
        let app = client.get_app(org_id, app_id).await?;
        sp.finish_and_clear();

        (Some(app.id), Some(app.name))
    } else {
        let sp = ui::spinner("Fetching applications...");
        let apps_resp = client.list_apps(org_id).await?;
        sp.finish_and_clear();

        if apps_resp.data.is_empty() {
            println!("No applications found in this organization.");
            (None, None)
        } else {
            let mut options: Vec<String> = apps_resp
                .data
                .iter()
                .map(|a| format!("{} ({})", a.name, a.id))
                .collect();
            options.push("(Skip - don't link an app)".to_string());

            let selection = Select::new("Select application:", options)
                .prompt()
                .map_err(|e| crate::errors::QuomeError::Io(std::io::Error::other(e.to_string())))?;

            if selection == "(Skip - don't link an app)" {
                (None, None)
            } else {
                let idx = apps_resp
                    .data
                    .iter()
                    .position(|a| format!("{} ({})", a.name, a.id) == selection)
                    .unwrap();

                let app = &apps_resp.data[idx];
                (Some(app.id), Some(app.name.clone()))
            }
        }
    };

    // Save linked context
    config.set_linked(LinkedContext {
        org_id,
        org_name: org_name.clone(),
        app_id,
        app_name: app_name.clone(),
    })?;
    config.save()?;

    let mut details = vec![("Organization", org_name.clone())];
    if let Some(ref name) = app_name {
        details.push(("Application", name.clone()));
    }

    let details_ref: Vec<(&str, &str)> = details.iter().map(|(k, v)| (*k, v.as_str())).collect();

    ui::print_success("Linked", &details_ref);

    Ok(())
}

/// The org a link may target: always the key's own. A `--org` that names a
/// different org is refused up front instead of failing on the first
/// resource call with a confusing "no access".
pub fn link_org(key_org: Uuid, requested: Option<Uuid>) -> Result<Uuid> {
    match requested {
        Some(r) if r != key_org => Err(crate::errors::QuomeError::Usage(format!(
            "This API key belongs to organization {} and cannot act on {}. \
             Log in with a key from that organization (one org = one key).",
            key_org, r
        ))),
        _ => Ok(key_org),
    }
}

fn short_id(id: &Uuid) -> String {
    let s = id.to_string();
    format!("org {}", &s[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_uses_the_keys_org_and_refuses_another() {
        let mine = Uuid::from_u128(1);
        let other = Uuid::from_u128(2);
        assert_eq!(link_org(mine, None).unwrap(), mine);
        assert_eq!(link_org(mine, Some(mine)).unwrap(), mine);
        let err = link_org(mine, Some(other)).unwrap_err().to_string();
        assert!(err.contains("one org = one key"), "{err}");
    }
}
