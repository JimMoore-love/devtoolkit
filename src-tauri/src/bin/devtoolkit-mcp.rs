//! DevToolkit MCP 服务（stdio 传输）
//!
//! 让 AI 客户端（WorkBuddy / Claude Code 等）通过 MCP 协议直接操作本机的 DevToolkit：
//! 查端口、查项目运行态、启停托管项目、结束占用进程。
//!
//! 本进程**不做任何业务判断**，它只是一个协议转换器：把一次 MCP 工具调用翻译成对
//! DevToolkit GUI 进程控制口的一次请求。好处是"AI 操作"与"人手点界面"共用同一套逻辑和同一份状态 ——
//! AI 启动的服务在界面上就是正常的「运行中」，能看日志、能一键停，不会出现两套口径。
//!
//! GUI 没在运行时会自动拉起同目录的 devtoolkit.exe（设 `DEVTOOLKIT_NO_AUTOSTART=1` 可禁用）。

#[path = "../control_proto.rs"]
#[allow(dead_code)]
mod control_proto;

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use control_proto::{cmd, Resp};
use serde::Deserialize;
use serde_json::{json, Value};

const SERVER_NAME: &str = "devtoolkit";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_PROTOCOL: &str = "2024-11-05";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// 探测候选端口时的连接超时。回环地址 300ms 足够；此前每个候选都等 3 秒，
/// 11 个候选全试一遍要 33 秒才得到"DevToolkit 未运行"，在 AI 侧就是"工具调用卡死"。
const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
/// 探测时的读超时：控制口正常应答在毫秒级，2 秒没动静就说明对面不是它
const PROBE_READ_TIMEOUT: Duration = Duration::from_secs(2);
const AUTOSTART_WAIT_MS: u64 = 600;
const AUTOSTART_TRIES: u32 = 30;

// ---------------- 与 GUI 的配置/控制口握手 ----------------

/// 配置文件的字段视图：只关心控制口所需的两项
#[derive(Deserialize, Default)]
struct MiniConfig {
    #[serde(default)]
    mcp_token: String,
    #[serde(default)]
    mcp_port: u16,
}

fn candidate_config_paths() -> Vec<PathBuf> {
    if let Ok(p) = std::env::var("DEVTOOLKIT_CONFIG") {
        if !p.trim().is_empty() {
            return vec![PathBuf::from(p)];
        }
    }
    let mut v = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        // 与 Tauri identifier 对应；第二项兼容手工放配置的情况
        v.push(PathBuf::from(&appdata).join("cn.devtoolkit.app").join("devtoolkit.json"));
        v.push(PathBuf::from(&appdata).join("DevToolkit").join("devtoolkit.json"));
    }
    v
}

fn load_config() -> MiniConfig {
    for p in candidate_config_paths() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(c) = serde_json::from_str::<MiniConfig>(&s) {
                eprintln!("[devtoolkit-mcp] 读取配置: {}", p.display());
                return c;
            }
        }
    }
    eprintln!("[devtoolkit-mcp] 未找到 devtoolkit.json，将以默认端口尝试连接");
    MiniConfig::default()
}

fn gui_exe() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let dir = me.parent()?;
    let name = if cfg!(windows) { "devtoolkit.exe" } else { "devtoolkit" };
    let p = dir.join(name);
    p.exists().then_some(p)
}

