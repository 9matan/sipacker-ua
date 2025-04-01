use std::net::{IpAddr, SocketAddr};

use anyhow::Result;
use bytes::Bytes;
use ezk_rtc::AsyncSdpSession;
use ezk_rtc_proto::{BundlePolicy, Options, RtcpMuxPolicy, TransportType};
use ezk_sip::{Client, MediaSession, RegistrarConfig, Registration};
use ezk_sip_auth::{DigestAuthenticator, DigestCredentials};
use tokio::sync::mpsc;

pub struct UserAgent {
    sip_client: Client,
    ip_addr: IpAddr,
    reg_data: Option<RegData>,
    out_call: Option<out_call::OutCall>,
}

struct RegData {
    pub registration: Registration,
    pub credentials: DigestCredentials,
    pub registrar_socket: SocketAddr,
    pub user_name: String,
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
            reg_data: None,
            out_call: None,
        })
    }

    pub async fn register(
        &mut self,
        user_name: &str,
        registrar_socket: SocketAddr,
        credentials: DigestCredentials,
    ) -> Result<()> {
        let registrar = misc::make_sip_uri(&user_name, &registrar_socket)?;
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
            user_name: user_name,
        };
        self.reg_data = Some(reg_data);
        Ok(())
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

        let target = misc::make_sip_uri(&target_user_name, &reg_data.registrar_socket)?;
        let authenticator = reg_data.create_authenticator();
        let media = self.create_media()?;
        let out_call = reg_data
            .registration
            .make_call(target, authenticator, media)
            .await?;
        let out_call = out_call::OutCall::new(out_call, audio_sender, audio_receiver);
        self.out_call = Some(out_call);

        Ok(())
    }

    pub async fn run(&mut self) -> Result<()> {
        if let Some(call) = self.out_call.as_mut() {
            let is_finished = call.run().await?;
            if is_finished {
                self.out_call.take();
            }
            Ok(())
        } else {
            Ok(())
        }
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
            .ok_or(anyhow::Error::msg("Could not create Audio media"))?;
        sdp_session.add_media(audio_media_id, ezk_rtc_proto::Direction::SendRecv);

        Ok(MediaSession::new(sdp_session))
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

mod rtp {
    use bytes::Bytes;
    use ezk_rtp::{RtpExtensions, RtpPacket, RtpTimestamp, SequenceNumber, Ssrc};

    pub struct RtpFactory {
        rtp_sequence_number: SequenceNumber,
        rtp_timestamp: RtpTimestamp,
        rtp_pt: u8,
    }

    impl RtpFactory {
        pub fn new(rtp_pt: u8) -> Self {
            Self {
                rtp_sequence_number: SequenceNumber(0),
                rtp_timestamp: RtpTimestamp(0),
                rtp_pt,
            }
        }

        pub fn create_rtp_packet(&mut self, payload: Bytes) -> RtpPacket {
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
            packet
        }
    }
}

mod out_call {
    use super::rtp;

    use std::time::Duration;

    use anyhow::Result;
    use bytes::Bytes;
    use ezk_rtp::RtpPacket;
    use ezk_sip::{Call, CallEvent, Codec, MediaSession, RtpReceiver, RtpSender};
    use tokio::{sync::mpsc, task::JoinHandle};

    pub struct OutCall {
        audio_sender: Option<mpsc::Sender<Bytes>>,
        audio_receiver: Option<mpsc::Receiver<Bytes>>,
        calling_task: Option<JoinHandle<Result<Call<MediaSession>>>>,
        call: Option<Call<MediaSession>>,
        sender_task: Option<JoinHandle<()>>,
        receiver_task: Option<JoinHandle<()>>,
    }

    impl OutCall {
        pub fn new(
            out_call: ezk_sip::OutboundCall<MediaSession>,
            audio_sender: mpsc::Sender<Bytes>,
            audio_receiver: mpsc::Receiver<Bytes>,
        ) -> Self {
            let calling_task = tokio::spawn(Self::run_calling_task(out_call));
            let calling_task = Some(calling_task);
            let audio_sender = Some(audio_sender);
            let audio_receiver = Some(audio_receiver);
            Self {
                audio_sender,
                audio_receiver,
                calling_task,
                call: None,
                sender_task: None,
                receiver_task: None,
            }
        }

        async fn run_calling_task(
            mut out_call: ezk_sip::OutboundCall<MediaSession>,
        ) -> Result<Call<MediaSession>> {
            let completed_call =
                match tokio::time::timeout(Duration::from_secs(10), out_call.wait_for_completion())
                    .await
                {
                    Ok(completed) => completed.map_err(|err| anyhow::Error::msg(err.to_string())),
                    Err(_) => {
                        out_call.cancel().await?;
                        Err(anyhow::Error::msg("Outbound call is timed out"))
                    }
                }?;

            completed_call
                .finish()
                .await
                .map_err(|err| anyhow::Error::msg(err.to_string()))
        }

        pub async fn cancel(&mut self) {
            if let Some(calling_task) = self.calling_task.take() {
                calling_task.abort();
                let _ = calling_task.await;
            }

            if let Some(sender_task) = self.sender_task.take() {
                sender_task.abort();
                let _ = sender_task.await;
            }

            if let Some(receiver_task) = self.receiver_task.take() {
                receiver_task.abort();
                let _ = receiver_task.await;
            }
        }

        pub async fn run(&mut self) -> Result<bool> {
            if self.calling_task.as_ref().is_some_and(|t| t.is_finished()) {
                let call = self.calling_task.take().unwrap().await??;
                self.call = Some(call);
            } else if let Some(call) = self.call.as_mut() {
                match call.run().await? {
                    CallEvent::Media(event) => match event {
                        ezk_sip::MediaEvent::SenderAdded { sender, codec } => {
                            self.run_sender_task(sender, codec);
                        }
                        ezk_sip::MediaEvent::ReceiverAdded { receiver, codec } => {
                            self.run_receiver_task(receiver, codec);
                        }
                    },
                    CallEvent::Terminated => {
                        self.cancel().await;
                        return Ok(true);
                    }
                }
            }

            Ok(false)
        }

        fn run_sender_task(&mut self, mut sender: RtpSender, codec: Codec) {
            let mut audio_receiver = self.audio_receiver.take().unwrap();
            let mut rtp_factory = rtp::RtpFactory::new(codec.pt);
            let sender_task = tokio::spawn(async move {
                loop {
                    if let Some(payload) = audio_receiver.recv().await {
                        let packet = rtp_factory.create_rtp_packet(payload);
                        if let Err(_) = sender.send(packet).await {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            });
            self.sender_task = Some(sender_task);
        }

        fn run_receiver_task(&mut self, mut receiver: RtpReceiver, _codec: Codec) {
            let audio_sender = self.audio_sender.take().unwrap();
            let receiver_task = tokio::spawn(async move {
                loop {
                    if let Some(packet) = receiver.recv().await {
                        let _ = audio_sender.try_send(packet.payload);
                    } else {
                        break;
                    }
                }
            });
            self.receiver_task = Some(receiver_task);
        }
    }
}
