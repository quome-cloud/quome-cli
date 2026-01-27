use clap::Parser;
use colored::Colorize;

use crate::api::models::CreateSessionRequest;
use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {
    /// Email address
    #[arg(short, long)]
    email: Option<String>,

    /// Password (will prompt if not provided)
    #[arg(short, long)]
    password: Option<String>,
}

pub async fn execute(args: Args) -> Result<()> {
    let email = match args.email {
        Some(e) => e,
        None => inquire::Text::new("Email:").prompt().map_err(|e| {
            crate::errors::QuomeError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
        })?,
    };

    let password = match args.password {
        Some(p) => p,
        None => inquire::Password::new("Password:")
            .without_confirmation()
            .prompt()
            .map_err(|e| {
                crate::errors::QuomeError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
            })?,
    };

    println!("Logging in...");

    let client = QuomeClient::new(None, None)?;

    let session = client
        .create_session(&CreateSessionRequest {
            email: Some(email.clone()),
            password: Some(password),
            session: None,
            organization_id: None,
        })
        .await?;

    // Now get user info with the new token
    let authed_client = QuomeClient::new(Some(&session.session), None)?;
    let user = authed_client.get_current_user().await?;

    // Save to config
    let mut config = Config::load()?;
    config.set_user(session.session, user.id, user.email.clone());
    config.save()?;

    println!(
        "{} Logged in as {}",
        "Success!".green().bold(),
        user.email.cyan()
    );

    Ok(())
}
