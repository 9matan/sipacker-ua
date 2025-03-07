use crate::sipacker::user_agent_event::*;

use bytes::Bytes;
use bytesstr::BytesStr;
use ezk_session::AsyncSdpSession;
use ezk_sip_core::{Endpoint, IncomingRequest, Layer, MayTake, Result};
use ezk_sip_types::header::typed::Contact;
use ezk_sip_types::{Name, StatusCode};
use ezk_sip_ua::dialog::Dialog;
use ezk_sip_ua::invite::acceptor::InviteAcceptor;
use ezk_sip_ua::invite::session::{InviteSession, InviteSessionEvent};
use simple_error::SimpleError;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const WAIT_FOR_ACTION_TIMEOUT_S: usize = 10;

pub struct InviteAcceptLayer {
    inner: Mutex<InviteAcceptLayerInner>,
    sdp_session: Arc<Mutex<AsyncSdpSession>>,
}

struct InviteAcceptLayerInner {
    invite_action: Option<InviteAction>,
    have_active_incoming: bool,
    outgoing_contact: Option<Contact>,
    event_sender: tokio::sync::mpsc::Sender<UserAgentEvent>,
}

#[derive(Clone)]
#[non_exhaustive]
pub enum InviteAction {
    Accept,
    Reject,
}

type DynResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

impl InviteAcceptLayer {
    pub fn new(
        sdp_session: Arc<Mutex<AsyncSdpSession>>,
        event_sender: tokio::sync::mpsc::Sender<UserAgentEvent>,
    ) -> Self {
        let inner = Mutex::new(InviteAcceptLayerInner {
            invite_action: None,
            have_active_incoming: false,
            outgoing_contact: None,
            event_sender,
        });
        Self { sdp_session, inner }
    }

    pub async fn set_invite_action(&self, invite_action: InviteAction) {
        self.inner.lock().await.invite_action = Some(invite_action);
    }

    pub async fn set_outgoing_contact(&self, outgoing_contact: Contact) {
        self.inner.lock().await.outgoing_contact = Some(outgoing_contact);
    }

    async fn handle_invite_req(
        &self,
        endpoint: &Endpoint,
        invite_req: IncomingRequest,
    ) -> DynResult<()> {
        let mut inner = self.inner.lock().await;
        let outgoing_contact = match &inner.outgoing_contact {
            Some(contact) => contact.clone(),
            None => {
                log::info!("The outgoing contact is not set. Skip the invite message");
                return Ok(());
            }
        };

        if inner.have_active_incoming {
            log::info!("There is an active incoming already. Skip the invite message");
            return Ok(());
        }

        inner.have_active_incoming = true;
        inner.invite_action = None;

        let incoming_event = data::IncomingCall {
            incoming_client: invite_req.base_headers.from.clone(),
        }
        .into();
        inner.event_sender.send(incoming_event).await?;
        std::mem::drop(inner);

        let invite_body = invite_req.body.clone();
        let dialog = Dialog::new_server(endpoint.clone(), &invite_req, outgoing_contact.clone())?;
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
                    println!("InviteSessionEvent::Bye");
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
