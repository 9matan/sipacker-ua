use crate::sipacker::audio;
use crate::sipacker::invite_acceptor_layer::{InviteAcceptLayer, InviteAction};
use crate::sipacker::registrator::{RegistrationStatusKind, Registrator};
use crate::sipacker::user_agent_event::UserAgentEvent;

use ezk_rtp::RtpPacket;
use ezk_session::{AsyncSdpSession, Codec, Codecs};
use ezk_sip_core::{transport::udp::Udp, Endpoint};
use ezk_sip_types::header::typed::Contact;
use ezk_sip_types::uri::sip::SipUri;
use ezk_sip_types::uri::NameAddr;
use ezk_sip_ua::{dialog::DialogLayer, invite::InviteLayer};
use simple_error::SimpleError;
use std::{error::Error, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{select, sync::mpsc, sync::Mutex};

type UAResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

static RTP_PAYLOAD_SIZE: usize = 160;

pub struct UserAgent {
    sip_endpoint: Endpoint,
    registrator: Option<Arc<Registrator>>,
    sdp_session: Arc<Mutex<AsyncSdpSession>>,
    socketaddr: SocketAddr,
    event_receiver: mpsc::Receiver<UserAgentEvent>,
}

impl UserAgent {
    pub async fn build(
        socketaddr: SocketAddr,
        media_sender: mpsc::Sender<bytes::Bytes>,
        media_receiver: mpsc::Receiver<bytes::Bytes>,
    ) -> UAResult<UserAgent> {
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

        let (event_sender, event_receiver) = mpsc::channel(20);
        let mut builder = Endpoint::builder();
        builder.add_layer(DialogLayer::default());
        builder.add_layer(InviteLayer::default());
        builder.add_layer(InviteAcceptLayer::new(
            Arc::clone(&sdp_session),
            event_sender,
        ));
        Udp::spawn(&mut builder, (socketaddr.ip(), socketaddr.port())).await?;
        let sip_endpoint = builder.build();

        sdp::run_sdp_handler(Arc::clone(&sdp_session), media_sender, media_receiver);

        Ok(UserAgent {
            sip_endpoint,
            sdp_session,
            registrator: None,
            socketaddr,
            event_receiver,
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
    use ezk_rtp::{RtpExtensions, RtpTimestamp, SequenceNumber, Ssrc};
    use ezk_session::{AsyncEvent, MediaId};

    pub fn run_sdp_handler(
        sdp_session: Arc<Mutex<AsyncSdpSession>>,
        media_sender: mpsc::Sender<bytes::Bytes>,
        media_receiver: mpsc::Receiver<bytes::Bytes>,
    ) {
        tokio::spawn(async {
            SDPHandler::new(media_sender, media_receiver)
                .run(sdp_session)
                .await;
        });
    }

    struct SDPHandler {
        media_sender: mpsc::Sender<bytes::Bytes>,
        media_receiver: mpsc::Receiver<bytes::Bytes>,
        rtp_sequence_number: SequenceNumber,
        rtp_timestamp: RtpTimestamp,
        rtp_pt: u8,
        active_media_id: Option<MediaId>,
    }

    impl SDPHandler {
        fn new(
            media_sender: mpsc::Sender<bytes::Bytes>,
            media_receiver: mpsc::Receiver<bytes::Bytes>,
        ) -> Self {
            Self {
                media_sender,
                media_receiver,
                rtp_sequence_number: SequenceNumber(0),
                rtp_timestamp: RtpTimestamp(0),
                rtp_pt: 8,
                active_media_id: None,
            }
        }

        fn has_active_media(&self) -> bool {
            self.active_media_id.is_some()
        }

        async fn run(&mut self, sdp_session: Arc<Mutex<AsyncSdpSession>>) {
            loop {
                tokio::time::sleep(Duration::from_micros(10)).await;
                let mut sdp_session = sdp_session.lock().await;
                loop {
                    let timeout = Duration::from_millis(10);
                    self.handle_input_media(&mut (*sdp_session));
                    select! {
                        event = sdp_session.run() => {
                            if let Ok(event) = event {
                                self.handle_sdp_event(event).await;
                            }
                        }
                        _ = tokio::time::sleep(timeout) => {
                            break
                        }
                    };
                }
            }
        }

        async fn handle_sdp_event(&mut self, event: ezk_session::AsyncEvent) {
            match event {
                AsyncEvent::MediaAdded(ev) => {
                    self.active_media_id = Some(ev.id);
                }
                AsyncEvent::MediaRemoved(_id) => {
                    self.active_media_id = None;
                }
                AsyncEvent::ReceiveRTP { media_id, packet } => {
                    let _ = self.media_sender.send(packet.payload).await;
                }
                _ => {}
            }
        }

        fn handle_input_media(&mut self, sdp_session: &mut AsyncSdpSession) {
            loop {
                let data = self.media_receiver.try_recv();
                if let Ok(mut data) = data {
                    if self.has_active_media() {
                        while data.len() > RTP_PAYLOAD_SIZE {
                            let data_chunk = data.split_to(RTP_PAYLOAD_SIZE);
                            self.create_and_send_rtp_packet(sdp_session, data_chunk);
                        }
                        self.create_and_send_rtp_packet(sdp_session, data);
                    }
                } else {
                    break;
                }
            }
        }

        fn create_and_send_rtp_packet(
            &mut self,
            sdp_session: &mut AsyncSdpSession,
            payload: bytes::Bytes,
        ) {
            let payload_len = payload.len();
            let packet = RtpPacket {
                pt: self.rtp_pt,
                sequence_number: self.rtp_sequence_number,
                timestamp: self.rtp_timestamp,
                payload: payload,
                ssrc: Ssrc(0),
                extensions: RtpExtensions::default(),
            };

            self.rtp_sequence_number = SequenceNumber(self.rtp_sequence_number.0 + 1);
            self.rtp_timestamp = RtpTimestamp(self.rtp_timestamp.0 + payload_len as u32);

            sdp_session.send_rtp(
                self.active_media_id.expect("Invalid active media id"),
                packet,
            );
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