/// 是否允许自动拉起 DevToolkit。
///
/// **默认关闭**，因为不可靠：MCP 服务通常由 AI 客户端作为子进程拉起，客户端结束会话时
/// 往往连带回收整棵进程树，被拉起的 GUI 会跟着消失 —— 而 GUI 关闭时又会结束它托管的全部任务，
/// 结果是"AI 帮我起的服务在会话结束时静默死掉"，这比明确报错更难排查。
/// 若你的客户端不回收子进程树（例如自己用终端跑这个 MCP 服务），可设 DEVTOOLKIT_AUTOSTART=1 打开。
fn autostart_allowed() -> bool {
    std::env::var("DEVTOOLKIT_AUTOSTART")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// 最好努力地脱离父进程后再拉起 GUI（仅在允许自动拉起时使用）
fn spawn_detached(exe: &PathBuf) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

        let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        let mut c = Command::new(exe);
        if let Some(dir) = exe.parent() {
            c.current_dir(dir);
        }
        if c.creation_flags(base | CREATE_BREAKAWAY_FROM_JOB).spawn().is_ok() {
            return Ok(());
        }
        let mut c2 = Command::new(exe);
        if let Some(dir) = exe.parent() {
            c2.current_dir(dir);
        }
        c2.creation_flags(base).spawn().map(|_| ())
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new(exe);
        if let Some(dir) = exe.parent() {
            c.current_dir(dir);
        }
        c.spawn().map(|_| ())
    }
}

/// GUI 未就绪时的统一提示：说清"该做什么"和"可执行文件在哪"
fn not_running_hint() -> String {
    let exe = gui_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("同目录下的 {}", if cfg!(windows) { "devtoolkit.exe" } else { "devtoolkit" }));
    format!(
        "DevToolkit 未运行（控制口 127.0.0.1 无响应）。请先打开 DevToolkit：{exe}\n\
         打开后控制口会自动就绪，再次调用即可。\n\
         如需让本 MCP 服务自动拉起它，请给本进程设置环境变量 DEVTOOLKIT_AUTOSTART=1。"
    )
}

struct Server {
    token: String,
    port_hint: u16,
    port: Option<u16>,
    protocol: String,
}

impl Server {
    fn new(token: String, port_hint: u16) -> Self {
        Server {
            token,
            port_hint,
            port: None,
            protocol: DEFAULT_PROTOCOL.to_string(),
        }
    }

    /// 候选端口：配置文件记录的优先，其次默认端口起顺延（GUI 若因占用换了端口也能找到）
    fn candidates(&self) -> Vec<u16> {
        let mut v = Vec::new();
        if self.port_hint > 0 {
            v.push(self.port_hint);
        }
        for off in 0..=control_proto::PORT_SCAN_RANGE {
            let p = control_proto::DEFAULT_PORT + off;
            if !v.contains(&p) {
                v.push(p);
            }
        }
        v
    }

    /// 探测可用控制口。只有 token 校验通过才算找到，避免误连到别的服务。
    fn locate(&mut self) -> Option<u16> {
        for p in self.candidates() {
            // 探测用短超时：找不到就是找不到，不该让调用方陪等
            if self
                .send(p, cmd::STATUS, json!({}), PROBE_CONNECT_TIMEOUT, PROBE_READ_TIMEOUT)
                .is_ok()
            {
                eprintln!("[devtoolkit-mcp] 已连接 DevToolkit 控制口 127.0.0.1:{p}");
                self.port = Some(p);
                return Some(p);
            }
        }
        None
    }

    fn raw_call(&self, port: u16, cmd_name: &str, args: Value) -> Result<Value, String> {
        self.send(port, cmd_name, args, CONNECT_TIMEOUT, CALL_TIMEOUT)
    }

