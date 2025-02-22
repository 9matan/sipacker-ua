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
    let settings = crate::sipacker::registration::Settings {
        sip_port: 5160,
        sip_registrar_ip: Ipv4Addr::new(192,168,3, 71).into(),
        contact_ip: Ipv4Addr::new(192,168,3, 92).into(),
        extension_number: 3333,
        expiry: Duration::from_secs(600),
    };

    let registrator = crate::sipacker::registration::Registrator::build(settings).await?;

    Arc::clone(&registrator).run_registration().await;

    loop {
        tokio::time::sleep(Duration::from_secs(20)).await;
    }

    Ok(())
}