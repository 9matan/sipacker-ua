use std::{net::AddrParseError, thread, time::Duration};

use crate::app::command::{self, Command};

use anyhow::Result;
use enum_dispatch::enum_dispatch;
use ezk_sip_auth::DigestUser;
use tokio::sync::mpsc;

pub(crate) fn run_input_system() -> mpsc::Receiver<Command> {
    let (command_sender, command_receiver) = mpsc::channel(20);
    thread::spawn(|| run_input_system_inner(command_sender));
    command_receiver
}

fn run_input_system_inner(command_sender: mpsc::Sender<Command>) {
    let mut input_system = CliInputSystem::new(command_sender);
    if let Err(err) = input_system.run() {
        tracing::error!("CLI input system err: {err}");
    }
}

struct CliInputSystem {
    command_sender: mpsc::Sender<Command>,
    parsers: Vec<CommandParser>,
}

impl CliInputSystem {
    pub fn new(command_sender: mpsc::Sender<Command>) -> Self {
        let parsers = vec![
            RegisterParser::new().into(),
            UnregisterParser::new().into(),
            MakeCallParser::new().into(),
            TerminateCallParser::new().into(),
        ];
        Self {
            command_sender,
            parsers,
        }
    }

    pub fn run(&mut self) -> Result<()> {
        tracing::info!("The CLI input system is running");
        loop {
            let command = self.read_command();
            if let Some(command) = command {
                self.send_command(command);
            }

            thread::sleep(Duration::from_secs(1));
        }
    }

    fn send_command<C: Into<Command>>(&mut self, command: C) {
        let result = self.command_sender.blocking_send(command.into());
        match result {
            Ok(_) => (),
            Err(err) => {
                tracing::error!("CLI input system err: {err}");
            }
        }
    }

    fn read_command(&mut self) -> Option<Command> {
        let line = misc::read_stdin_line()?;
        if line.starts_with("help") {
            self.print_help();
            None
        } else {
            self.parse_command(&line)
        }
    }

    fn print_help(&self) {
        println!("==== Help ====");
        for parser in &self.parsers {
            println!("\t {}", parser.get_help());
        }
    }

    fn parse_command(&self, line: &str) -> Option<Command> {
        // skip CommandParserError::Command error, try to find a parser for a command with a specified name
        let result = self.parsers.iter().find_map(|parser| {
            let result = parser.parse(line);
            if result.is_ok()
                || result
                    .as_ref()
                    .is_err_and(|err| matches!(err, CommandParserError::Arguments(_s)))
            {
                Some(result)
            } else {
                None
            }
        });

        match result {
            Some(result) => result
                .inspect_err(|err| {
                    tracing::warn!("CLI input system parser err: {err:?}");
                })
                .ok(),
            None => {
                tracing::warn!("Unknown command");
                None
            }
        }
    }
}

#[derive(Debug)]
enum CommandParserError {
    Command,
    Arguments(String),
}

#[enum_dispatch()]
trait CommandParserTrait {
    fn parse(&self, line: &str) -> Result<Command, CommandParserError>;
    fn get_help(&self) -> &str;
}

#[enum_dispatch(CommandParserTrait)]
enum CommandParser {
    RegisterParser,
    UnregisterParser,
    MakeCallParser,
    TerminateCallParser,
}

pub struct RegisterParser {
    parser: parser::Parser,
}

impl RegisterParser {
    pub fn new() -> Self {
        let parser = parser::Parser::new(["user".into(), "password".into(), "registrar".into()]);
        Self { parser }
    }
}

impl CommandParserTrait for RegisterParser {
    fn parse(&self, line: &str) -> Result<Command, CommandParserError> {
        if !line.starts_with("register") {
            Err(CommandParserError::Command)
        } else {
            let data = self
                .parser
                .parse(line.trim_start_matches("register"))
                .map_err(|err| CommandParserError::Arguments(err.to_string()))?;

            let user_name = data.get("user").ok_or(CommandParserError::Arguments(
                "\"user\" field is missing".to_owned(),
            ))?;
            let def_password = "".to_owned();
            let password = data.get("password").unwrap_or(&def_password);
            let registrar = data.get("registrar").ok_or(CommandParserError::Arguments(
                "\"registrar\" field is missing".to_owned(),
            ))?;

            let credential = DigestUser::new(user_name, password.as_bytes());
            let registrar = registrar
                .parse()
                .map_err(|err: AddrParseError| CommandParserError::Arguments(err.to_string()))?;

            let command = command::RegisterCommand::new(user_name, credential, registrar);

            Ok(command.into())
        }
    }

