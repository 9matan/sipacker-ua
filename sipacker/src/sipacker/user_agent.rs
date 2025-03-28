use crate::sipacker::invite_acceptor_layer::{InviteAcceptLayer, InviteAction};
use crate::sipacker::registrator::{RegistrationStatusKind, Registrator};
use crate::sipacker::user_agent_event::UserAgentEvent;

use ezk_rtp::RtpSession;
use ezk_session::{AsyncSdpSession, Codec, Codecs};
use ezk_sip_core::{transport::udp::Udp, Endpoint};
use ezk_sip_types::header::typed::Contact;
use ezk_sip_types::uri::sip::SipUri;
use ezk_sip_types::uri::NameAddr;
use ezk_sip_ua::{dialog::DialogLayer, invite::InviteLayer};
use simple_error::SimpleError;
use std::{error::Error, net::SocketAddr, sync::Arc, time::Duration};
use tokio::select;
use tokio::sync::Mutex;

type UAResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub struct UserAgent {
    sip_endpoint: Endpoint,
    registrator: Option<Arc<Registrator>>,
    sdp_session: Arc<Mutex<AsyncSdpSession>>,
    socketaddr: SocketAddr,
    event_receiver: tokio::sync::mpsc::Receiver<UserAgentEvent>,
    audio_player: AudioPlayer,
}

impl UserAgent {
    pub async fn build(socketaddr: SocketAddr) -> UAResult<UserAgent> {
        let options = ezk_session::Options {
            offer_transport: ezk_session::TransportType::Rtp,
            ..Default::default()
        };
        let mut sdp_session = AsyncSdpSession::new(socketaddr.ip(), options);
        let codecs = Codecs::new(ezk_session::MediaType::Audio).with_codec(Codec::PCMA);
        sdp_session
            .add_local_media(codecs, 1, ezk_session::Direction::SendRecv)
            .ok_or(SimpleError::new("Could not create a local media"))?;
        let sdp_session = Arc::new(Mutex::new(sdp_session));

        let (sender, receiver) = tokio::sync::mpsc::channel(20);
        let mut builder = Endpoint::builder();
        builder.add_layer(DialogLayer::default());
        builder.add_layer(InviteLayer::default());
        builder.add_layer(InviteAcceptLayer::new(Arc::clone(&sdp_session), sender));
        Udp::spawn(&mut builder, (socketaddr.ip(), socketaddr.port())).await?;
        let sip_endpoint = builder.build();

        let audio_player = AudioPlayer::build()?;
        sdp::run_sdp_event_handler(Arc::clone(&sdp_session), audio_player.clone_sink());

        Ok(UserAgent {
            sip_endpoint,
            sdp_session,
            registrator: None,
            socketaddr,
            event_receiver: receiver,
            audio_player,
        })
    }

    pub async fn register(&mut self, settings: registration::Settings) -> UAResult<()> {
        log::info!("Registering the UA ...");

        let contact = {
            let contact_ip = self.socketaddr.ip();
            let contact_port = self.socketaddr.port();
            let number = settings.extension_number.to_string();

            let contact = format!("sip:{number}@{contact_ip}:{contact_port}");
            let contact: SipUri = contact.parse()?;
            Contact::new(NameAddr::uri(contact))
        };

        let registrator =
            registration::build(self.sip_endpoint.clone(), settings, self.socketaddr).await?;
        Arc::clone(&registrator).run_registration().await;
        let reg_status = registrator.wait_for_registration_response().await;
        self.registrator = Some(registrator);

        let reason = reg_status.reason.as_str();
        match reg_status.kind {
            RegistrationStatusKind::Successful => {
                log::info!("The agent is successfuly registered");

                self.sip_endpoint
                    .layer::<InviteAcceptLayer>()
                    .set_outgoing_contact(contact)
                    .await;
            }
            RegistrationStatusKind::Unregistered => {
                log::warn!(reason:%; "The agent is unregistered");
            }
            RegistrationStatusKind::Failed => log::error!(reason:%; "The agent failed to register"),
        };

        Ok(())
    }

    pub async fn unregister(&mut self) {
        if let Some(registrator) = &self.registrator {
            Arc::clone(&registrator).stop_registration().await;
            self.registrator = None;
        }
    }

    pub async fn run(&mut self, timeout: Duration) -> Option<UserAgentEvent> {
        select! {
            event = self.event_receiver.recv() => {
                event
            }
            _ = tokio::time::sleep(timeout) => {
                None
            }
        }
    }

