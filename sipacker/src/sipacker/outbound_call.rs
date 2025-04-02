use crate::sipacker::user_agent::UserAgentEvent;

use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use ezk_sip::{Call, CallEvent, Codec, MediaSession, RtpReceiver, RtpSender};
use tokio::{select, sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

pub struct OutboundCall {
    audio_sender: Option<mpsc::Sender<Bytes>>,
    audio_receiver: Option<mpsc::Receiver<Bytes>>,
    calling_task: Option<JoinHandle<Result<Call<MediaSession>>>>,
    cancellation: CancellationToken,
    call: Option<Call<MediaSession>>,
    sender_task: Option<JoinHandle<()>>,
    receiver_task: Option<JoinHandle<()>>,
}

impl OutboundCall {
    pub fn new(
        out_call: ezk_sip::OutboundCall<MediaSession>,
        audio_sender: mpsc::Sender<Bytes>,
        audio_receiver: mpsc::Receiver<Bytes>,
        waiting_timeout: Duration,
    ) -> Self {
        let cancellation = CancellationToken::new();
        let calling_task = tokio::spawn(Self::run_calling_task(
            out_call,
            cancellation.clone(),
            waiting_timeout,
        ));
        let calling_task = Some(calling_task);
        let audio_sender = Some(audio_sender);
        let audio_receiver = Some(audio_receiver);
        Self {
            audio_sender,
            audio_receiver,
            calling_task,
            cancellation,
            call: None,
            sender_task: None,
            receiver_task: None,
        }
    }

    async fn run_calling_task(
        mut out_call: ezk_sip::OutboundCall<MediaSession>,
        cancellation: CancellationToken,
        waiting_duration: Duration,
    ) -> Result<Call<MediaSession>> {
        let completed_call = select! {
            _ = cancellation.cancelled() => Err(anyhow::Error::msg("Outbound call is cancelled")),
            _ = tokio::time::sleep(waiting_duration) => Err(anyhow::Error::msg("Outbound call is timed out")),
            completed = out_call.wait_for_completion() => {
                completed.map_err(|err| anyhow::Error::msg(err.to_string()))
            }
        };

        if completed_call.is_err() {
            out_call.cancel().await?;
        }
        let completed_call = completed_call?;

        select! {
            _ = cancellation.cancelled() => Err(anyhow::Error::msg("Outbound call is cancelled")),
            call = completed_call.finish() => call.map_err(|err| anyhow::Error::msg(err.to_string())),
        }
    }

    pub async fn cancel(&mut self) {
        if let Some(calling_task) = self.calling_task.take() {
            self.cancellation.cancel();
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

        if let Some(call) = self.call.take() {
            let _ = call.terminate().await;
        }
    }

    pub async fn run(&mut self) -> Result<Option<UserAgentEvent>> {
        if self.calling_task.as_ref().is_some_and(|t| t.is_finished()) {
            let call = self.calling_task.take().unwrap().await??;
            self.call = Some(call);
            Ok(Some(UserAgentEvent::CallEstablished))
        } else if let Some(call) = self.call.as_mut() {
            match call.run().await? {
                CallEvent::Media(event) => {
                    match event {
                        ezk_sip::MediaEvent::SenderAdded { sender, codec } => {
                            self.run_sender_task(sender, codec);
                        }
                        ezk_sip::MediaEvent::ReceiverAdded { receiver, codec } => {
                            self.run_receiver_task(receiver, codec);
                        }
                    }
                    Ok(None)
                }
                CallEvent::Terminated => {
                    self.cancel().await;
                    Ok(Some(UserAgentEvent::CallTerminated))
                }
            }
        } else {
            Ok(None)
        }
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