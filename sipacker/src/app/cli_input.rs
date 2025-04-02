use std::{net::Ipv4Addr, time::Duration};

use crate::app::commands::{commands, Command};

use anyhow::Result;
use ezk_sip_auth::DigestUser;
use tokio::sync::mpsc;

pub(crate) fn run_input_system(command_sender: mpsc::Sender<Command>) -> Result<()> {
    tokio::spawn(run_input_system_inner(command_sender));
    Ok(())
}

async fn run_input_system_inner(command_sender: mpsc::Sender<Command>) {
    let mut input_system = CliInputSystem::new(command_sender);
    if let Err(err) = input_system.run().await {
        log::error!(err:%; "Cli input system");
    }
}

struct CliInputSystem {
    command_sender: mpsc::Sender<Command>,
}

impl CliInputSystem {
    pub fn new(command_sender: mpsc::Sender<Command>) -> Self {
        Self { command_sender }
    }

    pub async fn run(&mut self) -> Result<()> {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let sip_ip: Ipv4Addr = "192.168.0.90".parse().unwrap();
        let reg_cmd =
            commands::Register::new("2502", DigestUser::new("2502", ""), (sip_ip, 5170).into());
        self.command_sender.send(reg_cmd.into()).await?;

        tokio::time::sleep(Duration::from_secs(2)).await;

        let make_call_cmd = commands::MakeCall::new("2503");
        self.command_sender.send(make_call_cmd.into()).await?;

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}
