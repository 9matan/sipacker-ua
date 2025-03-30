use crate::sipacker;
use crate::sipacker::user_agent_event::*;

use super::args::Args;

use std::error::Error;
use std::net::Ipv4Addr;
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
    let (audio_out_tx, audio_out_rx) = tokio::sync::mpsc::channel(100);
    let audio = sipacker::audio::AudioSystem::build(audio_out_rx)?;

    let ua_ip: Ipv4Addr = "192.168.0.117".parse().unwrap();
    let sip_ip: Ipv4Addr = "192.168.0.90".parse().unwrap();
    let mut user_agent =
        sipacker::user_agent::UserAgent::build((ua_ip, 5060).into(), audio_out_tx).await?;

    let reg_settings = sipacker::user_agent::registration::Settings::builder()
        .sip_server_port(5170)
        .sip_registrar_ip(sip_ip.into())
        .extension_number(2502)
        .expiry(Duration::from_secs(600))
        .build();

    user_agent.register(reg_settings).await?;

    loop {
        let user_agent_event = user_agent.run(Duration::from_millis(200)).await;
        if let Some(user_agent_event) = user_agent_event {
            if let UserAgentEventData::IncomingCall(incoming) = user_agent_event.data {
                println!("Incoming call from {:#?}", incoming.incoming_client);
                tokio::time::sleep(Duration::from_secs(1)).await;
                user_agent.accept_incoming_call().await;
                println!("The call is accepted");
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
