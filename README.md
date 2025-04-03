# Sipacker-UA
Rust CLI SIP User Agent. The agent is based on [ezk project](https://github.com/kbalt/ezk). Thank developers a lot!

The app has been tested with:
- SIP server: FreePBX 16.0.33 (chan_sip driver)
- Softphones: "MizuDroid" (Android) and "MicroSIP" (Windows)
- Target OS: Ubuntu 24.04

## Prerequisites (Ubuntu 24.04)
- "cpal" crate requires ALSA (libasound2-dev package)
- "ezk" - libsrtp2-dev package
- Also, "ezk" requires OpenSSL to be installed

## Functionality
- Registering/unregistering on the SIP registrar
- Making a call by a user name (phone number)
- Terminating an active call
- Audio channel supports only PCMA (G.711 alaw) codec.

## Usage
1. Launch the program with `cargo run -- --ip-addr <agent ip addr>` (run `cargo run -- help` to see the available args)
1. We need to register the agent on the SIP server, execute the command in the app: `register user=<agent phone number> registrar=<IP addr of SIP>:<port of SIP>` (by default, a port is 5060, but for the chan_sip driver it is 5170)
1. Make a call to another agent: `call user=<another agent phone number>`
1. To get the list of available commands in the app, type `help`
1. Enjoy the noisy call =)

## Architecture
The project comprises the app's stuff (app folder) and user agent (sipacker).
### sipacker
- **AudioSystem** handles input and output streams (resampling, encoding/decoding). Data exchange is done with channels.
- **OutboundCall** establishes an outbound call and starts data exchange with audio channels.
- **UserAgent** represents a set of functionalities (registration, calling).
### app
- **CliInputSystem** handles stdin and sends commands to the application.
- **App** orchestrates everything (audio, commands, user agent).
- 
## Known issues
- Invitation (calling) does not work if the authentication is required on the SIP proxy (if a password is set on the SIP server).
- The outbound call in the calling state can't be terminated with the "terminate call" command.
- The audio channel is noisy

## Next steps
- Implement handling of an incoming call (WIP).
- Organize logging to files.
- Implement multi-codecs support:
  - G.711 ulaw
  - G.722
- Add the terminal UI.
