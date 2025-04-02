use anyhow::Result;

use clap::Parser;
use sipacker_ua::app::{application, args};

fn main() -> Result<()> {
    let args = args::Args::try_parse()?;
    application::run_app(args)
}
