use std::default;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use bytesstr::BytesStr;
use ezk_session::{AsyncSdpSession, Codec, Codecs};
use ezk_sip_core::transport::udp::Udp;
use ezk_sip_core::{Endpoint, IncomingRequest, Layer, MayTake, Result};
use ezk_sip_types::header::typed::Contact;
use ezk_sip_types::uri::sip::SipUri;
use ezk_sip_types::uri::NameAddr;
use ezk_sip_types::{Method, Name, StatusCode};
use ezk_sip_ua::dialog::{Dialog, DialogLayer};
use ezk_sip_ua::invite::acceptor::InviteAcceptor;
use ezk_sip_ua::invite::session::InviteSessionEvent;
use ezk_sip_ua::invite::InviteLayer;
use tokio::sync::Mutex;

/// Custom layer which we use to accept incoming invites
pub struct InviteAcceptLayer {
    sdp_session: Arc<Mutex<AsyncSdpSession>>,
}

impl InviteAcceptLayer {
    pub fn new(sdp_session: Arc<Mutex<AsyncSdpSession>>) -> Self {
        Self { sdp_session }
    }
}

#[async_trait::async_trait]
impl Layer for InviteAcceptLayer {
    fn name(&self) -> &'static str {
        "invite-accept-layer"
    }

    async fn receive(&self, endpoint: &Endpoint, request: MayTake<'_, IncomingRequest>) {
        let invite = if request.line.method == Method::INVITE {
            request.take()
        } else {
            return;
        };

        let contact: SipUri = "sip:3333@192.168.3.14".parse().unwrap();
        let contact = Contact::new(NameAddr::uri(contact));

        // println!("{:#?}", invite.base_headers);
        // println!("{:#?}", invite.headers);
        let dialog = Dialog::new_server(endpoint.clone(), &invite, contact).unwrap();

        let invite_body = invite.body.clone();
        let mut acceptor = InviteAcceptor::new(dialog, invite);

        for _i in 0..10 {
            let response = acceptor
                .create_response(StatusCode::RINGING, None)
                .await
                .unwrap();
            acceptor.respond_provisional(response).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        let mut response = acceptor
            .create_response(StatusCode::OK, None)
            .await
            .unwrap();
        //println!("{:#?}", response);
        // sdp_session.set_transport_ports(transport_id, ip_addrs, rtp_port, rtcp_port);

        // sdp_session.
        // sdp_session.add_local_media(local_media_id, direction)
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
        // println!("BODY: {:#?}", response.msg.body);

        // Here goes SDP handling

        let (mut session, _ack) = acceptor.respond_success(response).await.unwrap();

        loop {
            let mut sdp_session = self.sdp_session.lock().await;
            sdp_session.run().await.unwrap();

            // match session.drive().await.unwrap() {
            //     InviteSessionEvent::RefreshNeeded(event) => {
            //         event.process_default().await.unwrap();
            //     }
            //     InviteSessionEvent::ReInviteReceived(event) => {
            //         let response = endpoint.create_response(&event.invite, StatusCode::OK, None);

            //         event.respond_success(response).await.unwrap();
            //     }
            //     InviteSessionEvent::Bye(event) => {
            //         event.process_default().await.unwrap();
            //     }
            //     InviteSessionEvent::Terminated => {
            //         break;
            //     }
            // }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}
