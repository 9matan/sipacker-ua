use crate::app::application::App;

use std::fmt::Display;

use anyhow::Result;
use enum_dispatch::enum_dispatch;
use ezk_sip_auth::{DigestCredentials, DigestUser};
use ezk_sip_types::host::HostPort;

#[enum_dispatch]
pub trait CommandTrait {
    async fn execute(self, app: &mut App) -> Result<()>;
}

#[enum_dispatch]
trait DisplayExt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
}

#[enum_dispatch(CommandTrait, DisplayExt)]
pub enum Command {
    Register,
    Unregister,
    MakeCall,
    TerminateCall,
    StopApp,
}

impl Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        DisplayExt::fmt(self, f)
    }
}

pub struct Register {
    user_name: String,
    credential: DigestUser,
    registrar_host: HostPort,
}

impl Register {
    pub fn new(user_name: &str, credential: DigestUser, registrar_host: HostPort) -> Self {
        Self {
            user_name: user_name.to_owned(),
            credential,
            registrar_host,
        }
    }
}

impl CommandTrait for Register {
    async fn execute(self, app: &mut App) -> Result<()> {
        let mut credentials = DigestCredentials::new();
        credentials.set_default(self.credential);
        app.register_ua(&self.user_name, credentials, self.registrar_host)
            .await
    }
}

impl DisplayExt for Register {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "register {{user:{}; registrar:{}}}",
            self.user_name,
            self.registrar_host.to_string(),
        )
    }
}

#[derive(Debug)]
pub struct Unregister;

impl Unregister {
    pub fn new() -> Self {
        Self {}
    }
}

impl CommandTrait for Unregister {
    async fn execute(self, app: &mut App) -> Result<()> {
        app.unregister().await
    }
}

impl DisplayExt for Unregister {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unregister")
    }
}

#[derive(Debug)]
pub struct MakeCall {
    target_user_name: String,
}

impl MakeCall {
    pub fn new(target_user_name: &str) -> Self {
        Self {
            target_user_name: target_user_name.to_owned(),
        }
    }
}

impl CommandTrait for MakeCall {
    async fn execute(self, app: &mut App) -> Result<()> {
        app.make_call(&self.target_user_name).await
    }
}

impl DisplayExt for MakeCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "make call {{user:{}}}", self.target_user_name)
    }
}

#[derive(Debug)]
pub struct TerminateCall;

impl TerminateCall {
    pub fn new() -> Self {
        Self {}
    }
}

impl CommandTrait for TerminateCall {
    async fn execute(self, app: &mut App) -> Result<()> {
        app.terminate_call().await
    }
}

impl DisplayExt for TerminateCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "terminate call")
    }
}

#[derive(Debug)]
pub struct StopApp;

impl StopApp {
    pub fn new() -> Self {
        Self {}
    }
}

impl CommandTrait for StopApp {
    async fn execute(self, app: &mut App) -> Result<()> {
        app.stop_app()
    }
}

impl DisplayExt for StopApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stop app")
    }
}
