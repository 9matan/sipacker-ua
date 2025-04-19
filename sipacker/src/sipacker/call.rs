use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use enum_dispatch::enum_dispatch;
use tokio::sync::mpsc;

type CallInner = ezk_sip::Call<ezk_sip::MediaSession>;

pub use incoming::DeclineCode;

pub mod states {
    pub use super::established::Established;
    pub use super::incoming::Incoming;
    pub use super::outgoing::Outgoing;

    pub mod incoming {
        pub use super::super::incoming::states::WaitingForAction;
        //pub use super::super::incoming::states::WaitingForActionResponse;
    }
}

use states::{Established, Incoming, Outgoing};

#[enum_dispatch()]
pub trait CallTrait {
    async fn run(self) -> Result<(Option<Call>, Option<CallEvent>)>;
    async fn terminate(self) -> Result<()>;
}

pub enum CallEvent {
    Established,
    Terminated,
}

#[enum_dispatch(CallTrait)]
pub enum Call {
    Incoming,
    Outgoing,
    Established,
}

impl Call {
    pub fn from_outgoing(
        outgoing_call: outgoing::OutgoingCallInner,
        audio_sender: mpsc::Sender<Bytes>,
        audio_receiver: mpsc::Receiver<Bytes>,
        waiting_timeout: Duration,
    ) -> Self {
        Outgoing::new(outgoing_call, audio_sender, audio_receiver, waiting_timeout).into()
    }

    pub fn from_incoming(incoming_call: incoming::IncomingCallInner) -> Self {
        Incoming::new(incoming_call).into()
    }

    pub fn as_incoming_waiting_for_action(
        self,
    ) -> Result<incoming::states::WaitingForAction, (Call, anyhow::Error)> {
        if let Call::Incoming(in_call) = self {
            if let Incoming::WaitingForAction(state) = in_call {
                return Ok(state);
            } else {
                Err((
                    in_call.into(),
                    anyhow::Error::msg("The incoming call is already handled"),
                ))
            }
        } else {
            Err((self, anyhow::Error::msg("There is no incoming call")))
        }
    }
}

mod outgoing {
    use super::states::Established;
    use super::{Call, CallEvent, CallInner, CallTrait};

    use std::time::Duration;

    use anyhow::Result;
    use bytes::Bytes;
    use tokio::{select, sync::mpsc, task::JoinHandle};
    use tokio_util::sync::CancellationToken;

    pub type OutgoingCallInner = ezk_sip::OutboundCall<ezk_sip::MediaSession>;

    pub struct Outgoing {
        audio_sender: mpsc::Sender<Bytes>,
        audio_receiver: mpsc::Receiver<Bytes>,
        calling_task: JoinHandle<Result<CallInner>>,
        cancellation: CancellationToken,
    }

