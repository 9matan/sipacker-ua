# Sipacker-UA
Rust CLI SIP User Agent. The agent is based on [ezk project](https://github.com/kbalt/ezk). Thank developers a lot!

The app has been tested with:
- SIP server: FreePBX 16.0.33
- Softphones: "MizuDroid" and "MicroSIP"
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

## Known issues
- Invitation (calling) does not work if the authentication is required on the SIP proxy (if a password is set on the SIP server).
- The outbound call in the calling state can't be terminated with the "terminate call" command.
- The audio channel is noisy

## Next steps
- Implement handling of an incoming call (WIP).
- Implement multi-codecs support:
  - G.711 mulaw
  - G.722
- Add the terminal UI
