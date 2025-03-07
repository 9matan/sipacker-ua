use tokio::select;

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
    let ua_ip: Ipv4Addr = "192.168.68.203".parse().unwrap();
    let sip_ip: Ipv4Addr = "192.168.68.188".parse().unwrap();
    let mut user_agent = sipacker::user_agent::UserAgent::build((ua_ip, 5060).into()).await?;

    let reg_settings = sipacker::user_agent::registration::Settings::builder()
        .sip_server_port(5160)
        .sip_registrar_ip(sip_ip.into())
        .extension_number(3333)
        .expiry(Duration::from_secs(600))
        .build();

    user_agent.register(reg_settings).await?;

    loop {
        select! {
            _ = user_agent.run() => {

            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {

            }
        };

        if let Some(from) = user_agent.get_incoming_call().await {
            println!("Incoming call from {:#?}", from.uri.uri);
            tokio::time::sleep(Duration::from_secs(1)).await;
            user_agent.accept_incoming_call().await;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    Ok(())
}
