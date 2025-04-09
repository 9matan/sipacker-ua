use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use bytesstr::BytesStr;
use enum_dispatch::enum_dispatch;
use ezk_sip::{Codec, MediaSession, RtpReceiver, RtpSender};
use ezk_sip_types::StatusCode;
use tokio::{select, sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

type CallInner = ezk_sip::Call<MediaSession>;
type IncomingCallInner = ezk_sip::IncomingCall<MediaSession>;
type OutgoingCallInner = ezk_sip::OutboundCall<MediaSession>;

pub struct Call {
    state: State,
}

impl Call {
    pub fn from_outgoing(
        outgoing_call: OutgoingCallInner,
        audio_sender: mpsc::Sender<Bytes>,
        audio_receiver: mpsc::Receiver<Bytes>,
    ) -> Self {
        let waiting_timeout = Duration::from_secs(10);
        let state = OutgoingCall::new(outgoing_call, audio_sender, audio_receiver, waiting_timeout);
        Self {
            state: state.into(),
        }
    }

    pub fn from_incoming(
        incoming_call: IncomingCallInner,
        action_receiver: mpsc::Receiver<IncomingCallAction>,
    ) -> Self {
        let state = IncomingCall::new(incoming_call, action_receiver);
        Self {
            state: state.into(),
        }
    }

    pub async fn run(self) -> Result<(Option<Self>, Option<Event>)> {
        let (state, event) = self.state.run().await?;
        Ok((state.map(|state| Self { state }), event))
    }

    pub async fn terminate(self) -> Result<()> {
        self.state.terminate().await
    }
}

pub enum Event {
    Established,
    Terminated,
}

#[enum_dispatch()]
trait StateTrait {
    async fn run(self) -> Result<(Option<State>, Option<Event>)>;
    async fn terminate(self) -> Result<()>;
}

#[enum_dispatch(StateTrait)]
enum State {
    IncomingCall,
    OutgoingCall,
    EstablishedCall,
}

struct OutgoingCall {
    audio_sender: mpsc::Sender<Bytes>,
    audio_receiver: mpsc::Receiver<Bytes>,
    calling_task: JoinHandle<Result<CallInner>>,
    cancellation: CancellationToken,
}

impl OutgoingCall {
    fn new(
        outgoing_call: OutgoingCallInner,
        audio_sender: mpsc::Sender<Bytes>,
        audio_receiver: mpsc::Receiver<Bytes>,
        waiting_timeout: Duration,
    ) -> Self {
        let cancellation = CancellationToken::new();
        let calling_task = tokio::spawn(Self::run_calling_task(
            outgoing_call,
            cancellation.clone(),
            waiting_timeout,
        ));
        Self {
            audio_sender,
            audio_receiver,
            calling_task,
            cancellation,
        }
    }

    async fn run_calling_task(
        mut outgoing_call: ezk_sip::OutboundCall<MediaSession>,
        cancellation: CancellationToken,
        waiting_duration: Duration,
    ) -> Result<CallInner> {
        let completed_call = select! {
            _ = cancellation.cancelled() => Err(anyhow::Error::msg("Outbound call is cancelled")),
            _ = tokio::time::sleep(waiting_duration) => Err(anyhow::Error::msg("Outbound call is timed out")),
            completed = outgoing_call.wait_for_completion() => {
                completed.map_err(|err| anyhow::Error::msg(err.to_string()))
            }
        };

        if completed_call.is_err() {
            outgoing_call.cancel().await?;
        }
        let completed_call = completed_call?;

        select! {
            _ = cancellation.cancelled() => Err(anyhow::Error::msg("Outbound call is cancelled")),
            call = completed_call.finish() => call.map_err(|err| anyhow::Error::msg(err.to_string())),
        }
    }
}

impl StateTrait for OutgoingCall {
    async fn run(self) -> Result<(Option<State>, Option<Event>)> {
        if self.calling_task.is_finished() {
            let call = self.calling_task.await??;
            let state = EstablishedCall::new(call, self.audio_sender, self.audio_receiver);
            let event = Some(Event::Established);
            Ok((Some(state.into()), event))
        } else {
            Ok((Some(self.into()), None))
        }
    }

    async fn terminate(self) -> Result<()> {
        self.cancellation.cancel();
        let _ = self.calling_task.await?;
        Ok(())
    }
}

struct IncomingCall {
    incoming_call: IncomingCallInner,
    action_receiver: mpsc::Receiver<IncomingCallAction>,
}

pub enum IncomingCallAction {
    Decline,
    Accept {
        audio_sender: mpsc::Sender<Bytes>,
        audio_receiver: mpsc::Receiver<Bytes>,
    },
}

impl IncomingCall {
    fn new(
        incoming_call: IncomingCallInner,
        action_receiver: mpsc::Receiver<IncomingCallAction>,
    ) -> Self {
        Self {
            incoming_call,
            action_receiver,
        }
    }

    async fn handle_action(self, action: IncomingCallAction) -> Result<(Option<State>, Event)> {
        match action {
            IncomingCallAction::Decline => {
                self.incoming_call
                    .decline(
                        StatusCode::DECLINE,
                        BytesStr::from_static("The call is declined").into(),
                    )
                    .await?;

                Ok((None, Event::Terminated))
            }
            IncomingCallAction::Accept {
                audio_sender,
                audio_receiver,
            } => {
                let call = self.incoming_call.accept().await?;
                let state = EstablishedCall::new(call, audio_sender, audio_receiver);
                Ok((Some(state.into()), Event::Established))
            }
        }
    }
}

impl StateTrait for IncomingCall {
    async fn run(mut self) -> Result<(Option<State>, Option<Event>)> {
        match self.action_receiver.try_recv() {
            Ok(action) => self
                .handle_action(action)
                .await
                .map(|(state, event)| (state, Some(event))),
            Err(err) => match err {
                mpsc::error::TryRecvError::Empty => Ok((Some(self.into()), None)),
                mpsc::error::TryRecvError::Disconnected => {
                    let _ = self
                        .incoming_call
                        .decline(
                            StatusCode::SERVER_INTERNAL_ERROR,
                            BytesStr::from(err.to_string().as_ref()).into(),
                        )
                        .await;
                    Err(err.into())
                }
            },
        }
    }

    async fn terminate(self) -> Result<()> {
        self.incoming_call
            .decline(
                StatusCode::DECLINE,
                BytesStr::from_static("The call is terminated").into(),
            )
            .await
            .map_err(|err| err.into())
    }
}

struct EstablishedCall {
    sending_channel: SendingChannel,
    receiving_channel: ReceivingChannel,
    call: CallInner,
}

enum SendingChannel {
    Waiting(mpsc::Receiver<Bytes>),
    Established(JoinHandle<()>),
}

enum ReceivingChannel {
    Waiting(mpsc::Sender<Bytes>),
    Established(JoinHandle<()>),
}

impl EstablishedCall {
    fn new(
        call: CallInner,
        audio_sender: mpsc::Sender<Bytes>,
        audio_receiver: mpsc::Receiver<Bytes>,
    ) -> Self {
        Self {
            call,
            sending_channel: SendingChannel::Waiting(audio_receiver),
            receiving_channel: ReceivingChannel::Waiting(audio_sender),
        }
    }

    fn run_sending_task(mut self, mut sender: RtpSender, codec: Codec) -> Self {
        self.sending_channel =
            if let SendingChannel::Waiting(mut audio_receiver) = self.sending_channel {
                let mut rtp_factory = rtp::RtpFactory::new(codec.pt);
                let sending_task = tokio::spawn(async move {
                    while let Some(payload) = audio_receiver.recv().await {
                        let packet = rtp_factory.create_rtp_packet(payload);
                        if sender.send(packet).await.is_err() {
                            break;
                        }
                    }
                });
                SendingChannel::Established(sending_task)
            } else {
                panic!("The sending channel must be in waiting state");
            };

        self
    }

    fn run_receiving_task(mut self, mut receiver: RtpReceiver, _codec: Codec) -> Self {
        self.receiving_channel =
            if let ReceivingChannel::Waiting(audio_sender) = self.receiving_channel {
                let receiver_task = tokio::spawn(async move {
                    while let Some(packet) = receiver.recv().await {
                        let _ = audio_sender.try_send(packet.payload);
                    }
                });
                ReceivingChannel::Established(receiver_task)
            } else {
                panic!("The receiving channel must be in waiting state");
            };

        self
    }
}

impl StateTrait for EstablishedCall {
    async fn run(mut self) -> Result<(Option<State>, Option<Event>)> {
        let run_res = select! {
            res = self.call.run() => res,
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                return Ok((Some(self.into()), None))
            }
        };

        match run_res {
            Ok(event) => match event {
                ezk_sip::CallEvent::Media(event) => {
                    let new_self = match event {
                        ezk_sip::MediaEvent::SenderAdded { sender, codec } => {
                            self.run_sending_task(sender, codec)
                        }
                        ezk_sip::MediaEvent::ReceiverAdded { receiver, codec } => {
                            self.run_receiving_task(receiver, codec)
                        }
                    };
                    Ok((Some(new_self.into()), None))
                }
                ezk_sip::CallEvent::Terminated => {
                    self.terminate().await?;
                    Ok((None, Some(Event::Terminated)))
                }
            },
            Err(err) => {
                self.terminate().await?;
                Err(err.into())
            }
        }
    }

    async fn terminate(self) -> Result<()> {
        self.call.terminate().await?;

        if let SendingChannel::Established(task) = self.sending_channel {
            task.abort();
            let _ = task.await;
        }

        if let ReceivingChannel::Established(task) = self.receiving_channel {
            task.abort();
            let _ = task.await;
        }

        Ok(())
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
                payload,
                ssrc: Ssrc(0),
                extensions: RtpExtensions::default(),
            };

            self.rtp_sequence_number = SequenceNumber(self.rtp_sequence_number.0 + 1);
            self.rtp_timestamp = RtpTimestamp(self.rtp_timestamp.0 + payload_len as u32);
            packet
        }
    }
}
