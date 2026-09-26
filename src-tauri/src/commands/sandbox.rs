use crate::tools::sandbox::{backend_descriptors, SandboxBackendDescriptor};

#[tauri::command(async)]
pub fn list_sandbox_backends() -> Vec<SandboxBackendDescriptor> {
    backend_descriptors()
}
