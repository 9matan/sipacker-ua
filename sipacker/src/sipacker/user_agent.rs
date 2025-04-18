use crate::sipacker::call;

use std::{
    collections::VecDeque,
    net::{IpAddr, SocketAddr},
};

use anyhow::Result;
use bytes::Bytes;
use bytesstr::BytesStr;
use ezk_rtc::AsyncSdpSession;
use ezk_rtc_proto::{BundlePolicy, Options, RtcpMuxPolicy, TransportType};
use ezk_sip::{Client, MediaSession, RegistrarConfig, Registration};
use ezk_sip_auth::{DigestAuthenticator, DigestCredentials};
use ezk_sip_types::{header::typed::FromTo, host::HostPort, StatusCode};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum UserAgentEvent {
    CallEstablished,
    Calling,
    CallTerminated,
    IncomingCall(FromTo),
    Registered,
    Unregistered,
}

pub struct UserAgent {
    sip_client: Client,
    ip_addr: IpAddr,
    events: VecDeque<UserAgentEvent>,
    reg_data: Option<RegData>,
    call: Option<call::Call>,
    in_call_action_sender: Option<mpsc::Sender<call::IncomingCallAction>>,
}

struct RegData {
    pub registration: Registration,
    pub credentials: DigestCredentials,
    pub registrar_host: HostPort,
    pub _user_name: String,
}

impl UserAgent {
    pub async fn build(udp_socket: SocketAddr) -> Result<Self> {
        let ip_addr = udp_socket.ip();
        let sip_client = ezk_sip::ClientBuilder::new()
            .listen_udp(udp_socket)
            .build()
            .await?;

        Ok(Self {
            sip_client,
            ip_addr,
            events: VecDeque::new(),
            reg_data: None,
            call: None,
            in_call_action_sender: None,
        })
    }

    pub fn is_registered(&self) -> bool {
        self.reg_data.is_some()
    }

    pub fn has_active_call(&self) -> bool {
        self.call.is_some()
    }

    pub fn has_incoming_call(&self) -> bool {
        self.in_call_action_sender.is_some()
    }

    pub async fn register(
        &mut self,
        user_name: &str,
        credentials: DigestCredentials,
        registrar_host: HostPort,
    ) -> Result<()> {
        let registrar = misc::make_sip_uri(user_name, &registrar_host)?;
        let user_name = user_name.to_owned();
        let config = RegistrarConfig {
            registrar,
            username: user_name.clone(),
            override_contact: None,
            override_id: None,
        };
        let authenticator = DigestAuthenticator::new(credentials.clone());
        let registration = self
            .sip_client
            .register(config, authenticator)
            .await
            .map_err(|err| anyhow::Error::msg(err.to_string()))?;

        let reg_data = RegData {
            registration,
            credentials,
            registrar_host,
            _user_name: user_name,
        };
        self.reg_data = Some(reg_data);

        self.events.push_back(UserAgentEvent::Registered);
        Ok(())
    }

    pub fn unregister(&mut self) {
        self.reg_data.take();
        self.events.push_back(UserAgentEvent::Unregistered);
    }

    pub async fn make_call(
        &mut self,
        target_user_name: &str,
        audio_sender: mpsc::Sender<Bytes>,
        audio_receiver: mpsc::Receiver<Bytes>,
    ) -> Result<()> {
        let reg_data = self
            .reg_data
            .as_ref()
            .ok_or(anyhow::Error::msg("The user agent is not registered"))?;

        let target = misc::make_sip_uri(target_user_name, &reg_data.registrar_host)?;
        let authenticator = reg_data.create_authenticator();
        let media = self.create_media()?;
        let outbound_call = reg_data
            .registration
            .make_call(target, authenticator, media)
            .await?;
        let call = call::Call::from_outgoing(outbound_call, audio_sender, audio_receiver);
        self.call = Some(call);

        self.events.push_back(UserAgentEvent::Calling);
        Ok(())
    }