    /// 一次一问一答的实体。探测与正常调用只有超时参数不同，逻辑共用一份。
    fn send(
        &self,
        port: u16,
        cmd_name: &str,
        args: Value,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<Value, String> {
        let req = json!({ "token": self.token, "cmd": cmd_name, "args": args });
        let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        line.push('\n');

        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let mut s = TcpStream::connect_timeout(&addr, connect_timeout)
            .map_err(|e| format!("连接 127.0.0.1:{port} 失败（{e}）"))?;
        s.set_write_timeout(Some(Duration::from_secs(10))).ok();
        s.set_read_timeout(Some(read_timeout)).ok();
        s.write_all(line.as_bytes())
            .map_err(|e| format!("发送请求失败（{e}）"))?;
        s.flush().ok();

        let mut reader = BufReader::new(s);
        let mut buf = String::new();
        reader
            .read_line(&mut buf)
            .map_err(|e| format!("读取响应失败（{e}）"))?;
        let resp: Resp = serde_json::from_str(buf.trim())
            .map_err(|e| format!("响应格式异常（{e}）"))?;
        if resp.ok {
            Ok(resp.data.unwrap_or(Value::Null))
        } else {
            Err(resp
                .error
                .unwrap_or_else(|| "DevToolkit 未返回错误详情".into()))
        }
    }

    /// 首次调用前确保已连上；GUI 重启换了端口也能自动找回
    fn ensure_port(&mut self) -> Result<u16, String> {
        if let Some(p) = self.port {
            return Ok(p);
        }
        if let Some(p) = self.locate() {
            return Ok(p);
        }
        if autostart_allowed() {
            if let Some(exe) = gui_exe() {
                eprintln!("[devtoolkit-mcp] DevToolkit 未运行，正在拉起 {}", exe.display());
                if let Err(e) = spawn_detached(&exe) {
                    return Err(format!("启动 DevToolkit 失败（{e}）。{}", not_running_hint()));
                }
                for _ in 0..AUTOSTART_TRIES {
                    std::thread::sleep(Duration::from_millis(AUTOSTART_WAIT_MS));
                    if let Some(p) = self.locate() {
                        return Ok(p);
                    }
                }
                return Err(format!(
                    "已尝试拉起 DevToolkit，但控制口一直没就绪（可能被调用方的进程树回收了）。\n{}",
                    not_running_hint()
                ));
            }
            return Err(format!(
                "DevToolkit 未运行，且在其安装目录找不到 {}。",
                if cfg!(windows) { "devtoolkit.exe" } else { "devtoolkit" }
            ));
        }
        Err(not_running_hint())
    }

    fn call(&mut self, cmd_name: &str, args: Value) -> Result<Value, String> {
        let port = self.ensure_port()?;
        match self.raw_call(port, cmd_name, args.clone()) {
            Ok(v) => Ok(v),
            Err(e) => {
                // 控制口可能随 GUI 重启失效，找回一次再试
                self.port = None;
                if let Ok(p) = self.ensure_port() {
                    if p != port {
                        return self.raw_call(p, cmd_name, args);
                    }
                }
                Err(e)
            }
        }
    }
}

// ---------------- MCP 工具定义 ----------------

fn str_prop(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}

/// 工具名 → 控制口命令的**唯一映射表**。
///
/// 工具 schema（`tools()`）负责"告诉 AI 有哪些工具"，这张表负责"收到了往哪转发"。
/// 以前两者是各自写一遍的：新增一个工具却忘了在转发处加分支，AI 会得到一个
/// 「未知工具」，而且没有任何测试会发现。现在 `tools()` 里的工具名必须在这张表
/// 或 `LOCAL_TOOLS` 里出现，`mcp_tool_routes_cover_all_tools` 会断言这一点。
const TOOL_ROUTES: &[(&str, &str)] = &[
    ("list_projects", cmd::LIST_PROJECTS),
    ("scan_ports", cmd::SCAN_PORTS),
    ("list_ports", cmd::LIST_PORTS),
    ("check_ports", cmd::CHECK_PORTS),
    ("list_tasks", cmd::LIST_TASKS),
    ("get_task_log", cmd::TASK_LOG),
    ("process_detail", cmd::PROCESS_DETAIL),
    ("start_project", cmd::START_PROJECT),
    ("stop_project", cmd::STOP_PROJECT),
    ("takeover_project", cmd::TAKEOVER_PROJECT),
    ("kill_pid", cmd::KILL_PID),
];

/// 由本地投影 / 特殊包装实现、不走进一对一路由的工具。
/// `list_running` 是 scan_ports 的过滤结果；`devtoolkit_status` 在应用未启动时
/// 要返回结构化结果而不是工具错误。
const LOCAL_TOOLS: &[&str] = &[
    "devtoolkit_status",
    "list_running",
];

fn route_of(tool: &str) -> Option<&'static str> {
    TOOL_ROUTES
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, c)| *c)
}

