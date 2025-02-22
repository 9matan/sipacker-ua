

use ezk_sip_core::transport::tcp::TcpConnector;
use ezk_sip_core::transport::udp::Udp;
use ezk_sip_core::transport::TargetTransportInfo;
use ezk_sip_core::Endpoint;
use ezk_sip_types::uri::sip::SipUri;
use ezk_sip_types::uri::NameAddr;
use ezk_sip_types::CodeKind;
use ezk_sip_ua::register::Registration;
use tokio::{sync::Mutex, task::JoinHandle};
use std::{net::{IpAddr, Ipv4Addr}, sync::Arc};
use std::error::Error;
use std::time::Duration;
use log::{error, info};

pub struct Settings {
    pub sip_port: u16,
    pub sip_registrar_ip: IpAddr,
    pub contact_ip: IpAddr,
    pub extension_number: u64,
    pub expiry: Duration,
}

pub struct Registrator {
    sip_endpoint: Endpoint,
    registration: Mutex<Registration>,
    reg_task: Mutex<Option<JoinHandle<()>>>,
}

impl Registrator {
    pub async fn build(settings: Settings) -> Result<Arc<Self>, Box<dyn Error + Send + Sync>> {
        let mut builder = Endpoint::builder();
        Udp::spawn(&mut builder, (Ipv4Addr::from_bits(0), settings.sip_port)).await?;
        builder.add_transport_factory(Arc::new(TcpConnector::default()));
        let sip_endpoint = builder.build();

        let contact_ip = settings.contact_ip.to_string();
        let number = settings.extension_number.to_string();
        let sip_ip = settings.sip_registrar_ip.to_string();
        let sip_port = settings.sip_port.to_string();

        let id = format!("sip:{number}@{contact_ip}");
        let contact = format!("sip:{number}@{contact_ip}:{sip_port}");
        let registrar = format!("sip:{number}@{sip_ip}:{sip_port}");
        info!(id:%, contact:%, registrar:%; "Creating registrator");
        
        let id: SipUri = id.parse()?;
        let contact: SipUri = contact.parse()?;
        let registrar: SipUri = registrar.parse()?;
        
        let registration = Registration::new(
            NameAddr::uri(id),
            NameAddr::uri(contact),
            registrar.into(),
            settings.expiry,
        );
        let registration = tokio::sync::Mutex::new(registration);

        let r = Registrator { sip_endpoint, registration, reg_task: Mutex::default() };
        Ok(Arc::new(r))
    }

    pub async fn run_registration(self: Arc<Self>) {
        let task = tokio::spawn(Arc::clone(&self).registering_task());
        let mut self_reg_task = self.reg_task.lock().await;
        *self_reg_task = Some(task);
    }

    async fn registering_task(self: Arc<Self>) {
        let mut target = TargetTransportInfo::default();
        loop {
            let res = Arc::clone(&self).registering_task_inner(&mut target).await; 
            if let Err(_err) = res {
                error!("Unknown error happened during the registration!");
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }

    async fn registering_task_inner(self: Arc<Self>, target: &mut TargetTransportInfo) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut registration = self.registration.lock().await;
        let request = registration.create_register(false);
        let mut transaction = self.sip_endpoint.send_request(request, target).await?;
        let response = transaction.receive_final().await?;

        match response.line.code.kind() {
            CodeKind::Success => {
                registration.receive_success_response(response);
                registration.wait_for_expiry().await;
            }
            _ => {
                let reason = response.line.reason.clone().unwrap_or_default();
                error!(reason:%; "Registration failed");
            }
        }
        
        Ok(())
    }

    pub async fn stop_registration(self: Arc<Self>) {
        let mut task = self.reg_task.lock().await;
        if let Some(task) = &mut (*task) {
            task.abort();
        }
    }
}