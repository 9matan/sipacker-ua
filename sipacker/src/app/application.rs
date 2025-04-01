use crate::app::args::Args;
use crate::sipacker;
use crate::sipacker::user_agent::UserAgent;

use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::Result;
use ezk_sip_auth::{DigestCredentials, DigestUser};

pub fn run_app(_args: Args) -> Result<()> {
    env_logger::init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_io()
        .enable_time()
        .build()?;
    rt.block_on(run_app_inner())?;

    Ok(())
}

async fn run_app_inner() -> Result<()> {
    let mut audio_system = sipacker::audio::AudioSystem::build()?;
    let audio_sender = audio_system.create_output_stream()?;
    let audio_receiver = audio_system.create_input_stream()?;

    let ua_ip: Ipv4Addr = "192.168.0.117".parse().unwrap();
    let sip_ip: Ipv4Addr = "192.168.0.90".parse().unwrap();

    let mut user_agent = UserAgent::build((ua_ip, 5060).into()).await?;
    let mut credentials = DigestCredentials::new();
    //credentials.set_default(DigestUser::new("2502", "2502"));
    //credentials.add_for_realm("asterisk", DigestUser::new("2502", "2502"));
    let registrar_socket = (sip_ip, 5170).into();

    user_agent
        .register("2502", registrar_socket, credentials)
        .await?;

    tokio::time::sleep(Duration::from_secs(2)).await;

    user_agent
        .make_call("2503", audio_sender, audio_receiver)
        .await?;

    loop {
        let _ = user_agent.run().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Ok(())
}