    impl Outgoing {
        pub(super) fn new(
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
            mut outgoing_call: OutgoingCallInner,
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

    impl CallTrait for Outgoing {
        async fn run(self) -> Result<(Option<Call>, Option<CallEvent>)> {
            if self.calling_task.is_finished() {
                let call = self.calling_task.await??;
                let state = Established::new(call, self.audio_sender, self.audio_receiver);
                let event = Some(CallEvent::Established);
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
}

mod incoming {
    use super::states::Established;
    use super::{Call, CallEvent, CallTrait};

    use anyhow::Result;
    use bytes::Bytes;
    use bytesstr::BytesStr;
    use enum_dispatch::enum_dispatch;
    use ezk_sip_types::StatusCode;
    use tokio::{select, sync::mpsc, task::JoinHandle};
    use tokio_util::sync::CancellationToken;

    pub type IncomingCallInner = ezk_sip::IncomingCall<ezk_sip::MediaSession>;

    use states::{WaitingForAction, WaitingForActionResponse};

    #[enum_dispatch(CallTrait)]
    pub enum Incoming {
        WaitingForAction,
        WaitingForActionResponse,
    }

    pub enum DeclineCode {
        Busy,
        ServerInternalError,
        UserDeclined,
    }

    impl From<DeclineCode> for StatusCode {
        fn from(value: DeclineCode) -> Self {
            match value {
                DeclineCode::Busy => StatusCode::BUSY_HERE,
                DeclineCode::ServerInternalError => StatusCode::SERVER_INTERNAL_ERROR,
                DeclineCode::UserDeclined => StatusCode::DECLINE,
            }
        }
    }

    enum IncomingCallAction {
        Decline {
            code: DeclineCode,
            reason: String,
        },
        Accept {
            audio_sender: mpsc::Sender<Bytes>,
            audio_receiver: mpsc::Receiver<Bytes>,
        },
    }

    pub mod states {
        use super::*;

        pub struct WaitingForAction {
            calling_task: JoinHandle<Result<Option<Established>>>,
            cancellation: CancellationToken,
            action_sender: mpsc::Sender<IncomingCallAction>,
        }

        impl WaitingForAction {
            pub fn new(incoming_call: IncomingCallInner) -> Self {
                let cancellation = CancellationToken::new();
                let (action_sender, action_receiver) = mpsc::channel(1);
                let calling_task = tokio::spawn(Self::run_calling_task(
                    incoming_call,
                    cancellation.clone(),
                    action_receiver,
                ));
                Self {
                    calling_task,
                    cancellation,
                    action_sender,
                }
            }

            pub async fn send_accept(
                self,
                audio_sender: mpsc::Sender<Bytes>,
                audio_receiver: mpsc::Receiver<Bytes>,
            ) -> Result<Call> {
                self.send_action(IncomingCallAction::Accept {
                    audio_sender,
                    audio_receiver,
                })
                .await
            }

            pub async fn send_decline(self, code: DeclineCode, reason: &str) -> Result<Call> {
                self.send_action(IncomingCallAction::Decline {
                    code,
                    reason: reason.to_owned(),
                })
                .await
            }

            async fn send_action(self, action: IncomingCallAction) -> Result<Call> {
                self.action_sender.send(action).await?;

                let incoming = Incoming::from(WaitingForActionResponse::new(self.calling_task));
                Ok(incoming.into())
            }

            async fn run_calling_task(
                incoming_call: IncomingCallInner,
                cancellation: CancellationToken,
                mut action_receiver: mpsc::Receiver<IncomingCallAction>,
            ) -> Result<Option<Established>> {
                let action = select! {
                    action = action_receiver.recv() => action,
                    _ = cancellation.cancelled() => Some(
                        IncomingCallAction::Decline{ code: DeclineCode::UserDeclined, reason: "The call cancelled".to_owned()}
                    ),
                };

                match action {
                    Some(action) => Self::handle_action(incoming_call, action).await,
                    None => {
                        let err_msg = "Action channel is closed";
                        let _ = incoming_call
                            .decline(
                                StatusCode::SERVER_INTERNAL_ERROR,
                                BytesStr::from_static(err_msg).into(),
                            )
                            .await;
                        Err(anyhow::Error::msg(err_msg))
                    }
                }
            }

            async fn handle_action(
                incoming_call: IncomingCallInner,
                action: IncomingCallAction,
            ) -> Result<Option<Established>> {
                match action {
                    IncomingCallAction::Accept {
                        audio_sender,
                        audio_receiver,
                    } => {
                        let call = incoming_call.accept().await?;
                        let call = Established::new(call, audio_sender, audio_receiver);
                        Ok(Some(call))
                    }
                    IncomingCallAction::Decline { code, reason } => {
                        incoming_call
                            .decline(code.into(), BytesStr::from(reason).into())
                            .await?;
                        Ok(None)
                    }
                }
            }
        }

        impl CallTrait for WaitingForAction {
            async fn run(self) -> Result<(Option<Call>, Option<CallEvent>)> {
                if self.calling_task.is_finished() {
                    let call = self.calling_task.await??;
                    let event = match call {
                        Some(_) => CallEvent::Established,
                        None => CallEvent::Terminated,
                    };
                    Ok((call.map(|c| c.into()), Some(event)))
                } else {
                    let incoming = Incoming::from(self);
                    Ok((Some(incoming.into()), None))
                }
            }

            async fn terminate(self) -> Result<()> {
                self.cancellation.cancel();
                Ok(())
            }
        }

        pub struct WaitingForActionResponse {
            calling_task: JoinHandle<Result<Option<Established>>>,
        }

        impl WaitingForActionResponse {
            pub fn new(calling_task: JoinHandle<Result<Option<Established>>>) -> Self {
                Self { calling_task }
            }
        }

        impl CallTrait for WaitingForActionResponse {
            async fn run(self) -> Result<(Option<Call>, Option<CallEvent>)> {
                if self.calling_task.is_finished() {
                    let call = self.calling_task.await??;
                    let event = match call {
                        Some(_) => CallEvent::Established,
                        None => CallEvent::Terminated,
                    };
                    Ok((call.map(|c| c.into()), Some(event)))
                } else {
                    let incoming = Incoming::from(self);
                    Ok((Some(incoming.into()), None))
                }
            }

            async fn terminate(self) -> Result<()> {
                Ok(())
            }
        }
    }

    impl Incoming {
        pub fn new(incoming_call: IncomingCallInner) -> Self {
            WaitingForAction::new(incoming_call).into()
        }
    }
}

mod established {
    use super::rtp;
    use super::{Call, CallEvent, CallInner, CallTrait};

    use std::time::Duration;

    use anyhow::Result;
    use bytes::Bytes;
    use ezk_sip::{Codec, RtpReceiver, RtpSender};
    use tokio::{select, sync::mpsc, task::JoinHandle};

    pub struct Established {
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

    impl Established {
        pub(super) fn new(
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

    impl CallTrait for Established {
        async fn run(mut self) -> Result<(Option<Call>, Option<CallEvent>)> {
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
                        Ok((None, Some(CallEvent::Terminated)))
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
