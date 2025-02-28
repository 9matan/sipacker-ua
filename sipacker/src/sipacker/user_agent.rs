use crate::sipacker::registrator::{RegistrationStatusKind, Registrator};
use ezk_sip_core::{
    transport::{tcp::TcpConnector, udp::Udp},
    Endpoint,
};
use ezk_sip_ua::{dialog::DialogLayer, invite::InviteLayer};
use std::{
    error::Error,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::net::ToSocketAddrs;

type UAResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub struct UserAgent {
    sip_endpoint: Endpoint,
    registrator: Option<Arc<Registrator>>,
    socketaddr: SocketAddr,
}

impl UserAgent {
    pub async fn build(socketaddr: SocketAddr) -> UAResult<UserAgent> {
        let mut builder = Endpoint::builder();
        // builder.add_layer(DialogLayer::default());
        // builder.add_layer(InviteLayer::default());
        Udp::spawn(&mut builder, (socketaddr.ip(), socketaddr.port())).await?;
        // builder.add_transport_factory(Arc::new(TcpConnector::default()));
        let sip_endpoint = builder.build();

        Ok(UserAgent {
            sip_endpoint,
            registrator: None,
            socketaddr,
        })
    }

    pub async fn register(&mut self, settings: registration::Settings) -> UAResult<()> {
        log::info!("Registering the UA ...");

        let registrator =
            registration::build(self.sip_endpoint.clone(), settings, self.socketaddr).await?;
        Arc::clone(&registrator).run_registration().await;
        let reg_status = registrator.wait_for_registration_response().await;
        self.registrator = Some(registrator);

        let reason = reg_status.reason.as_str();
        match reg_status.kind {
            RegistrationStatusKind::Successful => log::info!("The agent is successfuly registered"),
            RegistrationStatusKind::Unregistered => {
                log::warn!(reason:%; "The agent is unregistered")
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