fn tool_names() -> Vec<String> {
    tools()
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from))
        .collect()
}

fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "devtoolkit_status",
            "description": "查看 DevToolkit 自身状态：是否在运行、版本、控制口端口、已托管任务数、AI 操作次数。适合作为第一个调用来确认链路是否可用；若返回 running=false，说明 DevToolkit 尚未打开，请提示用户启动它再继续。",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "list_projects",
            "description": "列出 DevToolkit 中已配置托管的项目（含启动命令、工作目录、声明端口、分组、环境变量）。想看「现在哪些在跑」请用 list_running。",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "list_running",
            "description": "列出当前正在运行的项目：包含应用托管启动的，以及在别处（终端/IDE）启动后被识别到的。每项带运行来源（task=本应用启动 / port=声明端口在监听 / path=进程目录命中项目）、监听端口、进程 PID。判断端口归属用这个工具最快。",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "scan_ports",
            "description": "全量扫描：返回本机全部网络端口占用明细 + 每个项目的运行态原始数据。数据量较大，只在需要端口级细节（谁占着某个端口、进程名、状态）时使用；只想知道项目在不在跑请用 list_running。",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "list_tasks",
            "description": "列出由 DevToolkit 托管启动的任务（task_id / 项目 / PID / 启动时间）。这些任务可以读日志、可以 stop_project 停止。",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "get_task_log",
            "description": "读取某个托管任务最近的输出日志（stdout/stderr 合并，带 [stdout]/[stderr]/[sys] 前缀）。排查「服务起不来」「启动后报错」时先用它。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": str_prop("任务 id，从 list_tasks 或 start_project 的返回值获取"),
                    "limit": { "type": "integer", "description": "返回最后多少行，默认 200，最大 2000" }
                },
                "required": ["task_id"]
            }
        }),
        json!({
            "name": "list_ports",
            "description": "列出系统当前网络连接与监听端口（协议、地址、状态、PID、进程名）。想按项目归类请用 list_running。",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "check_ports",
            "description": "检查指定端口是否被占用，返回占用进程的 PID 与进程名。启动服务前用它确认端口空闲最稳妥。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ports": str_prop("端口列表，逗号或空格分隔，例如 \"8000,9528\"")
                },
                "required": ["ports"]
            }
        }),
        json!({
            "name": "process_detail",
            "description": "查看指定进程的完整命令行。用于判断某个占着端口的进程到底是不是目标项目。",
            "inputSchema": {
                "type": "object",
                "properties": { "pid": { "type": "integer", "description": "进程 PID" } },
                "required": ["pid"]
            }
        }),
        json!({
            "name": "start_project",
            "description": "启动一个已托管项目。启动前会自动做端口冲突检测，冲突时返回占用详情而不是硬启动；项目已在运行时不会重复启动。若项目被外部进程占用会提示改用 takeover_project。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": str_prop("项目名称，支持部分匹配（如 \"qidian-admin 后端\"）"),
                    "id": str_prop("项目 id，比 name 更精确，优先使用")
                }
            }
        }),
        json!({
            "name": "stop_project",
            "description": "停止一个由 DevToolkit 托管运行的项目（连同其子进程树）。只能停本应用启动的任务；外部启动的进程请用 takeover_project 或 kill_pid。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": str_prop("项目名称，支持部分匹配"),
                    "id": str_prop("项目 id，优先使用")
                }
            }
        }),
        json!({
            "name": "takeover_project",
            "description": "「结束并接管」：先结束该项目当前的外部进程，等端口释放后立刻以托管方式重新启动。注意会中断服务几秒。适合「服务是手工在终端起的、想交给 DevToolkit 管理」的场景。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": str_prop("项目名称，支持部分匹配"),
                    "id": str_prop("项目 id，优先使用")
                }
            }
        }),
        json!({
            "name": "kill_pid",
            "description": "强制结束指定进程及其子进程树。危险操作：会立即中断该进程提供的服务，仅在确认端口被无用进程占用时使用。",
            "inputSchema": {
                "type": "object",
                "properties": { "pid": { "type": "integer", "description": "要结束的进程 PID" } },
                "required": ["pid"]
            }
        }),
    ]
}

