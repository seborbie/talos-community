use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::CString,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::sync_channel,
    },
    thread::{self, ThreadId},
};

use serde::de::DeserializeOwned;
use tauri::{AppHandle, Manager, Resource, ResourceId, Runtime, plugin::PluginApi};

use crate::Error;
use crate::PermissionFlowExt;
use crate::models::*;
use permission_flow::{
    AppPath, Permission as NativePermission, PermissionFlowController, StartFlowOptions,
};

// Native controllers must live on the Tauri main thread. The JS-facing handle
// points at an entry in this registry instead of owning the controller
// directly, which keeps the public API compatible with Tauri's resource model.
thread_local! {
    static CONTROLLERS: RefCell<HashMap<u64, PermissionFlowController>> = RefCell::new(HashMap::new());
}

/// Creates the desktop plugin state that backs the Tauri commands.
pub fn init<R: Runtime, C: DeserializeOwned>(
    app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> crate::Result<PermissionFlowPlugin<R>> {
    Ok(PermissionFlowPlugin {
        app: app.clone(),
        main_thread_id: thread::current().id(),
        next_controller_id: AtomicU64::new(1),
    })
}

/// Access to the desktop permission-flow APIs.
pub struct PermissionFlowPlugin<R: Runtime> {
    app: AppHandle<R>,
    main_thread_id: ThreadId,
    next_controller_id: AtomicU64,
}

impl<R: Runtime> PermissionFlowPlugin<R> {
    /// Allocates a new controller, stores it in the main-thread registry, and
    /// returns a JS-visible Tauri resource id for that controller.
    pub fn create(&self) -> crate::Result<ResourceId> {
        let controller_id = self.next_controller_id.fetch_add(1, Ordering::Relaxed);
        self.run_on_main_thread(move || {
            CONTROLLERS.with(|controllers| {
                let mut controllers = controllers.borrow_mut();
                controllers.insert(controller_id, PermissionFlowController::new()?);
                Ok(())
            })
        })?;

        let mut resources_table = self.app.resources_table();
        let rid = resources_table.add(PermissionFlowHandle {
            app: self.app.clone(),
            controller_id,
        });
        Ok(rid)
    }

    /// Starts the floating guidance flow for a specific controller handle.
    pub fn start_flow(&self, controller_id: u64, payload: StartFlowRequest) -> crate::Result<()> {
        self.run_on_main_thread(move || {
            CONTROLLERS.with(|controllers| {
                let controllers = controllers.borrow();
                let controller = controllers.get(&controller_id).ok_or(Error::DriverClosed)?;

                eprintln!(
                    "permission-flow-tauri start_flow_entered controller_id={} permission={:?} app_path={} use_click_source_frame={}",
                    controller_id,
                    payload.permission,
                    payload.app_path,
                    payload.use_click_source_frame
                );
                let app_path = CString::new(payload.app_path)?;
                let mut options =
                    StartFlowOptions::new(permission_to_native(payload.permission), app_path);
                if !payload.use_click_source_frame {
                    options = options.without_click_source_frame();
                }

                eprintln!(
                    "permission-flow-tauri start_flow_before_native controller_id={}",
                    controller_id
                );
                controller.start_flow(options)?;
                eprintln!(
                    "permission-flow-tauri start_flow_after_native controller_id={}",
                    controller_id
                );
                Ok(())
            })
        })
    }

    /// Stops the active floating guidance flow for a specific controller
    /// handle, if one is currently open.
    pub fn stop_current_flow(&self, controller_id: u64) -> crate::Result<()> {
        self.run_on_main_thread(move || {
            CONTROLLERS.with(|controllers| {
                let controllers = controllers.borrow();
                let controller = controllers.get(&controller_id).ok_or(Error::DriverClosed)?;

                controller.stop_current_flow()?;
                Ok(())
            })
        })
    }

    /// Returns the current host-app status for a permission. This path is
    /// stateless and does not require a controller handle.
    pub fn authorization_state(
        &self,
        permission: Permission,
    ) -> crate::Result<PermissionAuthorizationState> {
        Ok(permission_to_native(permission)
            .authorization_state()?
            .into())
    }

    /// Returns a best-effort guess for the host app bundle path in the current
    /// launch context.
    pub fn suggested_host_app_path(&self) -> crate::Result<Option<String>> {
        Ok(
            AppPath::suggested_host_app()
                .map(|path| path.as_c_str().to_string_lossy().into_owned()),
        )
    }

    /// Removes a controller from the registry when its JS-visible resource
    /// handle is closed or collected.
    fn drop_controller(&self, controller_id: u64) {
        let _ = self.run_on_main_thread(move || {
            CONTROLLERS.with(|controllers| {
                let mut controllers = controllers.borrow_mut();
                controllers.remove(&controller_id);
            });
            Ok(())
        });
    }

    /// Ensures controller work runs on the Tauri main thread, which matches
    /// the threading contract required by the underlying Rust and Swift layers.
    fn run_on_main_thread<T, F>(&self, action: F) -> crate::Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> crate::Result<T> + Send + 'static,
    {
        if thread::current().id() == self.main_thread_id {
            return action();
        }

        let (sender, receiver) = sync_channel(1);
        self.app.run_on_main_thread(move || {
            let _ = sender.send(action());
        })?;

        receiver
            .recv()
            .map_err(|_| Error::MainThreadChannelClosed)?
    }
}

/// A JS-visible Tauri resource that owns one entry in the controller registry.
pub struct PermissionFlowHandle<R: Runtime> {
    app: AppHandle<R>,
    controller_id: u64,
}

impl<R: Runtime> PermissionFlowHandle<R> {
    /// Returns the registry key for the underlying native controller.
    pub fn controller_id(&self) -> u64 {
        self.controller_id
    }
}

impl<R: Runtime> Resource for PermissionFlowHandle<R> {
    /// Dropping the JS-visible handle releases the native controller entry.
    fn close(self: Arc<Self>) {
        self.app
            .permission_flow()
            .drop_controller(self.controller_id);
    }
}

impl From<permission_flow::PermissionAuthorizationState> for PermissionAuthorizationState {
    fn from(value: permission_flow::PermissionAuthorizationState) -> Self {
        match value {
            permission_flow::PermissionAuthorizationState::Granted => Self::Granted,
            permission_flow::PermissionAuthorizationState::NotGranted => Self::NotGranted,
            permission_flow::PermissionAuthorizationState::Unknown => Self::Unknown,
            permission_flow::PermissionAuthorizationState::Checking => Self::Checking,
        }
    }
}

/// Maps the plugin's serializable permission enum to the native Rust crate's
/// strongly-typed permission values.
fn permission_to_native(permission: Permission) -> NativePermission {
    match permission {
        Permission::Accessibility => NativePermission::ACCESSIBILITY,
        Permission::InputMonitoring => NativePermission::INPUT_MONITORING,
        Permission::ScreenRecording => NativePermission::SCREEN_RECORDING,
        Permission::AppManagement => NativePermission::APP_MANAGEMENT,
        Permission::Bluetooth => NativePermission::BLUETOOTH,
        Permission::DeveloperTools => NativePermission::DEVELOPER_TOOLS,
        Permission::FullDiskAccess => NativePermission::FULL_DISK_ACCESS,
        Permission::MediaAppleMusic => NativePermission::MEDIA_APPLE_MUSIC,
    }
}
