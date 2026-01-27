use clap::{Parser, Subcommand};
use uuid::Uuid;

use crate::api::models::AddOrgMemberRequest;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui::{self, MemberRow};

#[derive(Subcommand)]
pub enum MembersCommands {
    /// List organization members
    List(ListArgs),
    /// Add a member to the organization
    Add(AddArgs),
}

#[derive(Parser)]
pub struct ListArgs {
    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
pub struct AddArgs {
    /// User ID to add
    user_id: Uuid,

    /// Organization ID (uses linked org if not provided)
    #[arg(long)]
    org: Option<Uuid>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

pub async fn execute(command: MembersCommands) -> Result<()> {
    match command {
        MembersCommands::List(args) => list(args).await,
        MembersCommands::Add(args) => add(args).await,
    }
}

async fn list(args: ListArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Fetching members...");
    let response = client.list_org_members(org_id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.members)?);
    } else {
        if response.members.is_empty() {
            println!("No members found.");
            return Ok(());
        }

        let rows: Vec<MemberRow> = response
            .members
            .iter()
            .map(|member| MemberRow {
                user_id: member.user_id.to_string(),
                member_id: member.id.to_string(),
                joined: member.created_at.format("%Y-%m-%d %H:%M").to_string(),
            })
            .collect();

        ui::print_table(rows);
    }

    Ok(())
}

async fn add(args: AddArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Adding member...");
    let member = client
        .add_org_member(
            org_id,
            &AddOrgMemberRequest {
                user_id: args.user_id,
            },
        )
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&member)?);
    } else {
        ui::print_success("Added member", &[
            ("Member ID", &member.id.to_string()),
            ("User ID", &member.user_id.to_string()),
        ]);
    }

    Ok(())
}
