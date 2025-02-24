use crate::sipacker;

use super::args::Args;

use std::error::Error;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

pub fn run_app(_args: Args) -> Result<(), Box<dyn Error + Send + Sync>> {
    env_logger::init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_io()
        .enable_time()
        .build()?;
    rt.block_on(run_app_inner())?;

    Ok(())
}

async fn run_app_inner() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut user_agent = sipacker::user_agent::UserAgent::build(
        ("192.168.68.124".parse::<Ipv4Addr>().unwrap(), 5060).into(),
    )
    .await?;

    let reg_settings = sipacker::user_agent::registration::Settings::builder()
        .sip_server_port(5160)
        .sip_registrar_ip("192.168.68.119".parse().unwrap())
        .extension_number(3333)
        .expiry(Duration::from_secs(600))
        .build();

    user_agent.register(reg_settings).await?;

    loop {
        tokio::time::sleep(Duration::from_secs(20)).await;
    }

    Ok(())
}
