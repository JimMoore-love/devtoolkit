//! MCP 控制口 —— 把应用内已有的能力通过本机回环 TCP 暴露给独立的 `devtoolkit-mcp` 进程。
//!
//! 设计要点：
//! 1. **不重复实现业务逻辑**：所有分支都直接调用 `main.rs` 里已经被 GUI 使用的同一批函数，
//!    保证"AI 操作"和"人手点按钮"走完全同一条路径，不会出现两套口径。
//! 2. **只监听 127.0.0.1**，并且必须携带 token（随机生成后存在配置文件里），
//!    避免本机其它程序或浏览器页面随意驱动 DevToolkit。
//! 3. **写操作全部落审计**（配置里的 `mcp_audit`），GUI 上能看到"哪个 AI 在什么时候动了什么"。
//! 4. 控制口启动失败（端口被占满等）不影响 GUI 正常使用，只是 MCP 不可用。

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::control_proto::{self as proto, Cmd, Req, Resp};
use crate::{
    lockx, check_ports, kill_pid, list_ports, now_sec, process_detail, protected_port_list, read_config,
    scan_ports, start_task, stop_task, with_config, AppState, AuditItem, Config, Project, ProjectRuntime,
};

/// 请求处理上限：scan_ports 会枚举全机端口，给足时间但别无限等
const CONN_TIMEOUT: Duration = Duration::from_secs(30);

/// accept 轮询间隔。非阻塞 + 短睡眠，是为了让 `stop()` 能真正让线程退出并释放端口。
const ACCEPT_POLL: Duration = Duration::from_millis(50);

pub struct Ctx {
    pub port: u16,
    pub token: String,
    pub started_at: u64,
    pub requests: AtomicU64,
    pub errors: AtomicU64,
    /// 运行标志。置 false 后 accept 线程在下一个轮询周期退出，listener 随之 drop 并释放端口。
    /// 用标志位而不是"关掉 listener"，是因为阻塞式 `incoming()` 没有可移植的中断手段
    /// （自连接唤醒太脏），非阻塞轮询最直白。字段私有：只有本模块能造 Ctx。
    shutdown: Arc<AtomicBool>,
}

impl Ctx {
    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    pub fn errors(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }
}

/// 启动控制口。返回句柄用于 GUI 展示状态；绑定失败返回 None。
pub fn start(app: AppHandle) -> Option<Arc<Ctx>> {
    // 绑定前先读一次保护名单：它决定顺延时哪些端口必须绕开
    let protected = protected_port_list(&app);
    // 用户在管理页指定过端口就优先用它（0 = 默认段），否则改完端口重启会又掉回 9527
    let preferred = current_config(&app).mcp_preferred_port;
    let (listener, port) = match bind_any(&protected, preferred) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[mcp] 控制口绑定失败，MCP 功能不可用: {e}");
            return None;
        }
    };

    let token = match sync_config(&app, port) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[mcp] 写入 token 失败，控制口未启动: {e}");
            return None;
        }
    };

    let ctx = Arc::new(Ctx {
        port,
        token,
        started_at: now_sec(),
        requests: AtomicU64::new(0),
        errors: AtomicU64::new(0),
        shutdown: Arc::new(AtomicBool::new(false)),
    });

    // 非阻塞 accept + 短睡眠轮询，而不是 `for stream in listener.incoming()`：
    // 后者会永久阻塞在 accept 上，没有任何可移植办法让它在不杀进程的前提下退出，
    // 于是"关闭控制口"和"改端口"都做不到。轮询版本让 stop() 能真正释放端口。
    if listener.set_nonblocking(true).is_err() {
        eprintln!("[mcp] 无法将控制口设为非阻塞，关闭/改端口功能将不可用");
    }

    let accept_ctx = ctx.clone();
    let accept_app = app.clone();
    thread::spawn(move || loop {
        if accept_ctx.shutdown.load(Ordering::Relaxed) {
            break;
        }
        match listener.accept() {
            Ok((s, _)) => {
                // Windows 上 accept 返回的 socket **会继承 listener 的非阻塞属性**
                // （Linux 不会）。不显式改回阻塞，serve 里的 read_line 会立刻
                // 返回 WouldBlock，表现为"AI 一发请求就说解析失败"。必须还原。
                s.set_nonblocking(false).ok();
                let a = accept_app.clone();
                let c = accept_ctx.clone();
                thread::spawn(move || {
                    let _ = serve(a, c, s);
                });
            }
            // WouldBlock = 本轮没人连，正常；其它错误也别空转，睡一下再试
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    });

    eprintln!("[mcp] 控制口已启动 http://127.0.0.1:{port}");
    Some(ctx)
}

