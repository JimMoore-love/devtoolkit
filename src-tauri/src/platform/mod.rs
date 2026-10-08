//! 平台抽象层：把 Windows / macOS(Linux) 的进程与端口操作统一起来。
//! 通过 cfg 分派到 windows.rs 或 unix.rs。

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct PortInfo {
    pub proto: String,
    pub local_addr: String,
    pub local_port: u16,
    pub remote_addr: String,
    pub state: String,
    pub pid: u32,
    pub process_name: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ProcRow {
    pub pid: u32,
    pub name: String,
    pub mem_kb: u64,
}

#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod imp;

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "unix.rs"]
mod imp;

pub use imp::*;
