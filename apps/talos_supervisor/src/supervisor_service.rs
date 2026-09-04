use std::{
    ffi::OsString,
    sync::mpsc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

const SERVICE_NAME: &str = "TalosSupervisor";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

pub(crate) fn run() -> windows_service::Result<()> {
    crate::write_bootstrap_log("service_dispatcher_start", Some(SERVICE_NAME));
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

define_windows_service!(ffi_service_main, talos_supervisor_service_main);

fn talos_supervisor_service_main(_arguments: Vec<OsString>) {
    crate::write_bootstrap_log("service_main_enter", None);
    if let Err(err) = run_service() {
        crate::write_bootstrap_log("service_main_err", Some(&err.to_string()));
        tracing::error!(error = %err, "Talos Supervisor service failed");
    }
}

fn run_service() -> anyhow::Result<()> {
    let shutting_down = Arc::new(AtomicBool::new(false));
    let handler_shutdown = shutting_down.clone();
    let (stop_tx, stop_rx) = mpsc::channel();
    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop => {
                handler_shutdown.store(true, Ordering::SeqCst);
                let _ = stop_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;
    crate::write_bootstrap_log("service_control_handler_registered", None);
    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;
    crate::write_bootstrap_log("service_status_running", None);

    let stop_status_handle = status_handle.clone();
    std::thread::spawn(move || {
        if stop_rx.recv().is_ok() {
            let _ = stop_status_handle.set_service_status(ServiceStatus {
                service_type: SERVICE_TYPE,
                current_state: ServiceState::StopPending,
                controls_accepted: ServiceControlAccept::empty(),
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 1,
                wait_hint: Duration::from_secs(30),
                process_id: None,
            });
            crate::write_bootstrap_log("service_status_stop_pending", None);
        }
    });

    let args = crate::supervisor::SupervisorArgs::parse(&[])?;
    let run_result = crate::supervisor::run_with_shutdown(args, shutting_down);
    if let Err(err) = &run_result {
        crate::write_bootstrap_log("service_run_err", Some(&err.to_string()));
    }

    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(if run_result.is_ok() { 0 } else { 1 }),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;
    crate::write_bootstrap_log("service_status_stopped", None);

    run_result
}
