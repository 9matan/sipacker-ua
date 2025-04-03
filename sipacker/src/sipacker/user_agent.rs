use crate::sipacker::outbound_call::OutboundCall;

use std::{
    collections::VecDeque,
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use anyhow::{Ok, Result};
use bytes::Bytes;
use ezk_rtc::AsyncSdpSession;
use ezk_rtc_proto::{BundlePolicy, Options, RtcpMuxPolicy, TransportType};
use ezk_sip::{Client, MediaSession, RegistrarConfig, Registration};
use ezk_sip_auth::{DigestAuthenticator, DigestCredentials};
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserAgentEvent {
    CallEstablished,
    Calling,
    CallTerminated,
    Registered,
    Unregistered,
}

pub struct UserAgent {
    sip_client: Client,
    ip_addr: IpAddr,
    events: VecDeque<UserAgentEvent>,
    reg_data: Option<RegData>,
    outbound_call: Option<OutboundCall>,
}

struct RegData {
    pub registration: Registration,
    pub credentials: DigestCredentials,
    pub registrar_socket: SocketAddr,
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
            outbound_call: None,
        })
    }

    pub fn is_registered(&self) -> bool {
        self.reg_data.is_some()
    }

    pub fn has_active_call(&self) -> bool {
        self.outbound_call.is_some()
    }

    pub async fn register(
        &mut self,
        user_name: &str,
        credentials: DigestCredentials,
        registrar_socket: SocketAddr,
    ) -> Result<()> {
        let registrar = misc::make_sip_uri(user_name, &registrar_socket)?;
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
            registrar_socket,
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

        let target = misc::make_sip_uri(target_user_name, &reg_data.registrar_socket)?;
        let authenticator = reg_data.create_authenticator();
        let media = self.create_media()?;
        let outbound_call = reg_data
            .registration
            .make_call(target, authenticator, media)
            .await?;
        let waiting_timeout = Duration::from_secs(10);
        let outbound_call =
            OutboundCall::new(outbound_call, audio_sender, audio_receiver, waiting_timeout);
        self.outbound_call = Some(outbound_call);

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

    pub async fn terminate_call(&mut self) -> Result<()> {
        if let Some(mut call) = self.outbound_call.take() {
            call.cancel().await;
            self.events.push_back(UserAgentEvent::CallTerminated);
        }
        Ok(())
    }

    pub async fn run(&mut self) -> Result<Option<UserAgentEvent>> {
        let event = self.events.pop_front();
        if event.is_some() {
            return Ok(event);
        }

        self.update_call().await;
        Ok(None)
    }

    async fn update_call(&mut self) {
        if let Some(call) = self.outbound_call.as_mut() {
            let event = call.run().await.inspect_err(|err| {
                tracing::warn!("Outbound call err: {err}");
            });

            let is_err = event.is_err();
            let event = event.unwrap_or(None);

            if event.is_some() {
                self.events.push_back(event.clone().unwrap());
            } else if is_err {
                self.events.push_back(UserAgentEvent::CallTerminated);
            }

            if is_err || event == Some(UserAgentEvent::CallTerminated) {
                self.outbound_call.take();
            }
        }
    }
}

impl RegData {
    fn create_authenticator(&self) -> DigestAuthenticator {
        DigestAuthenticator::new(self.credentials.clone())
    }
}

mod misc {
    use std::net::SocketAddr;

    use anyhow::Result;
    use ezk_sip_types::uri::sip::{InvalidSipUri, SipUri};

    pub fn make_sip_uri(user_name: &str, sip_socket: &SocketAddr) -> Result<SipUri> {
        format!(
            "sip:{}@{}:{}",
            user_name,
            sip_socket.ip(),
            sip_socket.port()
        )
        .parse()
        .map_err(|err: InvalidSipUri| anyhow::Error::msg(err.to_string()))
    }
}