/// 停止控制口，并等到端口真的被释放才返回。
///
/// 只置标志就跑是不行的：紧接着的重启（改端口 / 重置令牌）会撞上"端口还被占着"，
/// 于是 bind 顺延到隔壁端口，用户看到"改了端口但没生效"。所以这里主动探测确认。
pub fn stop(ctx: &Ctx) -> Result<(), String> {
    ctx.shutdown.store(true, Ordering::Relaxed);
    for _ in 0..40 {
        match TcpListener::bind(("127.0.0.1", ctx.port)) {
            // 能绑上就说明旧 listener 已经释放（探测用的 listener 立刻 drop）
            Ok(l) => {
                drop(l);
                return Ok(());
            }
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    }
    Err(format!(
        "控制口 {} 停止后端口 2 秒内未释放，请稍后重试",
        ctx.port
    ))
}

/// 在 127.0.0.1 上找一个可用端口（默认 9527，被占就顺延）。
///
/// 顺延时**跳过受保护端口**：默认区间 9527..=9537 正好盖住用户前端的 9528，
/// 若不绕开，就会出现"控制口把前端端口抢走、前端起不来"这种本末倒置的事。
/// 在 127.0.0.1 上找一个可用端口：从 `preferred`（0 表示默认段）起顺延最多 PORT_SCAN_RANGE 个。
/// 受保护端口一律跳过 —— 保护名单的意义就是"任何自动化都不许占它"。
fn bind_any(protected: &[u16], preferred: u16) -> Result<(TcpListener, u16), String> {
    let base = if preferred == 0 {
        proto::DEFAULT_PORT
    } else {
        preferred
    };
    let mut last = String::from("未尝试");
    for offset in 0..=u32::from(proto::PORT_SCAN_RANGE) {
        let raw = u32::from(base) + offset;
        if raw > u32::from(u16::MAX) {
            break;
        }
        let port = raw as u16;
        if protected.contains(&port) {
            last = format!("{port}: 受保护端口，已跳过");
            continue;
        }
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => return Ok((l, port)),
            Err(e) => last = format!("{port}: {e}"),
        }
    }
    Err(format!(
        "127.0.0.1 上 {base} 起连续端口均不可用（最后错误 {last}）"
    ))
}

/// 首次启动时生成 token，并把实际监听端口写回配置（MCP 进程据此连接）。
/// 已有 token 时只更新端口，不轮换 token，避免已配置好的客户端突然连不上。
///
/// 写盘走 `with_config`（它持配置写锁）。此前这里是自行 read + write，
/// 与界面保存项目并发时会把对方刚写的内容整段覆盖掉。
fn sync_config(app: &AppHandle, port: u16) -> Result<String, String> {
    let cur = current_config(app);
    if !cur.mcp_token.trim().is_empty() && cur.mcp_port == port {
        return Ok(cur.mcp_token);
    }
    let state = app.state::<AppState>();
    with_config(&state, |cfg| {
        if cfg.mcp_token.trim().is_empty() {
            cfg.mcp_token = gen_token();
        }
        cfg.mcp_port = port;
        cfg.mcp_token.clone()
    })
}

/// 重置访问令牌，返回新令牌。
///
/// 不复用 `sync_config`：它的语义是"没有才生成"，而重置要的恰恰是"每次换新的"。
/// 调用方负责重启控制口 —— `Ctx.token` 是构造时拷进去的，不重启就还在用旧值校验。
pub fn rotate_token(app: &AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    with_config(&state, |cfg| {
        cfg.mcp_token = gen_token();
        cfg.mcp_token.clone()
    })
}

