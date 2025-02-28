use sipacker_ua::prelude::*;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let args = args::Args {};
    application::run_app(args)
}
