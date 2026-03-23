//! Platform-specific configuration: GPU backend, shell detection, IPC path.

use std::path::PathBuf;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod linux;

// ── GPU backends ──────────────────────────────────────────────────────────────

pub fn gpu_backends() -> wgpu::Backends {
    #[cfg(target_os = "windows")]
    return wgpu::Backends::DX12;

    #[cfg(target_os = "linux")]
    return wgpu::Backends::VULKAN;

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    return wgpu::Backends::all();
}

// ── Default shell ─────────────────────────────────────────────────────────────

pub fn default_shell() -> PathBuf {
    #[cfg(target_os = "windows")]
    return windows::default_shell();

    #[cfg(target_os = "linux")]
    return linux::default_shell();

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    return PathBuf::from("sh");
}

// ── IPC path ──────────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub fn ipc_path() -> String {
    #[cfg(target_os = "windows")]
    return r"\\.\pipe\alfred".to_string();

    #[cfg(target_os = "linux")]
    return "/tmp/alfred.sock".to_string();

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    return "/tmp/alfred.sock".to_string();
}