/// 32 位十六进制随机串。用 RandomState 的系统随机种子，避免为一个 token 引入 rand 依赖。
fn gen_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::with_capacity(32);
    for i in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(now_sec());
        h.write_u64(std::process::id() as u64);
        h.write_u64(i);
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// 单条连接 = 一问一答，读完即关。短连接换来的是"GUI 重启后自动恢复"，无需维护重连状态。
fn serve(app: AppHandle, ctx: Arc<Ctx>, stream: TcpStream) -> std::io::Result<()> {
    let peer_ok = stream
        .peer_addr()
        .map(|a| a.ip().is_loopback())
        .unwrap_or(false);
    if !peer_ok {
        return Ok(());
    }

    stream.set_read_timeout(Some(CONN_TIMEOUT)).ok();
    stream.set_write_timeout(Some(CONN_TIMEOUT)).ok();

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let resp = if line.len() > proto::MAX_LINE_BYTES {
        ctx.errors.fetch_add(1, Ordering::Relaxed);
        Resp::err("请求体过大")
    } else {
        match serde_json::from_str::<Req>(line.trim()) {
            Ok(req) => {
                if req.token != ctx.token {
                    ctx.errors.fetch_add(1, Ordering::Relaxed);
                    Resp::err("token 校验失败")
                } else {
                    ctx.requests.fetch_add(1, Ordering::Relaxed);
                    match Cmd::parse(&req.cmd) {
                        None => {
                            ctx.errors.fetch_add(1, Ordering::Relaxed);
                            Resp::err(format!(
                                "未知命令「{}」。可用命令：{}",
                                req.cmd,
                                proto::all_names().join(", ")
                            ))
                        }
                        Some(c) => match dispatch(&app, &ctx, c, req.args) {
                            Ok(v) => Resp::ok(v),
                            Err(e) => {
                                ctx.errors.fetch_add(1, Ordering::Relaxed);
                                Resp::err(e)
                            }
                        },
                    }
                }
            }
            Err(e) => {
                ctx.errors.fetch_add(1, Ordering::Relaxed);
                Resp::err(format!("请求解析失败: {e}"))
            }
        }
    };

    let mut out = stream;
    let text = serde_json::to_string(&resp)
        .unwrap_or_else(|_| r#"{"ok":false,"error":"响应序列化失败"}"#.to_string());
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

// ---------------- 命令分发 ----------------

fn dispatch(app: &AppHandle, ctx: &Ctx, c: Cmd, args: Value) -> Result<Value, String> {
    // 穷举 match：以后往 Cmd 里加命令却忘了在这里处理，编译期就会报错
    match c {
        Cmd::Status => {
            let cfg = current_config(app);
            let running = lockx(&app.state::<AppState>().tasks).len();
            Ok(json!({
                "app": "DevToolkit",
                "version": env!("CARGO_PKG_VERSION"),
                "pid": std::process::id(),
                "control_port": ctx.port,
                "control_started_at": ctx.started_at,
                "requests_served": ctx.requests(),
                "request_errors": ctx.errors(),
                "managed_tasks": running,
                "configured_projects": cfg.projects.len(),
                "mcp_audit_count": cfg.mcp_audit.len(),
            }))
        }

        Cmd::ListProjects => {
            let cfg = current_config(app);
            Ok(json!({
                "total": cfg.projects.len(),
                "groups": cfg.projects.iter().map(|p| p.group.clone()).filter(|g| !g.is_empty()).collect::<std::collections::BTreeSet<_>>(),
                "projects": cfg.projects,
            }))
        }

        Cmd::ScanPorts => {
            let projects = match args.get("projects") {
                Some(Value::Array(_)) => serde_json::from_value::<Vec<Project>>(args["projects"].clone())
                    .map_err(|e| format!("projects 参数格式错误: {e}"))?,
                Some(Value::Null) | None => current_config(app).projects,
                Some(_) => return Err("projects 必须是项目数组".into()),
            };
            let r = block(scan_ports(app.clone(), projects))?;
            to_value(r)
        }

        Cmd::ListPorts => to_value(list_ports(app.clone())?),

        Cmd::CheckPorts => {
            let ports = arg_str(&args, "ports")?;
            to_value(block(check_ports(ports))?)
        }

        Cmd::ListTasks => {
            let state = app.state::<AppState>();
            let tasks = lockx(&state.tasks);
            let list: Vec<Value> = tasks
                .iter()
                .map(|(id, t)| {
                    json!({
                        "task_id": id,
                        "project_id": t.project.id,
                        "name": t.project.name,
                        "pid": t.pid,
                        "started_at": t.started_at,
                        "cwd": t.project.cwd,
                        "ports": t.project.ports,
                    })
                })
                .collect();
            Ok(json!({ "total": list.len(), "tasks": list }))
        }

        Cmd::TaskLog => {
            let task_id = arg_str(&args, "task_id")?;
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(200)
                .clamp(1, 2000) as usize;
            let state = app.state::<AppState>();
            let running = lockx(&state.tasks).contains_key(&task_id);
            let buf = state
                .logs
                .lock()
                .unwrap()
                .get(&task_id)
                .cloned()
                .unwrap_or_default();
            let total = buf.len();
            let lines: Vec<String> = buf.into_iter().skip(total.saturating_sub(limit)).collect();
            Ok(json!({
                "task_id": task_id,
                "running": running,
                "total_lines": total,
                "returned": lines.len(),
                "lines": lines,
            }))
        }

        Cmd::ProcessDetail => {
            let pid = arg_pid(&args)?;
            to_value(process_detail(pid)?)
        }

        Cmd::KillPid => {
            let pid = arg_pid(&args)?;
            let r = kill_pid(app.clone(), pid)?;
            audit(app, c.as_str(), "kill_pid", &format!("PID {pid}"), r.ok);
            to_value(r)
        }

        Cmd::StartProject => {
            let project = pick_project(app, &args)?;
            // 外部占用时不要硬撞端口，直接引导 AI 走「结束并接管」
            if let Some(rt) = runtime_of(app, &project)? {
                if rt.external {
                    audit(app, c.as_str(), "start", &project.name, false);
                    // 不需要再区分"端口是否受保护"：接管走的是项目上下文，
                    // 受保护端口对它不设限（见 `KillScope::Project`）
                    return Err(format!(
                        "项目「{}」正被外部进程占用（PID {}，端口 {}）。请改用 takeover_project 结束外部进程并接管。",
                        project.name,
                        join_u32(&rt.pids),
                        join_u16(&rt.ports)
                    ));
                }
                if rt.running {
                    return Ok(json!({
                        "started": false,
                        "reason": "already_running",
                        "message": format!("项目「{}」已在运行中", project.name),
                        "task_id": rt.task_id,
                        "pid": rt.pids.first().copied().unwrap_or(0),
                    }));
                }
            }
            match block(start_task(app.clone(), project.clone())) {
                Ok(t) => {
                    audit(app, c.as_str(), "start", &project.name, true);
                    to_value(t)
                }
                Err(e) => {
                    audit(app, c.as_str(), "start", &project.name, false);
                    Err(e)
                }
            }
        }

        Cmd::StopProject => {
            let project = pick_project(app, &args)?;
            let task_id = {
                let state = app.state::<AppState>();
                let tasks = lockx(&state.tasks);
                tasks
                    .iter()
                    .find(|(_, t)| t.project.id == project.id)
                    .map(|(k, _)| k.clone())
            };
            match task_id {
                Some(tid) => match block(stop_task(app.clone(), tid.clone())) {
                    Ok(_) => {
                        audit(app, c.as_str(), "stop", &project.name, true);
                        Ok(json!({ "stopped": true, "task_id": tid, "name": project.name }))
                    }
                    Err(e) => {
                        audit(app, c.as_str(), "stop", &project.name, false);
                        Err(e)
                    }
                },
                None => Err(format!(
                    "项目「{}」不是本应用启动的（或已停止）。外部进程请用 kill_pid 结束。",
                    project.name
                )),
            }
        }

        Cmd::TakeoverProject => {
            let project = pick_project(app, &args)?;
            let name = project.name.clone();
            // 接管的目标被限定为"占用本项目声明端口的进程"，所以受保护端口不拦 ——
            // 那道闸门防的是"拿一个裸 PID 乱杀无关服务"，不是正常启停自己的项目。
            // 具体实现复用界面同一份 `takeover_project`，两条入口不会走出两套行为。
            match block(crate::takeover_project(app.clone(), project)) {
                Ok(r) => {
                    audit(app, c.as_str(), "takeover", &name, true);
                    to_value(json!({
                        "taken_over": true,
                        "killed_pids": r.killed_pids,
                        "task": r.task,
                    }))
                }
                Err(e) => {
                    audit(app, c.as_str(), "takeover", &name, false);
                    Err(e)
                }
            }
        }
    }
}

// ---------------- 辅助 ----------------

/// 在 Tauri 的 async 运行时上同步等待异步命令，控制口线程本身是阻塞模型
fn block<T>(fut: impl std::future::Future<Output = T>) -> T {
    tauri::async_runtime::block_on(fut)
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

fn current_config(app: &AppHandle) -> Config {
    let state = app.state::<AppState>();
    let path = lockx(&state.config_path).clone();
    read_config(&path)
}

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("缺少参数 {key}"))
}

fn arg_pid(args: &Value) -> Result<u32, String> {
    let v = args.get("pid").ok_or("缺少参数 pid")?;
    let pid = v
        .as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
        .ok_or("pid 必须是数字")?;
    u32::try_from(pid).map_err(|_| "pid 超出范围".to_string())
}

/// 按 id 精确 / name 精确 / name 模糊 定位项目；模糊命中多个时要求调用方消歧
fn pick_project(app: &AppHandle, args: &Value) -> Result<Project, String> {
    let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    resolve_project(&current_config(app), id, name)
}

/// 纯函数版本的定位逻辑，便于单测；参数校验与候选罗列都在这里
fn resolve_project(cfg: &Config, id: &str, name: &str) -> Result<Project, String> {
    if id.is_empty() && name.is_empty() {
        return Err("需要提供项目 id 或 name".into());
    }

    if !id.is_empty() {
        return cfg
            .projects
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "没有 id 为「{id}」的项目。现有项目：{}",
                    list_names(cfg)
                )
            });
    }

    let needle = name.to_lowercase();
    if let Some(p) = cfg
        .projects
        .iter()
        .find(|p| p.name.to_lowercase() == needle)
    {
        return Ok(p.clone());
    }
    let hits: Vec<&Project> = cfg
        .projects
        .iter()
        .filter(|p| p.name.to_lowercase().contains(&needle))
        .collect();
    match hits.len() {
        1 => Ok(hits[0].clone()),
        0 => Err(format!(
            "没有名称匹配「{name}」的项目。现有项目：{}",
            list_names(cfg)
        )),
        _ => Err(format!(
            "「{name}」匹配到多个项目：{}，请用 id 指定",
            hits.iter()
                .map(|p| format!("{}（{}）", p.name, p.id))
                .collect::<Vec<_>>()
                .join("、")
        )),
    }
}

