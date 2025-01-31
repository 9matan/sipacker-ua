use ezk_sip_core as sip_core;
use ezk_sip_types::host::HostPort;
use ezk_sip_types as sip_types;
use ezk_sip_ua as sip_ua;

use sip_core::transport::tcp::TcpConnector;
use sip_core::transport::udp::Udp;
use sip_core::transport::TargetTransportInfo;
use sip_core::{Endpoint, Result};
use sip_types::uri::sip::SipUri;
use sip_types::uri::NameAddr;
use sip_types::CodeKind;
use sip_ua::register::Registration;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    // Create the endpoint
    let mut builder = Endpoint::builder();

    // Add a IPv4 UDP Socket
    Udp::spawn(&mut builder, "0.0.0.0:5170").await?;

    // Add a TCP connector
    builder.add_transport_factory(Arc::new(TcpConnector::default()));

    let endpoint = builder.build();

    let id: SipUri = "sip:2003@192.168.0.102".parse().unwrap();
    let contact: SipUri = "sip:2003@192.168.0.102:5170".parse().unwrap();
    let registrar: SipUri = "sip:192.168.0.90:5170".parse().unwrap();

    let mut target = TargetTransportInfo::default();
    //target.via_host_port = Some(SocketAddrV4::new(Ipv4Addr::new(192, 168, 0, 90), 5170).into());
    let mut registration = Registration::new(
        NameAddr::uri(id),
        NameAddr::uri(contact),
        registrar.into(),
        Duration::from_secs(600),
    );

    loop {
        let request = registration.create_register(false);
        let mut transaction = endpoint.send_request(request, &mut target).await?;
        let response = transaction.receive_final().await?;

        match response.line.code.kind() {
            CodeKind::Success => {}
            _ => panic!("registration failed!"),
        }

        registration.receive_success_response(response);

        registration.wait_for_expiry().await;
    }
}