    pub async fn accept_incoming_call(&self) {
        self.sip_endpoint
            .layer::<InviteAcceptLayer>()
            .set_invite_action(InviteAction::Accept)
            .await;
    }
}

mod sdp {
    use super::*;
    use ezk_session::AsyncEvent;

    pub fn run_sdp_event_handler(
        sdp_session: Arc<Mutex<AsyncSdpSession>>,
        audio_sink: Arc<rodio::Sink>,
    ) {
        tokio::spawn(sdp_event_handler_task(sdp_session, audio_sink));
    }

    async fn sdp_event_handler_task(
        sdp_session: Arc<Mutex<AsyncSdpSession>>,
        audio_sink: Arc<rodio::Sink>,
    ) {
        loop {
            tokio::time::sleep(Duration::from_micros(10)).await;
            let mut sdp_session = sdp_session.lock().await;
            loop {
                let timeout = Duration::from_millis(10);
                select! {
                    event = sdp_session.run() => {
                        if let Ok(event) = event {
                            handle_sdp_event(event, &audio_sink);
                        }
                    }
                    _ = tokio::time::sleep(timeout) => {
                        break
                    }
                };
            }
        }
    }

    fn handle_sdp_event(event: ezk_session::AsyncEvent, audio_sink: &Arc<rodio::Sink>) {
        match event {
            AsyncEvent::MediaAdded(_event) => {}
            AsyncEvent::MediaRemoved(_) => {}
            AsyncEvent::ReceiveRTP { media_id, packet } => {
                println!(
                    "Packet: {:?} [{:?}] s: {}",
                    packet.sequence_number,
                    packet.timestamp,
                    packet.payload.len(),
                );
                let data = packet
                    .payload
                    .iter()
                    .map(|&b| ezk_g711::alaw::decode(b))
                    .collect::<Vec<_>>();

                let buffer = rodio::buffer::SamplesBuffer::new(1u16, 8000, data);
                audio_sink.append(buffer);
                // audio_sink.sleep_until_end();
            }
            _ => {}
        }
    }
}

pub mod registration {
    use super::*;
    use ezk_sip_types::{
        header::typed::Contact,
        uri::{sip::SipUri, NameAddr},
    };
    use ezk_sip_ua::register::Registration;
    use std::net::IpAddr;
    use typed_builder::TypedBuilder;

    #[non_exhaustive]
    #[derive(TypedBuilder, Clone)]
    pub struct Settings {
        #[builder(default = 5060)]
        pub sip_server_port: u16,
        pub sip_registrar_ip: IpAddr,
        pub extension_number: u64,
        #[builder(default=Duration::from_secs(600))]
        pub expiry: Duration,
    }

    pub(super) async fn build(
        endpoint: Endpoint,
        settings: Settings,
        agent_socketaddr: SocketAddr,
    ) -> Result<Arc<Registrator>, Box<dyn Error + Send + Sync>> {
        let contact_ip = agent_socketaddr.ip();
        let contact_port = agent_socketaddr.port();
        let number = settings.extension_number.to_string();
        let sip_ip = settings.sip_registrar_ip.to_string();
        let sip_port = settings.sip_server_port.to_string();

        let id = format!("sip:{number}@{contact_ip}");
        let contact = format!("sip:{number}@{contact_ip}:{contact_port}");
        let registrar = format!("sip:{number}@{sip_ip}:{sip_port}");
        log::debug!(id:%, contact:%, registrar:%; "Creating registrator");

        let id: SipUri = id.parse()?;
        let contact: SipUri = contact.parse()?;
        let registrar: SipUri = registrar.parse()?;

        let registration = Registration::new(
            NameAddr::uri(id),
            Contact::new(NameAddr::uri(contact)),
            registrar.into(),
            settings.expiry,
        );
        Ok(Registrator::new(endpoint, registration))
    }
}

pub struct AudioPlayer {
    _output_stream: rodio::OutputStream,
    _output_stream_h: rodio::OutputStreamHandle,
    sink: Arc<rodio::Sink>,
}

impl AudioPlayer {
    pub fn build() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let (_output_stream, _output_stream_h) = rodio::OutputStream::try_default()?;
        let sink = Arc::new(rodio::Sink::try_new(&_output_stream_h)?);
        Ok(Self {
            _output_stream,
            _output_stream_h,
            sink,
        })
    }

    pub fn clone_sink(&self) -> Arc<rodio::Sink> {
        Arc::clone(&self.sink)
    }
}
