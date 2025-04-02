use anyhow::Result;

use sipacker_ua::app::{application, args};

fn main() -> Result<()> {
    let args = args::Args {};
    application::run_app(args)
}
