//! DevToolkit 控制口线协议 —— GUI 侧 `control.rs` 与独立进程 `devtoolkit-mcp` 共用同一份定义。
//!
//! 之所以不各自写一套：命令名一旦两边不一致，就是"AI 调了但后端不认"的隐形故障。
//! 本文件不依赖 tauri，两个二进制都能直接 include。
//!
//! 传输：本机回环 TCP，非 127.0.0.1 来源一律拒绝；一行请求 / 一行响应，UTF-8 JSON。
//! ```text
//! -> {"token":"...","cmd":"start_project","args":{"name":"qidian-admin 后端"}}
//! <- {"ok":true,"data":{...}}
//! <- {"ok":false,"error":"..."}
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 默认监听端口；被占用时向后顺延 PORT_SCAN_RANGE 个
pub const DEFAULT_PORT: u16 = 9527;
pub const PORT_SCAN_RANGE: u16 = 10;

/// 单条请求上限，防止异常输入把内存撑爆
pub const MAX_LINE_BYTES: usize = 1 << 20;

pub mod cmd {
    pub const STATUS: &str = "status";
    pub const LIST_PROJECTS: &str = "list_projects";
    pub const SCAN_PORTS: &str = "scan_ports";
    pub const LIST_PORTS: &str = "list_ports";
    pub const CHECK_PORTS: &str = "check_ports";
    pub const LIST_TASKS: &str = "list_tasks";
    pub const TASK_LOG: &str = "task_log";
    pub const PROCESS_DETAIL: &str = "process_detail";
    pub const START_PROJECT: &str = "start_project";
    pub const STOP_PROJECT: &str = "stop_project";
    pub const TAKEOVER_PROJECT: &str = "takeover_project";
    pub const KILL_PID: &str = "kill_pid";
}

/// 控制口命令的枚举形态。
///
/// 存在的意义只有一个：**让"新增了命令却忘了处理"变成编译错误**。
/// `control.rs` 的 `dispatch` 是对本枚举的穷举 match，漏掉一个变体编译不过；
/// 而在此之前，命令名只是一堆字符串常量，加个常量忘了加分支，要到运行期
/// 才会表现为"AI 调了但后端回未知命令"。`as_str` 同样是穷举 match，
/// 所以命令名与命令集合不会各自漂移。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cmd {
    Status,
    ListProjects,
    ScanPorts,
    ListPorts,
    CheckPorts,
    ListTasks,
    TaskLog,
    ProcessDetail,
    StartProject,
    StopProject,
    TakeoverProject,
    KillPid,
}

impl Cmd {
    /// 全部命令，顺序即文档顺序
    pub const ALL: &'static [Cmd] = &[
        Cmd::Status,
        Cmd::ListProjects,
        Cmd::ScanPorts,
        Cmd::ListPorts,
        Cmd::CheckPorts,
        Cmd::ListTasks,
        Cmd::TaskLog,
        Cmd::ProcessDetail,
        Cmd::StartProject,
        Cmd::StopProject,
        Cmd::TakeoverProject,
        Cmd::KillPid,
    ];

    /// 命令行名字（协议里传输的字符串）
    pub const fn as_str(self) -> &'static str {
        match self {
            Cmd::Status => cmd::STATUS,
            Cmd::ListProjects => cmd::LIST_PROJECTS,
            Cmd::ScanPorts => cmd::SCAN_PORTS,
            Cmd::ListPorts => cmd::LIST_PORTS,
            Cmd::CheckPorts => cmd::CHECK_PORTS,
            Cmd::ListTasks => cmd::LIST_TASKS,
            Cmd::TaskLog => cmd::TASK_LOG,
            Cmd::ProcessDetail => cmd::PROCESS_DETAIL,
            Cmd::StartProject => cmd::START_PROJECT,
            Cmd::StopProject => cmd::STOP_PROJECT,
            Cmd::TakeoverProject => cmd::TAKEOVER_PROJECT,
            Cmd::KillPid => cmd::KILL_PID,
        }
    }

    /// 解析命令名；未知返回 None（由调用方给出带候选列表的错误）
    pub fn parse(s: &str) -> Option<Cmd> {
        let s = s.trim();
        Cmd::ALL.iter().copied().find(|c| c.as_str() == s)
    }
}

/// 全部命令名，供自检脚本与文档使用
pub fn all_names() -> Vec<&'static str> {
    Cmd::ALL.iter().map(|c| c.as_str()).collect()
}

#[derive(Debug, Deserialize)]
pub struct Req {
    #[serde(default)]
    pub token: String,
    pub cmd: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Resp {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Resp {
    pub fn ok(data: Value) -> Self {
        Resp {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Resp {
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}
