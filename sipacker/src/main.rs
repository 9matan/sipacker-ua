use anyhow::Result;
use sipacker_ua::prelude::*;

fn main() -> Result<()> {
    let args = args::Args {};
    application::run_app(args)
}
