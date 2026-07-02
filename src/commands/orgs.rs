use clap::{Parser, Subcommand};
use uuid::Uuid;

use crate::api::models::CreateOrgRequest;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui::{self, OrgRow};

#[derive(Subcommand)]
pub enum OrgsCommands {
    /// List all organizations
    List(ListArgs),
    /// Create a new organization
    Create(CreateArgs),
    /// Get organization details
    Get(GetArgs),
}

#[derive(Parser)]
pub struct ListArgs {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct CreateArgs {
    /// Organization name
    name: String,

    /// URL-safe slug (derived from name if not provided)
    #[arg(long)]
    slug: Option<String>,

    /// Organization description
    #[arg(short, long)]
    description: Option<String>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct GetArgs {
    /// Organization ID (uses linked org if not provided)
    #[arg(short, long)]
    id: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(command: OrgsCommands) -> Result<()> {
    match command {
        OrgsCommands::List(args) => list(args).await,
        OrgsCommands::Create(args) => create(args).await,
        OrgsCommands::Get(args) => get(args).await,
    }
}

/// Derive a URL-safe slug from an organization name.
fn slugify(name: &str) -> String {
    let mut slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    slug.trim_matches('-').to_string()
}

async fn list(args: ListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching organizations...");
    let orgs = client.list_orgs().await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&orgs)?);
    } else {
        if orgs.is_empty() {
            println!("No organizations found.");
            return Ok(());
        }

        let rows: Vec<OrgRow> = orgs
            .iter()
            .map(|org| OrgRow {
                id: org.id.to_string(),
                name: org.name.clone(),
                slug: org.slug.clone(),
                created: org.created_at.format("%Y-%m-%d %H:%M").to_string(),
            })
            .collect();

        ui::print_table(rows);
    }

    Ok(())
}

async fn create(args: CreateArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let client = QuomeClient::new(Some(&token), None)?;

    let slug = args.slug.unwrap_or_else(|| slugify(&args.name));

    let sp = ui::spinner("Creating organization...");
    let org = client
        .create_org(&CreateOrgRequest {
            name: args.name,
            slug,
            description: args.description,
        })
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&org)?);
    } else {
        ui::print_success(
            "Created organization",
            &[
                ("ID", &org.id.to_string()),
                ("Name", &org.name),
                ("Slug", &org.slug),
            ],
        );
    }

    Ok(())
}

async fn get(args: GetArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.id {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching organization...");
    let org = client.get_org(org_id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&org)?);
    } else {
        let mut details = vec![
            ("ID", org.id.to_string()),
            ("Name", org.name.clone()),
            ("Slug", org.slug.clone()),
        ];

        if let Some(ref desc) = org.description {
            details.push(("Description", desc.clone()));
        }
        if let Some(ref provider) = org.cloud_provider {
            details.push(("Cloud", provider.clone()));
        }
        details.push(("Cloud connected", org.gcp_connected.to_string()));
        details.push((
            "Created",
            org.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        ));

        let details_ref: Vec<(&str, &str)> =
            details.iter().map(|(k, v)| (*k, v.as_str())).collect();

        ui::print_detail(&org.name, &details_ref);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::slugify;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("My Org"), "my-org");
        assert_eq!(slugify("  Acme, Inc.  "), "acme-inc");
        assert_eq!(slugify("already-slugged"), "already-slugged");
        assert_eq!(slugify("Multiple   Spaces"), "multiple-spaces");
    }
}
