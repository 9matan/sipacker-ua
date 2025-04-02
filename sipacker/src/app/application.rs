use crate::app::{args::Args, cli_input, commands::Command};
use crate::sipacker::{audio::AudioSystem, user_agent::UserAgent};

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::Result;
use ezk_sip_auth::DigestCredentials;
use tokio::select;
use tokio::sync::mpsc;

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
    let (command_sender, command_receiver) = mpsc::channel(20);
    cli_input::run_input_system(command_sender)?;

    let ua_ip: Ipv4Addr = "192.168.0.117".parse().unwrap();
    let ua_port = 5060;

    let mut app = App::build((ua_ip, ua_port).into()).await?;
    app.run(command_receiver).await
}

pub(crate) struct App {
    user_agent: UserAgent,
    audio_system: AudioSystem,
}

impl App {
    pub(super) async fn build(ua_socket: SocketAddr) -> Result<Self> {
        let user_agent = UserAgent::build(ua_socket).await?;
        let audio_system = AudioSystem::build()?;
        Ok(Self {
            user_agent,
            audio_system,
        })
    }

    pub(super) async fn run(
        &mut self,
        mut command_receiver: mpsc::Receiver<Command>,
    ) -> Result<()> {
        loop {
            select! {
                command = command_receiver.recv() => if let Some(command) = command {
                    command.execute(self).await?
                },
                _ua_event = self.user_agent.run() => (),
            }

            //let _ = tokio::time::timeout(Duration::from_millis(100), ).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Ok(())
    }

    pub(crate) async fn register_ua(
        &mut self,
        user_name: &str,
        credentials: DigestCredentials,
        registrar_socket: SocketAddr,
    ) -> Result<()> {
        self.user_agent
            .register(user_name, credentials, registrar_socket)
            .await
    }

    pub(crate) async fn make_call(&mut self, target_user_name: &str) -> Result<()> {
        if !self.user_agent.is_registered() {
            Err(anyhow::Error::msg(
                "Can't make a call. The UA is not registered",
            ))
        } else {
            let audio_sender = self.audio_system.create_output_stream()?;
            let audio_receiver = self.audio_system.create_input_stream()?;
            self.user_agent
                .make_call(target_user_name, audio_sender, audio_receiver)
                .await
        }
    }
}