    fn create_media(&self) -> Result<MediaSession> {
        let options = Options {
            offer_transport: TransportType::Rtp,
            offer_ice: false,
            offer_avpf: false,
            rtcp_mux_policy: RtcpMuxPolicy::Negotiate,
            bundle_policy: BundlePolicy::MaxCompat,
        };
        let mut sdp_session = AsyncSdpSession::new(self.ip_addr, options);

        let audio_media_id = sdp_session
            .add_local_media(
                ezk_rtc_proto::Codecs::new(ezk_sdp_types::MediaType::Audio)
                    .with_codec(ezk_rtc_proto::Codec::PCMA),
                1,
                ezk_rtc_proto::Direction::SendRecv,
            )
            .ok_or(anyhow::Error::msg("Could not create audio media"))?;
        sdp_session.add_media(audio_media_id, ezk_rtc_proto::Direction::SendRecv);

        Ok(MediaSession::new(sdp_session))
    }

    pub async fn accept_incoming_call(
        &mut self,
        audio_sender: mpsc::Sender<Bytes>,
        audio_receiver: mpsc::Receiver<Bytes>,
    ) -> Result<()> {
        let sender = self
            .in_call_action_sender
            .take()
            .ok_or(anyhow::Error::msg("There is no incoming call to accept"))?;

        sender
            .send(call::IncomingCallAction::Accept {
                audio_sender,
                audio_receiver,
            })
            .await?;
        Ok(())
    }

    pub async fn decline_incoming_call(&mut self) -> Result<()> {
        let sender = self
            .in_call_action_sender
            .take()
            .ok_or(anyhow::Error::msg("There is no incoming call to decline"))?;

        sender.send(call::IncomingCallAction::Decline).await?;
        Ok(())
    }

    pub async fn terminate_call(&mut self) -> Result<()> {
        if let Some(call) = self.call.take() {
            call.terminate().await?;
            self.in_call_action_sender = None;
            self.events.push_back(UserAgentEvent::CallTerminated);
        }
        Ok(())
    }

    pub async fn run(&mut self) -> Result<Option<UserAgentEvent>> {
        let event = self.events.pop_front();
        if event.is_some() {
            return Ok(event);
        }

        self.handle_incoming_call_req().await?;
        self.update_call().await;
        Ok(None)
    }

    async fn handle_incoming_call_req(&mut self) -> Result<()> {
        if let Some(reg_data) = &mut self.reg_data {
            let result = self
                .sip_client
                .get_incoming_call(reg_data.registration.contact().clone())
                .await;
            if let Ok(Some((incoming_call, from))) = result {
                if self.has_active_call() {
                    tracing::debug!("Reject incoming call: there is the active call already");
                    let _ = incoming_call
                        .decline(
                            StatusCode::BUSY_HERE,
                            BytesStr::from("There is an active call").into(),
                        )
                        .await
                        .inspect_err(|err| {
                            tracing::warn!("Declining error: {err}");
                        });
                } else {
                    let (action_tx, action_rx) = mpsc::channel(1);
                    let incoming_call = incoming_call.with_media(self.create_media()?);
                    let call = call::Call::from_incoming(incoming_call, action_rx);
                    self.in_call_action_sender = Some(action_tx);
                    self.call = Some(call);
                    self.events.push_back(UserAgentEvent::IncomingCall(from));
                }
            }
        }

        Ok(())
    }

    async fn update_call(&mut self) {
        self.call = if let Some(call) = self.call.take() {
            let run_res = call.run().await.inspect_err(|err| {
                tracing::warn!("Call err: {err}");
            });

            let (call, event) = match run_res {
                Ok((call, event)) => {
                    let event = event.map(|event| match event {
                        call::Event::Established => UserAgentEvent::CallEstablished,
                        call::Event::Terminated => UserAgentEvent::CallTerminated,
                    });
                    (call, event)
                }
                Err(_err) => (None, Some(UserAgentEvent::CallTerminated)),
            };

            if let Some(event) = event {
                self.events.push_back(event);
            }

            call
        } else {
            None
        };

        if self.call.is_none() {
            self.in_call_action_sender = None;
        }
    }
}

impl RegData {
    fn create_authenticator(&self) -> DigestAuthenticator {
        DigestAuthenticator::new(self.credentials.clone())
    }
}

mod misc {
    use anyhow::Result;
    use ezk_sip_types::{
        host::HostPort,
        uri::sip::{InvalidSipUri, SipUri},
    };

    pub fn make_sip_uri(user_name: &str, sip_domain: &HostPort) -> Result<SipUri> {
        format!("sip:sip@{}", sip_domain.to_string(),)
            .parse()
            .map_err(|err: InvalidSipUri| anyhow::Error::msg(err.to_string()))
    }
}