    fn get_help(&self) -> &str {
        "register user=<extension_number> [password=<password>] registrar=<ip:port>"
    }
}

pub struct UnregisterParser;

impl UnregisterParser {
    pub fn new() -> Self {
        Self {}
    }
}

impl CommandParserTrait for UnregisterParser {
    fn parse(&self, line: &str) -> Result<Command, CommandParserError> {
        if !line.starts_with("unregister") {
            Err(CommandParserError::Command)
        } else {
            Ok(command::UnregisterCommand::new().into())
        }
    }

    fn get_help(&self) -> &str {
        "unregister"
    }
}

pub struct MakeCallParser {
    parser: parser::Parser,
}

impl MakeCallParser {
    pub fn new() -> Self {
        let parser = parser::Parser::new(["user".into()]);
        Self { parser }
    }
}

impl CommandParserTrait for MakeCallParser {
    fn parse(&self, line: &str) -> Result<Command, CommandParserError> {
        if !line.starts_with("call") {
            Err(CommandParserError::Command)
        } else {
            let data = self
                .parser
                .parse(line.trim_start_matches("call"))
                .map_err(|err| CommandParserError::Arguments(err.to_string()))?;

            let target_user_name = data.get("user").ok_or(CommandParserError::Arguments(
                "\"user\" field is missing".to_owned(),
            ))?;

            let command = command::MakeCallCommand::new(target_user_name);

            Ok(command.into())
        }
    }

    fn get_help(&self) -> &str {
        "call user=<extension_number>"
    }
}

pub struct TerminateCallParser;

impl TerminateCallParser {
    pub fn new() -> Self {
        Self {}
    }
}

impl CommandParserTrait for TerminateCallParser {
    fn parse(&self, line: &str) -> Result<Command, CommandParserError> {
        if !line.starts_with("terminate call") {
            Err(CommandParserError::Command)
        } else {
            Ok(command::TerminateCallCommand::new().into())
        }
    }

    fn get_help(&self) -> &str {
        "terminate call"
    }
}

mod parser {
    use std::collections::HashMap;

    use anyhow::Result;

    pub struct Parser {
        fields: Vec<String>,
    }

    impl Parser {
        pub fn new<I: IntoIterator<Item = String>>(fields: I) -> Self {
            let fields = fields.into_iter().collect();
            Self { fields }
        }

        pub fn parse(&self, line: &str) -> Result<HashMap<String, String>> {
            let tokens = line.split(' ');
            let mut data = HashMap::new();

            for token in tokens.filter(|token| !token.is_empty()) {
                let (name, value) = self.parse_field(token)?;
                if self.fields.contains(&name.into()) {
                    let _ = data.insert(name.into(), value.to_owned());
                } else {
                    return Err(anyhow::Error::msg(format!("Unknown field: {name}")));
                }
            }

            Ok(data)
        }

        fn parse_field<'a>(&self, token: &'a str) -> Result<(&'a str, &'a str)> {
            let mut field = token.split('=');
            let name = field
                .next()
                .ok_or(anyhow::Error::msg("Field name is missing"))?;
            let value = field
                .next()
                .ok_or(anyhow::Error::msg("Field value is missing"))?;
            Ok((name, value))
        }
    }
}

mod misc {
    pub fn read_stdin_line() -> Option<String> {
        let mut buf = String::new();
        std::io::stdin()
            .read_line(&mut buf)
            .inspect_err(|err| {
                tracing::warn!("CLI input system err: {err}");
            })
            .map(|_| {
                trim_newline(&mut buf);
                buf
            })
            .ok()
    }

    fn trim_newline(s: &mut String) {
        if s.ends_with('\n') {
            s.pop();
            if s.ends_with('\r') {
                s.pop();
            }
        }
    }
}