// ---------------- JSON-RPC over stdio ----------------

fn main() {
    let cfg = load_config();
    // 不在启动阶段探测控制口：探测会拖慢 MCP 握手，改为首次工具调用时按需连接
    let mut server = Server::new(cfg.mcp_token, cfg.mcp_port);

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("[devtoolkit-mcp] 读取 stdin 失败: {e}");
                break;
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[devtoolkit-mcp] 非法 JSON-RPC 报文: {e}");
                continue;
            }
        };

        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

        // 通知（无 id）不需要回复
        if method.starts_with("notifications/") || id.is_none() {
            if method == "initialize" {
                eprintln!("[devtoolkit-mcp] 收到无 id 的 initialize，忽略");
            }
            continue;
        }

        let out = match method {
            "initialize" => {
                if let Some(v) = msg
                    .get("params")
                    .and_then(|p| p.get("protocolVersion"))
                    .and_then(|v| v.as_str())
                {
                    server.protocol = v.to_string();
                }
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": server.protocol,
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION }
                    }
                })
            }
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": tools() }
            }),
            "tools/call" => {
                let name = msg
                    .get("params")
                    .and_then(|p| p.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let args = msg
                    .get("params")
                    .and_then(|p| p.get("arguments"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let (text, is_error) = run_tool(&mut server, name, args);
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [ { "type": "text", "text": text } ],
                        "isError": is_error
                    }
                })
            }
            other => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("不支持的方法: {other}") }
            }),
        };

        if let Ok(s) = serde_json::to_string(&out) {
            if writer.write_all(s.as_bytes()).is_err() || writer.write_all(b"\n").is_err() {
                break;
            }
            let _ = writer.flush();
        }
    }
}

