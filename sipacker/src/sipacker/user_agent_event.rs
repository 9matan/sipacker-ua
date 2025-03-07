pub struct UserAgentEvent {
    pub data: UserAgentEventData,
}

#[non_exhaustive]
pub enum UserAgentEventData {
    IncomingCall(data::IncomingCall),
    IncomingCallCancelled,
}

impl From<UserAgentEventData> for UserAgentEvent {
    fn from(value: UserAgentEventData) -> Self {
        Self { data: value }
    }
}

pub mod data {
    use super::*;
    use ezk_sip_types::header::typed::FromTo;

    pub struct IncomingCall {
        pub incoming_client: FromTo,
    }

    impl From<IncomingCall> for UserAgentEventData {
        fn from(value: IncomingCall) -> Self {
            UserAgentEventData::IncomingCall(value)
        }
    }

    impl From<IncomingCall> for UserAgentEvent {
        fn from(value: IncomingCall) -> Self {
            UserAgentEventData::from(value).into()
        }
    }
}
