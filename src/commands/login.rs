use clap::Parser;

use crate::client::QuomeClient;
use crate::config::Config;
use crate::errors::Result;
use crate::ui;

#[derive(Parser)]
pub struct Args {
    /// API key (will prompt if not provided)
    #[arg(short, long)]
    token: Option<String>,
}

pub async fn execute(args: Args) -> Result<()> {
    let token = match args.token {
        Some(t) => t,
        None => inquire::Password::new("API Key:")
            .without_confirmation()
            .with_help_message("Generate an API key from the Quome dashboard")
            .prompt()
            .map_err(|e| crate::errors::QuomeError::Io(std::io::Error::other(e.to_string())))?,
    };

    let sp = ui::spinner("Validating token...");

    // Validate the token by fetching user info
    let client = QuomeClient::new(Some(&token), None)?;
    let user = client.get_current_user().await?;

    // Save to config
    let mut config = Config::load()?;
    config.set_user(token, user.id, user.email.clone());
    config.save()?;

    sp.finish_and_clear();

    ui::print_success("Logged in", &[
        ("Email", &user.email),
        ("User ID", &user.id.to_string()),
    ]);

    Ok(())
}