/// 返回 (给模型看的文本, 是否错误)
fn run_tool(server: &mut Server, name: &str, args: Value) -> (String, bool) {
    if name.is_empty() {
        return ("缺少工具名".into(), true);
    }

    // 白名单前置：既不在路由表、也不在本地工具表里的名字，一律挡在这里，
    // 绝不会带着一个错名字去请求控制口（少一次往返，错误信息也更直接）
    let routed = route_of(name);
    if routed.is_none() && !LOCAL_TOOLS.contains(&name) {
        return (
            format!("未知工具「{name}」。可用工具：{}", tool_names().join(", ")),
            true,
        );
    }

    // list_running 是 scan_ports 的投影，纯过滤，不额外引入判断逻辑
    if name == "list_running" {
        return match server.call(cmd::SCAN_PORTS, json!({})) {
            Ok(v) => {
                let running: Vec<Value> = v
                    .get("runtime")
                    .and_then(|r| r.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter(|x| x.get("running").and_then(|b| b.as_bool()).unwrap_or(false))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                (
                    pretty(&json!({
                        "total": running.len(),
                        "running": running,
                        "hint": "source=task 表示由 DevToolkit 托管（可 get_task_log / stop_project）；source=port|path 且 external=true 表示外部启动（需 takeover_project 或 kill_pid）"
                    })),
                    false,
                )
            }
            Err(e) => (e, true),
        };
    }

    // devtoolkit_status 特殊处理：应用没开时给结构化结果而不是工具错误，
    // 让 AI 能一眼看出"是应用没启动"而不是"工具坏了"
    if name == "devtoolkit_status" {
        return match server.call(cmd::STATUS, json!({})) {
            Ok(mut v) => {
                if let Some(o) = v.as_object_mut() {
                    o.insert("running".into(), json!(true));
                }
                (pretty(&v), false)
            }
            Err(e) => (pretty(&json!({ "running": false, "hint": e })), false),
        };
    }

    match routed {
        Some(c) => match server.call(c, args) {
            Ok(v) => (pretty(&v), false),
            Err(e) => (e, true),
        },
        // 本地工具分支都在上面 return 了，走到这里说明登记了却没实现
        None => (
            format!("工具「{name}」标记为本地实现但没有对应分支（内部错误）"),
            true,
        ),
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// 漂移一号位：tools() 里列了、但没有处理逻辑 → AI 看得见却调不通
    #[test]
    fn every_listed_tool_is_routable() {
        for name in tool_names() {
            assert!(
                route_of(&name).is_some() || LOCAL_TOOLS.contains(&name.as_str()),
                "工具「{name}」没有处理逻辑：请在 TOOL_ROUTES 或 LOCAL_TOOLS 中登记"
            );
        }
    }

    /// 漂移二号位：路由表里有、但 tools() 没列 → 写了却对 AI 不可见
    #[test]
    fn every_route_is_listed() {
        let names = tool_names();
        for (name, _) in TOOL_ROUTES {
            assert!(
                names.iter().any(|n| n == name),
                "路由「{name}」没有在 tools() 中声明"
            );
        }
    }

    /// 漂移三号位：路由指向一个后端根本不认识的控制口命令
    #[test]
    fn every_route_points_to_a_known_command() {
        for (name, c) in TOOL_ROUTES {
            assert!(
                control_proto::Cmd::parse(c).is_some(),
                "工具「{name}」指向未知控制口命令「{c}」"
            );
        }
    }

    /// 本地工具也必须真的在 tools() 列表里，否则就是删了工具忘了删登记
    #[test]
    fn local_tools_are_listed() {
        let names = tool_names();
        for name in LOCAL_TOOLS {
            assert!(names.iter().any(|n| n == name), "LOCAL_TOOLS 里的「{name}」不在 tools() 中");
        }
    }

    #[test]
    fn tool_names_are_unique() {
        let mut seen = HashSet::new();
        for n in tool_names() {
            assert!(seen.insert(n.clone()), "工具名重复: {n}");
        }
    }

    /// 工具 schema 自身要合法：description 非空、required 的字段必须在 properties 里定义
    /// （required 指向不存在的字段时，模型无从填参，只会反复调错）
    #[test]
    fn tools_have_valid_shape() {
        for t in tools() {
            let name = t
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| panic!("工具缺少 name: {t}"));
            assert!(
                t.get("description")
                    .and_then(|v| v.as_str())
                    .map_or(false, |s| !s.is_empty()),
                "工具「{name}」缺少 description"
            );
            let schema = t.get("inputSchema").expect("inputSchema 缺失");
            assert_eq!(
                schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "工具「{name}」的 inputSchema.type 必须是 object"
            );
            let props = schema
                .get("properties")
                .and_then(|v| v.as_object())
                .expect("properties 缺失");
            if let Some(req) = schema.get("required").and_then(|v| v.as_array()) {
                for r in req {
                    let k = r.as_str().unwrap_or_default();
                    assert!(
                        props.contains_key(k),
                        "工具「{name}」required 里的「{k}」未在 properties 中定义"
                    );
                }
            }
        }
    }

    /// 参数名必须与控制口读取的键一致，否则调用会被"缺少参数"顶回来
    #[test]
    fn required_args_match_backend_keys() {
        let expect = [("get_task_log", "task_id"), ("check_ports", "ports"), ("kill_pid", "pid")];
        for (tool, key) in expect {
            let t = tools()
                .into_iter()
                .find(|t| t["name"] == tool)
                .unwrap_or_else(|| panic!("找不到工具 {tool}"));
            let req = t["inputSchema"]["required"].as_array().expect("required 缺失");
            assert!(
                req.iter().any(|r| r.as_str() == Some(key)),
                "工具「{tool}」的 required 应含「{key}」，实际为 {req:?}"
            );
        }
    }
}