fn list_names(cfg: &Config) -> String {
    if cfg.projects.is_empty() {
        return "（当前没有任何已托管项目）".into();
    }
    cfg.projects
        .iter()
        .map(|p| format!("{}（{}）", p.name, p.id))
        .collect::<Vec<_>>()
        .join("、")
}

/// 复用 GUI 的运行态判定，保证 AI 看到的"在不在跑"和界面上完全一致
fn runtime_of(app: &AppHandle, project: &Project) -> Result<Option<ProjectRuntime>, String> {
    let r = block(scan_ports(app.clone(), vec![project.clone()]))?;
    Ok(r.runtime.into_iter().next())
}

fn join_u32(v: &[u32]) -> String {
    if v.is_empty() {
        return "-".to_string();
    }
    v.iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join(" / ")
}

fn join_u16(v: &[u16]) -> String {
    if v.is_empty() {
        "-".to_string()
    } else {
        v.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// 写操作审计：AI 动了什么，界面上要能查到
fn audit(app: &AppHandle, tool: &str, action: &str, target: &str, ok: bool) {
    let state = app.state::<AppState>();
    let _ = with_config(&state, |cfg| {
        cfg.mcp_audit.insert(
            0,
            AuditItem {
                ts: now_sec(),
                tool: tool.to_string(),
                action: action.to_string(),
                target: target.to_string(),
                ok,
            },
        );
        cfg.mcp_audit.truncate(200);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_proto::{Cmd, Resp};
    use crate::{EnvVar, Project};

    fn proj(id: &str, name: &str) -> Project {
        Project {
            id: id.to_string(),
            name: name.to_string(),
            command: "echo hi".into(),
            cwd: "C:\\proj".into(),
            color: "#fff".into(),
            note: String::new(),
            kind: "command".into(),
            script_path: String::new(),
            interpreter: String::new(),
            args: String::new(),
            env: vec![EnvVar {
                key: "A".into(),
                value: "1".into(),
            }],
            group: String::new(),
            ports: String::new(),
        }
    }

    /// 用 `..Default::default()` 补齐其余字段：Config 以后再加字段时这里不必跟着改
    /// （逐字段列写法刚刚就因为新增 protected_ports 而编译失败）
    fn cfg_with(list: Vec<Project>) -> Config {
        Config {
            projects: list,
            mcp_token: "t".into(),
            mcp_port: 9527,
            ..Config::default()
        }
    }

    #[test]
    fn resolve_by_id_takes_priority() {
        let cfg = cfg_with(vec![proj("a", "同名"), proj("b", "同名")]);
        // id 命中时不看 name，避免同名项目歧义
        assert_eq!(resolve_project(&cfg, "b", "同名").unwrap().id, "b");
    }

    #[test]
    fn resolve_by_name_exact_then_partial() {
        let cfg = cfg_with(vec![proj("q1", "qidian-admin 后端"), proj("q2", "其他")]);
        assert_eq!(resolve_project(&cfg, "", "qidian-admin 后端").unwrap().id, "q1");
        assert_eq!(resolve_project(&cfg, "", "其他").unwrap().id, "q2");
        // 大小写不敏感
        assert_eq!(
            resolve_project(&cfg, "", "QIDIAN-ADMIN 后端").unwrap().id,
            "q1"
        );
    }

    #[test]
    fn resolve_ambiguous_name_asks_for_id() {
        let cfg = cfg_with(vec![proj("q1", "qidian 前端"), proj("q2", "qidian 后端")]);
        let e = resolve_project(&cfg, "", "qidian").unwrap_err();
        assert!(e.contains("匹配到多个项目"), "{e}");
        assert!(e.contains("q1") && e.contains("q2"), "{e}");
    }

    #[test]
    fn resolve_missing_project_lists_candidates() {
        let cfg = cfg_with(vec![proj("q1", "qidian 后端")]);
        let e = resolve_project(&cfg, "", "不存在").unwrap_err();
        assert!(e.contains("没有名称匹配"), "{e}");
        assert!(e.contains("qidian 后端"), "错误信息应附上现有项目：{e}");

        let e2 = resolve_project(&cfg, "ghost", "").unwrap_err();
        assert!(e2.contains("没有 id 为"), "{e2}");
    }

    #[test]
    fn resolve_requires_some_key() {
        let cfg = cfg_with(vec![proj("q1", "a")]);
        assert!(resolve_project(&cfg, "", "").is_err());
    }

    #[test]
    fn resp_roundtrip_keeps_error_and_data() {
        let ok = serde_json::to_string(&Resp::ok(serde_json::json!({"a": 1}))).unwrap();
        assert!(ok.contains("\"ok\":true"));
        assert!(!ok.contains("error"), "成功响应不应带 error 字段: {ok}");

        let err = serde_json::to_string(&Resp::err("boom")).unwrap();
        let back: Resp = serde_json::from_str(&err).unwrap();
        assert!(!back.ok);
        assert_eq!(back.error.as_deref(), Some("boom"));
        assert!(back.data.is_none());
    }

    /// 命令名与命令集合必须一一对应：这是"AI 调了但后端不认"的根防线。
    /// `as_str` 是穷举 match，所以只要这里的往返成立，两边就不会各漂各的。
    #[test]
    fn command_enum_roundtrips() {
        let mut seen = std::collections::HashSet::new();
        for c in Cmd::ALL {
            let name = c.as_str();
            assert!(!name.trim().is_empty(), "命令名不能为空");
            assert!(seen.insert(name), "控制口命令名重复: {name}");
            assert_eq!(Cmd::parse(name), Some(*c), "「{name}」应能解析回原命令");
            assert_eq!(Cmd::parse(c.as_str()), Some(*c));
        }
        // 大小写与空白不该被当成另一个命令
        assert_eq!(Cmd::parse("  status  "), Some(Cmd::Status));
        assert_eq!(Cmd::parse("STATUS"), None);
        assert_eq!(Cmd::parse("unknown_cmd"), None);
        assert_eq!(Cmd::parse(""), None);
    }

    #[test]
    fn every_command_name_is_reachable_from_parse() {
        let names = crate::control_proto::all_names();
        assert_eq!(names.len(), Cmd::ALL.len());
        for n in names {
            assert!(Cmd::parse(n).is_some(), "all_names() 里的「{n}」无法被 parse");
        }
    }

    /// 未知命令的错误信息要带上候选列表，AI 不必猜
    #[test]
    fn unknown_command_error_is_unknown_command() {
        assert!(Cmd::parse("nope").is_none());
    }

    #[test]
    fn required_arg_helpers_reject_bad_input() {
        // pid：数字与数字字符串都收，0 与越界交给 kill_pid 的守卫处理
        assert_eq!(arg_pid(&serde_json::json!({"pid": 42})).unwrap(), 42);
        assert_eq!(arg_pid(&serde_json::json!({"pid": "42"})).unwrap(), 42);
        assert!(arg_pid(&serde_json::json!({"pid": "abc"})).is_err());
        assert!(arg_pid(&serde_json::json!({})).is_err());
        assert!(arg_pid(&serde_json::json!({"pid": -1})).is_err());
        assert!(arg_pid(&serde_json::json!({"pid": 99999999999u64})).is_err());

        // 字符串参数：空白视为未提供，避免把空名字当成一次真实查询
        assert_eq!(arg_str(&serde_json::json!({"ports": " 8000 "}), "ports").unwrap(), "8000");
        assert!(arg_str(&serde_json::json!({"ports": "   "}), "ports").is_err());
        assert!(arg_str(&serde_json::json!({}), "ports").is_err());
        assert!(arg_str(&serde_json::json!({"ports": 1}), "ports").is_err());
    }

    /// 请求报文的容错：缺 token / 缺 args 不该让整个连接炸掉
    #[test]
    fn request_parsing_tolerates_missing_optional_fields() {
        let r: Req = serde_json::from_str(r#"{"cmd":"status"}"#).unwrap();
        assert!(r.token.is_empty());
        assert_eq!(r.cmd, "status");
        assert!(r.args.is_null());

        let r: Req = serde_json::from_str(r#"{"token":"t","cmd":"check_ports","args":{"ports":"80"}}"#)
            .unwrap();
        assert_eq!(r.token, "t");
        assert_eq!(r.args["ports"], "80");

        // 未知字段要能忽略，方便以后协议加字段时新旧两端共存
        assert!(serde_json::from_str::<Req>(r#"{"cmd":"status","future":1}"#).is_ok());
        // 缺 cmd 是硬错误
        assert!(serde_json::from_str::<Req>(r#"{"token":"t"}"#).is_err());
    }
}
