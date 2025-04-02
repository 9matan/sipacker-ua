use crate::app::{args::Args, cli_input, commands::Command};
use crate::sipacker::user_agent::UserAgentEvent;
use crate::sipacker::{audio::AudioSystem, user_agent::UserAgent};

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::Result;
use ezk_sip_auth::DigestCredentials;
use tokio::select;
use tokio::sync::mpsc;

pub fn run_app(args: Args) -> Result<()> {
    env_logger::init();
    log::info!("Initializing the application");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_io()
        .enable_time()
        .build()?;
    rt.block_on(run_app_inner(args))?;

    Ok(())
}

async fn run_app_inner(args: Args) -> Result<()> {
    let ua_ip: Ipv4Addr = args.ip_addr;
    let ua_port = args.port;

    let command_receiver = cli_input::run_input_system();

    log::info!("Running the application");
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
                    self.execute_command(command).await
                },
                _ = self.update_user_agent() => (),
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn execute_command(&mut self, command: Command) {
        log::info!(command:%; "Executing the command.");
        let _ = command
            .execute(self)
            .await
            .inspect_err(|err| log::warn!(err:%; "Command execution."));
    }

    async fn update_user_agent(&mut self) {
        let result = self.user_agent.run().await;
        match result {
            Ok(event) => {
                if let Some(event) = event {
                    self.handle_ua_event(event);
                }
            }
            Err(err) => {
                log::error!(err:%; "User agent updating.");
            }
        }
    }

    fn handle_ua_event(&mut self, event: UserAgentEvent) {
        log::debug!(event:?; "Handling ua event.");
        match event {
            UserAgentEvent::CallTerminated => {
                self.audio_system.destroy_input_stream();
                self.audio_system.destroy_output_stream();
            }
        }
    }

    pub(crate) async fn register_ua(
        &mut self,
        user_name: &str,
        credentials: DigestCredentials,
        registrar_socket: SocketAddr,
    ) -> Result<()> {
        log::info!(user_name:%; "Registering the UA.");
        self.user_agent
            .register(user_name, credentials, registrar_socket)
            .await
    }

    pub(crate) async fn make_call(&mut self, target_user_name: &str) -> Result<()> {
        if !self.user_agent.is_registered() {
            Err(anyhow::Error::msg(
                "Can't make a call. The UA is not registered",
            ))
        } else if self.user_agent.has_active_call() {
            Err(anyhow::Error::msg(
                "Can't make a call. There is an active call already",
            ))
        } else {
            log::info!(target_user_name:%; "Making a call.");
            let audio_sender = self.audio_system.create_output_stream()?;
            let audio_receiver = self.audio_system.create_input_stream()?;
            self.user_agent
                .make_call(target_user_name, audio_sender, audio_receiver)
                .await
        }
    }

    pub(crate) async fn terminate_call(&mut self) -> Result<()> {
        if !self.user_agent.has_active_call() {
            Err(anyhow::Error::msg(
                "Can't terminate a call. There is no active call",
            ))
        } else {
            log::info!("Terminating the call.");
            self.audio_system.destroy_input_stream();
            self.audio_system.destroy_output_stream();
            self.user_agent.terminate_call().await
        }
    }
}
