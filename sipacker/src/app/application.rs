use crate::app::{
    args::Args,
    cli_input,
    command::{Command, CommandTrait},
};
use crate::sipacker::{
    audio::AudioSystem,
    user_agent::{UserAgent, UserAgentEvent},
};

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::Result;
use ezk_sip_auth::DigestCredentials;
use ezk_sip_types::host::HostPort;
use tokio::sync::mpsc;

pub fn run_app(args: Args) -> Result<()> {
    init_logging();
    tracing::info!("Initializing the application...");

    let rt = create_async_runtime(args.jobs)?;
    tracing::info!("Async runtime is initialized");
    rt.block_on(run_app_inner(args))?;

    Ok(())
}

fn init_logging() {
    use tracing_subscriber::{
        filter::LevelFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter,
    };

    let envfilter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .with_env_var("RUST_LOG")
        .from_env_lossy();
    tracing_subscriber::registry()
        .with(envfilter)
        .with(fmt::Layer::default())
        .init();
}

fn create_async_runtime(threads_count: usize) -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads_count)
        .enable_io()
        .enable_time()
        .build()
}

async fn run_app_inner(args: Args) -> Result<()> {
    let ua_ip: Ipv4Addr = args.ip_addr;
    let ua_port = args.port;

    let command_receiver = cli_input::run_input_system();

    let mut app = App::build((ua_ip, ua_port).into()).await?;
    app.run(command_receiver).await
}

pub(crate) struct App {
    stop_app: bool,
    user_agent: UserAgent,
    audio_system: AudioSystem,
}

impl App {
    pub(super) async fn build(ua_socket: SocketAddr) -> Result<Self> {
        let user_agent = UserAgent::build(ua_socket).await?;
        tracing::info!("User agent is initialized");
        let audio_system = AudioSystem::build()?;
        tracing::info!("Audio system is initialized");
        Ok(Self {
            stop_app: false,
            user_agent,
            audio_system,
        })
    }

    pub(super) async fn run(
        &mut self,
        mut command_receiver: mpsc::Receiver<Command>,
    ) -> Result<()> {
        tracing::info!("The application is running");
        println!("The application is running");
        while !self.stop_app {
            self.update_user_agent().await;
            if let Ok(command) = command_receiver.try_recv() {
                self.execute_command(command).await;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    async fn execute_command(&mut self, command: Command) {
        tracing::info!("Executing the command: {}", command);
        let _ = command
            .execute(self)
            .await
            .inspect_err(|err| tracing::warn!("Command execution err: {err}"));
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
                tracing::error!("User agent updating err: {err}");
            }
        }
    }

    fn handle_ua_event(&mut self, event: UserAgentEvent) {
        tracing::debug!("Handling UA event: {:?}", event);
        self.print_ua_event(&event);
        if event == UserAgentEvent::CallTerminated {
            self.audio_system.destroy_input_stream();
            self.audio_system.destroy_output_stream();
        }
    }

    fn print_ua_event(&self, event: &UserAgentEvent) {
        match event {
            UserAgentEvent::CallEstablished => println!("The call is established"),
            UserAgentEvent::Calling => println!("Calling..."),
            UserAgentEvent::CallTerminated => println!("The call is terminated"),
            UserAgentEvent::Registered => println!("The agent is registered"),
            UserAgentEvent::Unregistered => println!("The agent is unregistered"),
        }
    }

    pub(crate) async fn register_ua(
        &mut self,
        user_name: &str,
        credentials: DigestCredentials,
        registrar_host: HostPort,
    ) -> Result<()> {
        tracing::info!("Registering the UA: {user_name}");
        self.user_agent
            .register(user_name, credentials, registrar_host)
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
            tracing::info!("Making a call to {target_user_name}");
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
            tracing::info!("Terminating the call.");
            self.user_agent.terminate_call().await
        }
    }

    pub(crate) async fn unregister(&mut self) -> Result<()> {
        self.user_agent.unregister();
        Ok(())
    }

    pub(crate) fn stop_app(&mut self) -> Result<()> {
        self.stop_app = true;
        Ok(())
    }
}
