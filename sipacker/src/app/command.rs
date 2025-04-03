use crate::app::application::App;

use std::{fmt::Display, net::SocketAddr};

use anyhow::Result;
use enum_dispatch::enum_dispatch;
use ezk_sip_auth::{DigestCredentials, DigestUser};

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
    RegisterCommand,
    UnregisterCommand,
    MakeCallCommand,
    TerminateCallCommand,
}

impl Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        DisplayExt::fmt(self, f)
    }
}

pub struct RegisterCommand {
    user_name: String,
    credential: DigestUser,
    registrar: SocketAddr,
}

impl RegisterCommand {
    pub fn new(user_name: &str, credential: DigestUser, registrar: SocketAddr) -> Self {
        Self {
            user_name: user_name.to_owned(),
            credential,
            registrar,
        }
    }
}

impl CommandTrait for RegisterCommand {
    async fn execute(self, app: &mut App) -> Result<()> {
        let mut credentials = DigestCredentials::new();
        credentials.set_default(self.credential);
        app.register_ua(&self.user_name, credentials, self.registrar)
            .await
    }
}

impl DisplayExt for RegisterCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "register {{user:{}; registrar:{}}}",
            self.user_name, self.registrar
        )
    }
}

#[derive(Debug)]
pub struct UnregisterCommand;

impl UnregisterCommand {
    pub fn new() -> Self {
        Self {}
    }
}

impl CommandTrait for UnregisterCommand {
    async fn execute(self, app: &mut App) -> Result<()> {
        app.unregister().await
    }
}

impl DisplayExt for UnregisterCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unregister")
    }
}

#[derive(Debug)]
pub struct MakeCallCommand {
    target_user_name: String,
}

impl MakeCallCommand {
    pub fn new(target_user_name: &str) -> Self {
        Self {
            target_user_name: target_user_name.to_owned(),
        }
    }
}

impl CommandTrait for MakeCallCommand {
    async fn execute(self, app: &mut App) -> Result<()> {
        app.make_call(&self.target_user_name).await
    }
}

impl DisplayExt for MakeCallCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "make call {{user:{}}}", self.target_user_name)
    }
}

#[derive(Debug)]
pub struct TerminateCallCommand;

impl TerminateCallCommand {
    pub fn new() -> Self {
        Self {}
    }
}

impl CommandTrait for TerminateCallCommand {
    async fn execute(self, app: &mut App) -> Result<()> {
        app.terminate_call().await
    }
}

impl DisplayExt for TerminateCallCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "terminate call")
    }
}
