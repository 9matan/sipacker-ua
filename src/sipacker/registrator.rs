use ezk_sip_core::Endpoint;
use ezk_sip_core::{transaction::TsxResponse, transport::TargetTransportInfo};
use ezk_sip_types::msg::StatusLine;
use ezk_sip_types::{Code, CodeKind};
use ezk_sip_ua::register::Registration;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;
use tokio::{sync::Mutex, task::JoinHandle};

pub struct Registrator {
    endpoint: Endpoint,
    registration: Mutex<Registration>,
    reg_task: Mutex<Option<JoinHandle<()>>>,
    last_response_status: Mutex<Option<StatusLine>>,
}

impl Registrator {
    pub fn new(endpoint: Endpoint, registration: Registration) -> Arc<Self> {
        let registration = Mutex::new(registration);
        let r = Registrator {
            endpoint,
            registration,
            reg_task: Mutex::default(),
            last_response_status: Mutex::default(),
        };
        Arc::new(r)
    }

    pub async fn run_registration(self: Arc<Self>) {
        let mut self_reg_task = self.reg_task.lock().await;
        assert!(
            self_reg_task.is_none(),
            "Stop the registration before starting the new one"
        );

        let task = tokio::spawn(Arc::clone(&self).registering_task());
        *self_reg_task = Some(task);
    }

    async fn registering_task(self: Arc<Self>) {
        let mut target = TargetTransportInfo::default();
        loop {
            let res = self.registering_task_inner(&mut target).await;
            if let Err(err) = res {
                log::error!(err:%; "Error happened during the registration!");
            }
        }
    }

    async fn registering_task_inner(
        &self,
        target: &mut TargetTransportInfo,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut registration = self.registration.lock().await;
        let request = registration.create_register(false);
        let mut transaction = self.endpoint.send_request(request, target).await?;
        let response = transaction.receive_final().await?;

        self.set_last_response_status(Some(response.line.clone()))
            .await;
        match response.line.code.clone().kind() {
            CodeKind::Success => {
                registration.receive_success_response(response);
                registration.wait_for_expiry().await;
            }
            _ => {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }

        Ok(())
    }

    pub async fn stop_registration(&self) {
        let mut task = self.reg_task.lock().await;
        if let Some(task) = &mut (*task) {
            task.abort();
            self.set_last_response_status(None).await;
        }
    }

    pub async fn wait_for_registration_response(&self) -> RegistrationStatus {
        loop {
            let last_response_status = self.last_response_status.lock().await;
            match last_response_status.as_ref() {
                Some(status) => return status.clone().into(),
                None => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    }

    async fn set_last_response_status(&self, status: Option<StatusLine>) {
        *(self.last_response_status.lock().await) = status;
    }

    // async fn registration_status(&self) -> RegistrationStatus {
    //     let last_response_status = self.last_response_status.lock().await;
    //     (*last_response_status).as_ref().map(|sl| sl.code).into()
    // }
}

pub struct RegistrationStatus {
    pub kind: RegistrationStatusKind,
    pub reason: bytesstr::BytesStr,
}

impl From<StatusLine> for RegistrationStatus {
    fn from(value: StatusLine) -> Self {
        let kind = value.code.into();
        let reason = value.reason.unwrap_or_default();
        Self { kind, reason }
    }
}

#[non_exhaustive]
pub enum RegistrationStatusKind {
    Unregistered,
    Failed,
    Successful,
}

impl From<Code> for RegistrationStatusKind {
    fn from(value: Code) -> Self {
        match value.kind() {
            CodeKind::Success => RegistrationStatusKind::Successful,
            CodeKind::GlobalFailure => RegistrationStatusKind::Failed,
            CodeKind::RequestFailure => RegistrationStatusKind::Failed,
            CodeKind::ServerFailure => RegistrationStatusKind::Failed,
            _ => RegistrationStatusKind::Unregistered,
        }
    }
}
