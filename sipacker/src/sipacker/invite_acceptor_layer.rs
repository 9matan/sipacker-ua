use std::default;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use bytesstr::BytesStr;
use ezk_session::{AsyncSdpSession, Codec, Codecs};
use ezk_sip_core::transport::udp::Udp;
use ezk_sip_core::{Endpoint, IncomingRequest, Layer, MayTake, Request, Result};
use ezk_sip_types::header::typed::{Contact, FromTo};
use ezk_sip_types::uri::sip::SipUri;
use ezk_sip_types::uri::NameAddr;
use ezk_sip_types::{Method, Name, StatusCode};
use ezk_sip_ua::dialog::{Dialog, DialogLayer};
use ezk_sip_ua::invite::acceptor::InviteAcceptor;
use ezk_sip_ua::invite::session::{InviteSession, InviteSessionEvent};
use ezk_sip_ua::invite::InviteLayer;
use simple_error::SimpleError;
use tokio::sync::Mutex;

const WAIT_FOR_ACTION_TIMEOUT_S: usize = 10;

pub struct InviteAcceptLayer {
    inner: Mutex<InviteAcceptLayerInner>,
    sdp_session: Arc<Mutex<AsyncSdpSession>>,
}

#[derive(Default)]
struct InviteAcceptLayerInner {
    invite_action: Option<InviteAction>,
    incoming_from: Option<FromTo>,
    outgoing_contact: Option<Contact>,
}

#[derive(Clone)]
#[non_exhaustive]
pub enum InviteAction {
    Accept,
    Reject,
}

type DynResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

impl InviteAcceptLayer {
    pub fn new(sdp_session: Arc<Mutex<AsyncSdpSession>>) -> Self {
        let inner = Mutex::default();
        Self { sdp_session, inner }
    }

    pub async fn set_invite_action(&self, invite_action: InviteAction) {
        self.inner.lock().await.invite_action = Some(invite_action);
    }

    pub async fn set_outgoing_contact(&self, outgoing_contact: Contact) {
        self.inner.lock().await.outgoing_contact = Some(outgoing_contact);
    }

    pub async fn incoming_from(&self) -> Option<FromTo> {
        self.inner.lock().await.incoming_from.clone()
    }

    async fn outgoing_contact(&self) -> Option<Contact> {
        self.inner.lock().await.outgoing_contact.clone()
    }

    async fn has_active_incoming(&self) -> bool {
        self.inner.lock().await.incoming_from.is_some()
    }

    async fn handle_invite_req(
        &self,
        endpoint: &Endpoint,
        invite_req: IncomingRequest,
    ) -> DynResult<()> {
        let outgoing_contact = match self.outgoing_contact().await {
            Some(contact) => contact,
            None => {
                log::info!("The outgoing contact is not set. Skip the invite message");
                return Ok(());
            }
        };

        if self.has_active_incoming().await {
            log::info!("There is an active incoming already. Skip the invite message");
            return Ok(());
        }

        async {
            let mut inner = self.inner.lock().await;
            inner.incoming_from = Some(invite_req.base_headers.from.clone());
            inner.invite_action = None;
        }
        .await;

        let invite_body = invite_req.body.clone();
        let dialog = Dialog::new_server(endpoint.clone(), &invite_req, outgoing_contact)?;
        let mut acceptor = InviteAcceptor::new(dialog, invite_req);

        let action = self.wait_for_invite_action(&mut acceptor).await?;

        if let InviteAction::Accept = action {
            let (session, _req) = self.send_ok_response(invite_body, acceptor).await?;
            self.handle_invite_session(&endpoint, session).await?;
        }

        Ok(())
    }

    async fn wait_for_invite_action(
        &self,
        acceptor: &mut InviteAcceptor,
    ) -> DynResult<InviteAction> {
        for _i in 0..WAIT_FOR_ACTION_TIMEOUT_S {
            match self.inner.lock().await.invite_action.clone() {
                None => {
                    let response = acceptor
                        .create_response(StatusCode::RINGING, None)
                        .await
                        .unwrap();
                    acceptor.respond_provisional(response).await.unwrap();
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
                Some(action) => return Ok(action),
            }
        }

        Err(Box::new(SimpleError::new(
            "Waiting for the invite action is timed out",
        )))
    }

    async fn handle_invite_session(
        &self,
        endpoint: &Endpoint,
        mut session: InviteSession,
    ) -> DynResult<()> {
        loop {
            match session.drive().await.unwrap() {
                InviteSessionEvent::RefreshNeeded(event) => {
                    event.process_default().await.unwrap();
                }
                InviteSessionEvent::ReInviteReceived(event) => {
                    let response = endpoint.create_response(&event.invite, StatusCode::OK, None);

                    event.respond_success(response).await.unwrap();
                }
                InviteSessionEvent::Bye(event) => {
                    event.process_default().await.unwrap();
                }
                InviteSessionEvent::Terminated => {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        Ok(())
    }

    async fn send_ok_response(
        &self,
        invite_body: Bytes,
        acceptor: InviteAcceptor,
    ) -> DynResult<(InviteSession, IncomingRequest)> {
        let mut response = acceptor
            .create_response(StatusCode::OK, None)
            .await
            .unwrap();

        let asnwer = {
            let mut sdp_session = self.sdp_session.lock().await;
            let invite_bytestr: BytesStr = BytesStr::from_utf8_bytes(invite_body).unwrap();
            let invite_sdp = ezk_sdp_types::SessionDescription::parse(&invite_bytestr).unwrap();

            let mut sdp_answer = sdp_session.receive_sdp_offer(invite_sdp).await.unwrap();
            sdp_answer.origin.username = "sipacker-ua 0.1.0".into();
            sdp_answer
        };

        response
            .msg
            .headers
            .insert(Name::CONTENT_TYPE, "application/sdp");
        response.msg.body = Bytes::copy_from_slice(asnwer.to_string().as_bytes());
        let res = acceptor.respond_success(response).await?;
        Ok(res)
    }

    async fn handle_bye_req(&self, endpoint: &Endpoint, request: IncomingRequest) -> DynResult<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl Layer for InviteAcceptLayer {
    fn name(&self) -> &'static str {
        "invite-accept-layer"
    }

    async fn receive(&self, endpoint: &Endpoint, request: MayTake<'_, IncomingRequest>) {
        let _res: Result<(), Box<dyn std::error::Error + Send + Sync>> =
            match request.line.method.to_string().as_str() {
                "INVITE" => self.handle_invite_req(endpoint, request.take()).await,
                "BYE" => self.handle_bye_req(endpoint, request.take()).await,
                _ => Ok(()),
            };
    }
}
