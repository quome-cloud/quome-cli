use clap::{Parser, Subcommand};
use uuid::Uuid;

use crate::api::models::CreateOrgInviteRequest;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui::{self, MemberRow};

#[derive(Subcommand)]
pub enum MembersCommands {
    /// List organization members
    List(ListArgs),
    /// Invite a member to the organization by email
    Invite(InviteArgs),
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
pub struct InviteArgs {
    /// Email address to invite
    email: String,

    /// Role for the invited member (member or admin)
    #[arg(long, default_value = "member")]
    role: String,

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
        MembersCommands::Invite(args) => invite(args).await,
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
    let members = client.list_org_members(org_id).await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&members)?);
    } else {
        if members.is_empty() {
            println!("No members found.");
            return Ok(());
        }

        let rows: Vec<MemberRow> = members
            .iter()
            .map(|member| MemberRow {
                name: member.user_name.clone(),
                email: member.user_email.clone(),
                role: member.role.clone(),
                joined: member.created_at.format("%Y-%m-%d %H:%M").to_string(),
            })
            .collect();

        ui::print_table(rows);
    }

    Ok(())
}

async fn invite(args: InviteArgs) -> Result<()> {
    let config = Config::load()?;
    let token = config.require_token()?;

    let org_id = match args.org {
        Some(id) => id,
        None => config.require_linked_org()?,
    };

    let client = QuomeClient::new(Some(&token), None)?;

    let sp = ui::spinner("Sending invite...");
    let invite = client
        .create_org_invite(
            org_id,
            &CreateOrgInviteRequest {
                email: args.email,
                role: args.role,
            },
        )
        .await?;
    sp.finish_and_clear();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&invite)?);
    } else {
        let expires = invite
            .expires_at
            .map(|e| e.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "-".to_string());
        ui::print_success(
            "Invited member",
            &[
                ("Email", &invite.email),
                ("Role", &invite.role),
                ("Expires", &expires),
            ],
        );
    }

    Ok(())
}
