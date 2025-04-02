use std::{thread, time::Duration};

use crate::app::commands::Command;

use anyhow::Result;
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
            parsers::Register::new().into(),
            parsers::Unregister::new().into(),
            parsers::MakeCall::new().into(),
            parsers::TerminateCall::new().into(),
        ];
        Self {
            command_sender,
            parsers,
        }
    }

    pub fn run(&mut self) -> Result<()> {
        tracing::info!("Running the CLI input system");
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
                || result.as_ref().is_err_and(|err| match err {
                    CommandParserError::Arguments(_) => true,
                    _ => false,
                })
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

enum CommandParser {
    Register(parsers::Register),
    Unregister(parsers::Unregister),
    MakeCall(parsers::MakeCall),
    TerminateCall(parsers::TerminateCall),
}

impl CommandParser {
    fn parse(&self, line: &str) -> Result<Command, CommandParserError> {
        match self {
            CommandParser::Register(parser) => parser.parse(line),
            CommandParser::Unregister(parser) => parser.parse(line),
            CommandParser::MakeCall(parser) => parser.parse(line),
            CommandParser::TerminateCall(parser) => parser.parse(line),
        }
    }

    fn get_help(&self) -> &str {
        match self {
            CommandParser::Register(parser) => parser.get_help(),
            CommandParser::Unregister(parser) => parser.get_help(),
            CommandParser::MakeCall(parser) => parser.get_help(),
            CommandParser::TerminateCall(parser) => parser.get_help(),
        }
    }
}

mod parsers {
    use super::{CommandParser, CommandParserError};
    use crate::app::commands::{commands, Command};

    use std::{collections::HashMap, net::AddrParseError};

    use anyhow::Result;
    use ezk_sip_auth::DigestUser;

    struct Parser {
        fields: Vec<String>,
    }

    impl Parser {
        fn new<I: IntoIterator<Item = String>>(fields: I) -> Self {
            let fields = fields.into_iter().collect();
            Self { fields }
        }

        fn parse(&self, line: &str) -> Result<HashMap<String, String>> {
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

    pub struct Register {
        parser: Parser,
    }

    impl Register {
        pub fn new() -> Self {
            let parser = Parser::new(["user".into(), "password".into(), "registrar".into()]);
            Self { parser }
        }

        pub fn parse(&self, line: &str) -> Result<Command, CommandParserError> {
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
                let registrar = registrar.parse().map_err(|err: AddrParseError| {
                    CommandParserError::Arguments(err.to_string())
                })?;

                let command = commands::Register::new(user_name, credential, registrar);

                Ok(command.into())
            }
        }

        pub fn get_help(&self) -> &str {
            "register user=<extension_number> [password=<password>] registrar=<ip:port>"
        }
    }

    impl From<Register> for CommandParser {
        fn from(value: Register) -> Self {
            CommandParser::Register(value)
        }
    }

    pub struct Unregister;

    impl Unregister {
        pub fn new() -> Self {
            Self {}
        }

        pub fn parse(&self, line: &str) -> Result<Command, CommandParserError> {
            if !line.starts_with("unregister") {
                Err(CommandParserError::Command)
            } else {
                Ok(commands::Unregister::new().into())
            }
        }

        pub fn get_help(&self) -> &str {
            "unregister"
        }
    }

    impl From<Unregister> for CommandParser {
        fn from(value: Unregister) -> Self {
            CommandParser::Unregister(value)
        }
    }

    pub struct MakeCall {
        parser: Parser,
    }

    impl MakeCall {
        pub fn new() -> Self {
            let parser = Parser::new(["user".into()]);
            Self { parser }
        }

        pub fn parse(&self, line: &str) -> Result<Command, CommandParserError> {
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

                let command = commands::MakeCall::new(target_user_name);

                Ok(command.into())
            }
        }

        pub fn get_help(&self) -> &str {
            "call user=<extension_number>"
        }
    }

    impl From<MakeCall> for CommandParser {
        fn from(value: MakeCall) -> Self {
            CommandParser::MakeCall(value)
        }
    }

    pub struct TerminateCall;

    impl TerminateCall {
        pub fn new() -> Self {
            Self {}
        }

        pub fn parse(&self, line: &str) -> Result<Command, CommandParserError> {
            if !line.starts_with("terminate call") {
                Err(CommandParserError::Command)
            } else {
                Ok(commands::TerminateCall::new().into())
            }
        }

        pub fn get_help(&self) -> &str {
            "terminate call"
        }
    }

    impl From<TerminateCall> for CommandParser {
        fn from(value: TerminateCall) -> Self {
            CommandParser::TerminateCall(value)
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
