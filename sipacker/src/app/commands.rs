use std::fmt::Display;

use crate::app::application::App;

use anyhow::Result;

pub enum Command {
    Register(commands::Register),
    MakeCall(commands::MakeCall),
}

impl Command {
    pub async fn execute(self, app: &mut App) -> Result<()> {
        match self {
            Command::Register(cmd) => cmd.execute(app).await,
            Command::MakeCall(cmd) => cmd.execute(app).await,
        }
    }
}

impl Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::Register(cmd) => cmd.fmt(f),
            Command::MakeCall(cmd) => cmd.fmt(f),
        }
    }
}

pub(crate) mod commands {
    use super::Command;
    use crate::app::application::App;

    use std::{fmt::Display, net::SocketAddr};

    use anyhow::Result;
    use ezk_sip_auth::{DigestCredentials, DigestUser};

    pub struct Register {
        user_name: String,
        credential: DigestUser,
        registrar: SocketAddr,
    }

    impl Register {
        pub fn new(user_name: &str, credential: DigestUser, registrar: SocketAddr) -> Self {
            Self {
                user_name: user_name.to_owned(),
                credential,
                registrar,
            }
        }

        pub async fn execute(self, app: &mut App) -> Result<()> {
            let mut credentials = DigestCredentials::new();
            credentials.set_default(self.credential);
            app.register_ua(&self.user_name, credentials, self.registrar)
                .await
        }
    }

    impl From<Register> for Command {
        fn from(value: Register) -> Self {
            Command::Register(value)
        }
    }

    impl Display for Register {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "register {{user:{}; registrar:{}}}",
                self.user_name, self.registrar
            )
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

        pub async fn execute(self, app: &mut App) -> Result<()> {
            app.make_call(&self.target_user_name).await
        }
    }

    impl From<MakeCall> for Command {
        fn from(value: MakeCall) -> Self {
            Command::MakeCall(value)
        }
    }

    impl Display for MakeCall {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "make call {{user:{}}}", self.target_user_name)
        }
    }
}
