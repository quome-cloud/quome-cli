use clap::Parser;
use colored::Colorize;

use crate::config::Config;
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {}

pub async fn execute(_args: Args) -> Result<()> {
    let mut config = Config::load()?;

    if config.user.is_none() {
        println!("Not logged in.");
        return Ok(());
    }

    config.clear_user();
    config.save()?;

    println!("{} Logged out successfully.", "Success!".green().bold());

    Ok(())
}
