use clap::Parser;
use colored::Colorize;

use crate::config::Config;
use crate::errors::Result;

#[derive(Parser)]
pub struct Args {}

pub async fn execute(_args: Args) -> Result<()> {
    let mut config = Config::load()?;

    if config.get_linked()?.is_none() {
        println!("Not linked to any organization or application.");
        return Ok(());
    }

    config.clear_linked()?;
    config.save()?;

    println!("{} Unlinked current directory.", "Success!".green().bold());

    Ok(())
}
