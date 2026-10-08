#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod control;
mod control_proto;
mod mcp_clients;
mod network;
mod platform;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sysinfo::System;
use tauri::{AppHandle, Emitter, Manager};

use platform::{PortInfo, ProcRow};

// ---------------- data types ----------------

fn default_kind() -> String {
    "command".to_string()
}

/// 受保护端口：DevToolkit 不会结束监听这些端口的进程，控制口顺延时也会绕开它们。
///
/// 默认值是用户常驻的本地项目（qidian-admin 后端 8000 / 前端 9528）。
/// 之所以放进配置而不是写死：换项目时不必改代码重新编译。
fn default_protected_ports() -> Vec<u16> {
    vec![8000, 9528]
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EnvVar {
    pub key: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// 命令行模式下的启动命令
    #[serde(default)]
    pub command: String,
    pub cwd: String,
    pub color: String,
    #[serde(default)]
    pub note: String,
    // ---- v2 新增字段（全部带 default，兼容旧配置） ----
    /// 启动方式：command（命令行） | script（脚本文件）
    #[serde(default = "default_kind")]
    pub kind: String,
    /// kind=script 时的脚本文件路径
    #[serde(default)]
    pub script_path: String,
    /// 解释器；留空按脚本扩展名自动识别
    #[serde(default)]
    pub interpreter: String,
    /// 附加启动参数（空格分隔）
    #[serde(default)]
    pub args: String,
    /// 项目级环境变量
    #[serde(default)]
    pub env: Vec<EnvVar>,
    /// 分组名，用于列表归类
    #[serde(default)]
    pub group: String,
    /// 启动前检测 / 端口关联，如 "3000,8080"
    #[serde(default)]
    pub ports: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HistoryItem {
    pub ts: u64,
    pub command: String,
    pub cwd: String,
    pub code: i32,
    pub duration_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct KillItem {
    pub ts: u64,
    pub pid: u32,
    pub name: String,
}

/// MCP（AI 直连）操作的审计条目：谁在什么时候动了什么，界面上要看得见
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AuditItem {
    pub ts: u64,
    /// 触发来源，目前固定是 mcp 命令名
    pub tool: String,
    /// 动作：start / stop / takeover / kill_pid
    pub action: String,
    pub target: String,
    pub ok: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub history: Vec<HistoryItem>,
    #[serde(default)]
    pub kills: Vec<KillItem>,
    // ---- MCP 控制口（v1.4 新增，全部带 default，兼容旧配置） ----
    /// 控制口访问令牌，首次启动时随机生成
    #[serde(default)]
    pub mcp_token: String,
    /// 控制口实际监听端口（默认 9527，被占用时会顺延，MCP 进程据此连接）
    #[serde(default)]
    pub mcp_port: u16,
    /// 用户期望的控制口起始端口（0 = 从默认段 9527 起扫）。
    /// 必须与 `mcp_port` 分开：后者记录的是"实际绑到了哪个端口"，顺延后会被改写，
    /// 拿它当偏好会让端口一路漂移（9527 被占 → 记 9528 → 下次从 9528 起…）。
    #[serde(default)]
    pub mcp_preferred_port: u16,
    /// AI 操作审计记录
    #[serde(default)]
    pub mcp_audit: Vec<AuditItem>,
    // ---- 端口保护（v1.5 新增，带 default，兼容旧配置） ----
    /// 受保护端口：监听这些端口的进程不允许被结束，控制口顺延也会绕开
    #[serde(default = "default_protected_ports")]
    pub protected_ports: Vec<u16>,
}

#[derive(Serialize, Clone, Debug)]
pub struct SysInfo {
    pub hostname: String,
    pub os: String,
    pub cpu_brand: String,
    pub cores: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct Metrics {
    pub cpu: f32,
    pub mem_total: u64,
    pub mem_used: u64,
    pub uptime: u64,
    pub ts: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct TaskInfo {
    pub id: String,
    pub project: Project,
    pub pid: u32,
    pub started_at: u64,
}

/// 接管结果：结束了哪些外部进程，以及接管后新起的任务
#[derive(Serialize, Clone, Debug)]
pub struct TakeoverResult {
    pub killed_pids: Vec<u32>,
    pub task: TaskInfo,
}

#[derive(Serialize, Debug)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
    pub duration_ms: u64,
    /// 是否因为超过 EXEC_TIMEOUT 被强杀。code 会是 -1，但 -1 也可能是命令自己返回的，
    /// 界面需要区分"命令退出码是 -1"和"我们把命令杀了"。
    pub timed_out: bool,
    /// 输出是否被 EXEC_OUTPUT_CAP 截断。以前是静默截断，用户只看到半截日志却不知道。
    pub truncated: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct KillResult {
    pub ok: bool,
    pub message: String,
}

/// 项目配置体检结果：保存前把"启动一定会失败"的原因提前说清楚
#[derive(Serialize, Clone, Debug, Default)]
pub struct ProjectCheck {
    /// 阻断保存 / 启动的问题
    pub errors: Vec<String>,
    /// 不阻断但大概率会出问题，比如工作目录不存在
    pub warnings: Vec<String>,
}

#[derive(Serialize, Debug)]
pub struct ProcDetail {
    pub cmdline: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct PortConflict {
    pub port: u16,
    pub pid: u32,
    pub process_name: String,
}

/// 项目运行态判定来源
const SRC_TASK: &str = "task"; // 由本应用启动
const SRC_PORT: &str = "port"; // 声明的端口正在被监听
const SRC_PATH: &str = "path"; // 监听进程的工作目录 / 命令行命中项目目录

/// 单个项目的运行态：端口页与项目管理页共用，避免"各算一套"导致口径不一致
#[derive(Serialize, Clone, Debug)]
pub struct ProjectRuntime {
    pub id: String,
    pub name: String,
    /// 本应用启动的任务 id；外部启动时为空字符串
    pub task_id: String,
    /// 是否正在运行（任务 / 声明端口 / 进程目录 任一命中）
    pub running: bool,
    /// 运行中且不是本应用启动的（例如在终端里手工起的服务）
    pub external: bool,
    /// 判定来源：task | port | path
    pub source: String,
    /// 实际正在监听的端口
    pub ports: Vec<u16>,
    /// 对应进程 PID（用于结束外部进程）
    pub pids: Vec<u32>,
    /// 代表进程名，便于提示
    pub process_name: String,
}

/// 一次扫描的完整结果：全量端口 + 项目运行态
#[derive(Serialize, Clone, Debug)]
pub struct ScanResult {
    pub ports: Vec<PortInfo>,
    pub runtime: Vec<ProjectRuntime>,
}

#[derive(Clone, Debug)]
struct RunningTask {
    project: Project,
    pid: u32,
    started_at: u64,
}

struct AppState {
    tasks: Mutex<HashMap<String, RunningTask>>,
    config_path: Mutex<String>,
    /// 控制口句柄；绑定失败为 None（只是 MCP 不可用，GUI 照常）
    control: Mutex<Option<Arc<control::Ctx>>>,
    /// 任务日志环形缓冲：MCP 侧靠它读日志，GUI 重载后也能补看
    logs: Mutex<HashMap<String, VecDeque<String>>>,
    /// 配置文件写锁：GUI 与控制口并发写会互相覆盖
    cfg_lock: Mutex<()>,
    /// 端口列表短 TTL 缓存：端口页 / 项目管理页 / MCP 可能在同一秒内各扫一次，
    /// 而每次扫描都要起 netstat + tasklist 两个子进程，没必要重复
    ports_cache: Mutex<Vec<PortInfo>>,
    ports_cache_at: Mutex<Option<Instant>>,
}

/// 单个任务保留的日志行数上限
const TASK_LOG_CAP: usize = 400;
/// 日志缓冲最多保留多少个任务的记录
const TASK_LOG_BUCKETS: usize = 50;
/// 任务 id 序号，见 `next_task_id`
static TASK_SEQ: AtomicU64 = AtomicU64::new(0);

/// 端口扫描结果的缓存有效期。取 1 秒：足够吸收"同一次刷新里三个页面各问一遍"，
/// 又短到用户点一下刷新就能看到真实变化。
const PORTS_CACHE_TTL: Duration = Duration::from_millis(1000);

/// 托管项目数量上限：配置是整份读写的，无上限时一个异常的大数组会让每次操作都变慢
const MAX_PROJECTS: usize = 500;
/// 端口区间写法（3000-3010）一次最多展开多少个
const MAX_PORT_RANGE_SPAN: u16 = 256;

/// `exec_command` 默认超时。没有超时的话，一条 `ping -t` 就能把处理该请求的
/// IPC 线程永久占住，界面表现为"快速命令页彻底卡死"。
const EXEC_TIMEOUT: Duration = Duration::from_secs(60);
/// 超时后等待读取线程收尾的时间；管道理论上会随进程树结束而关闭，但不赌它
const EXEC_READER_GRACE: Duration = Duration::from_secs(2);
const EXEC_POLL: Duration = Duration::from_millis(50);
/// 单个流最多回传多少字符，避免一条刷屏命令把响应撑爆
const EXEC_OUTPUT_CAP: usize = 256 * 1024;

/// 不允许结束的进程：0 = System Idle Process，4 = System（Windows 内核态关键进程）
const PROTECTED_PIDS: &[u32] = &[0, 4];

/// 受保护端口名单上限：名单是整份读写的，不设上限时一个异常的长列表会拖慢每次结束操作
const MAX_PROTECTED_PORTS: usize = 64;

// ---------------- helpers ----------------

/// 取锁，**中毒也继续用**。
///
/// 这里被保护的都是"可自愈"的数据（任务表、日志缓冲、配置路径、端口缓存），
/// 没有需要靠 panic 来守住的强不变量。反过来，任何一处 `lock().unwrap()`
/// 在别的线程 panic 之后都会二次 panic——一个日志读取线程的意外足够带走整个应用。
fn lockx<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn now_sec() -> u64 {
    now_ms() / 1000
}

impl Default for Config {
    fn default() -> Self {
        Config {
            projects: vec![],
            history: vec![],
            kills: vec![],
            mcp_token: String::new(),
            mcp_port: 0,
            mcp_preferred_port: 0,
            mcp_audit: vec![],
            protected_ports: default_protected_ports(),
        }
    }
}

/// 任务 id：毫秒 + 单调序号。
///
/// 只用毫秒时间戳是不够的：批量启动时两次 `start_task` 完全可能落在同一毫秒，
/// id 一撞，后一个任务会把前一个从任务表里覆盖掉——被覆盖的那个进程从此
/// 既停不掉也看不见，直接变成孤儿进程。
fn next_task_id() -> String {
    let n = TASK_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("t{}-{}", now_ms(), n)
}

fn config_bak_path(path: &str) -> String {
    format!("{path}.bak")
}

/// 解析配置文件；文件不存在或内容损坏都返回 None（由调用方决定回退策略）
fn parse_config_file(path: &str) -> Option<Config> {
    let s = fs::read_to_string(path).ok()?;
    serde_json::from_str::<Config>(&s).ok()
}

/// 读配置。
///
/// 主文件损坏时退回 `.bak`：配置是"托管了哪些项目"的唯一真相，被截断或手工改坏
/// 如果直接回退到空配置，用户看到的就是"我的项目全没了"，而且下一次保存就把空配置写实了。
fn read_config(path: &str) -> Config {
    if let Some(c) = parse_config_file(path) {
        return c;
    }
    if Path::new(path).exists() {
        let bak = config_bak_path(path);
        if let Some(c) = parse_config_file(&bak) {
            eprintln!("[config] 主配置不可解析，已从备份恢复: {bak}");
            return c;
        }
        eprintln!("[config] 主配置不可解析且无可用备份，本次以空配置启动: {path}");
    }
    Config::default()
}

/// 原子落盘 + 留一份备份。
///
/// 直接 `fs::write` 到目标文件，会在写入过程中被断电/崩溃/安全软件拦截打断，
/// 留下一个半截 JSON——下次启动就变成"配置损坏"。改为同目录写临时文件再 rename，
/// rename 在本地文件系统上是原子的，读到的要么是旧内容要么是新内容。
fn write_config(path: &str, cfg: &Config) -> Result<(), String> {
    let s = serde_json::to_string_pretty(cfg).map_err(|e| format!("配置序列化失败: {e}"))?;
    let tmp = format!("{path}.tmp");
    fs::write(&tmp, s).map_err(|e| format!("写入临时配置失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("替换配置失败: {e}"))?;
    // 落盘成功后再留副本：备份要对应"最后一次成功的保存"，恢复时才是当下最接近的状态。
    // 备份失败不阻断保存——它是保险，不是前置条件。
    if let Err(e) = fs::copy(path, config_bak_path(path)) {
        eprintln!("[config] 备份失败（忽略）: {e}");
    }
    Ok(())
}

/// 读-改-写配置。加写锁是必要的：控制口来的 AI 操作与界面上的手工操作会并发落盘。
fn with_config<T>(state: &AppState, f: impl FnOnce(&mut Config) -> T) -> Result<T, String> {
    let _guard = lockx(&state.cfg_lock);
    let path = lockx(&state.config_path).clone();
    let mut cfg = read_config(&path);
    let r = f(&mut cfg);
    write_config(&path, &cfg)?;
    Ok(r)
}

// ---------------- system commands ----------------

#[tauri::command]
fn sys_info() -> SysInfo {
    let sys = System::new_all();
    SysInfo {
        hostname: System::host_name().unwrap_or_default(),
        os: System::long_os_version().unwrap_or_else(|| "Unknown".into()),
        cpu_brand: sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_default(),
        cores: sys.cpus().len(),
    }
}

/// 带短 TTL 缓存的端口列表。端口页、项目管理页、控制口会在同一秒内各问一遍，
/// 每次都起 netstat + tasklist 两个子进程是纯浪费。
///
/// 注意：**启动前的端口冲突检测不走这里**（`find_port_conflicts` 直连系统调用）。
/// "结束外部进程后立刻重启"这类流程对时效性敏感，吃 1 秒前的缓存会误报冲突。
fn cached_ports(state: &AppState) -> Result<Vec<PortInfo>, String> {
    // 两把锁分别取，不嵌套：写入路径是 ports_cache → ports_cache_at，
    // 若这里反过来嵌套就成了典型的锁序反转，迟早死锁
    let fresh_enough = lockx(&state.ports_cache_at)
        .map(|t| t.elapsed() < PORTS_CACHE_TTL)
        .unwrap_or(false);
    if fresh_enough {
        let cached = lockx(&state.ports_cache).clone();
        if !cached.is_empty() {
            return Ok(cached);
        }
    }
    let fresh = platform::list_ports()?;
    *lockx(&state.ports_cache) = fresh.clone();
    *lockx(&state.ports_cache_at) = Some(Instant::now());
    Ok(fresh)
}

#[tauri::command]
fn list_ports(app: AppHandle) -> Result<Vec<PortInfo>, String> {
    cached_ports(&app.state::<AppState>())
}

// ---------------- 端口保护 ----------------
//
// 「别动我的本地项目」不能只写在文档里：端口页的"结束"、项目页的"停止"、
// 以及 AI 经控制口调的 kill_pid / stop_project / takeover_project，全都汇到
// kill 这一个动作上。所以闸门装在 `protected_hits`，而不是散在各处各判一次。

/// 读取受保护端口名单。名单随配置存盘，每次结束操作都重新读，改完立即生效。
pub fn protected_port_list(app: &AppHandle) -> Vec<u16> {
    let state = app.state::<AppState>();
    let path = lockx(&state.config_path).clone();
    read_config(&path).protected_ports
}

/// 从端口表里筛出「该 PID 占用、且端口在保护名单里」的端口（升序去重）。
///
/// 逻辑抽成纯函数是为了能单测：`protected_hits` 负责取数据，判断只在这一处，
/// 免得以后改了判定条件却没测到。
fn match_protected(ports: &[PortInfo], pid: u32, protected: &[u16]) -> Vec<u16> {
    let mut hit: Vec<u16> = ports
        .iter()
        .filter(|p| p.pid == pid && protected.contains(&p.local_port))
        .map(|p| p.local_port)
        .collect();
    hit.sort_unstable();
    hit.dedup();
    hit
}

/// 目标 PID 正在监听的受保护端口。空表示未命中保护。
///
/// 端口表读不出来时返回 Err 而不是空：这一步存在的唯一目的就是"不误杀"，
/// 宁可让调用方看到一句明确的拒绝原因，也不要赌一次"应该没事"。
fn protected_hits(app: &AppHandle, pid: u32) -> Result<Vec<u16>, String> {
    let protected = protected_port_list(app);
    if protected.is_empty() {
        return Ok(Vec::new());
    }
    let state = app.state::<AppState>();
    let ports = cached_ports(&state).map_err(|e| format!("端口占用读取失败（{e}）"))?;
    Ok(match_protected(&ports, pid, &protected))
}

/// 关闭应用时**必须留下**的托管任务 PID。
///
/// 不能只看「任务进程自己有没有监听受保护端口」：任务是用 `cmd /C` 拉起的，
/// `t.pid` 是 cmd 那一层，它自己不监听任何端口，命中判定必然为空 ——
/// 那条判断于是形同虚设；而结束动作用的是 `taskkill /F /T`（连进程树杀），
/// 结果就是关掉 DevToolkit，顺手把用户在 8000/9528 上的服务一起带走。
///
/// 所以判定分两层，命中任一层即保留：
/// 1. 任务进程自己就在受保护端口上监听（`match_protected`）；
/// 2. 本项目**声明且目前正在监听**的端口中，有落在保护名单上的
///    （借 `runtime_by_ports` 展开声明，端口表达式不必再解析一遍）。
fn keep_alive_tasks(tasks: &[(u32, Project)], ports: &[PortInfo], protected: &[u16]) -> Vec<u32> {
    if protected.is_empty() {
        return Vec::new();
    }
    let projects: Vec<Project> = tasks.iter().map(|(_, p)| p.clone()).collect();
    let runtime = runtime_by_ports(&projects, ports);
    tasks
        .iter()
        .filter(|(pid, p)| {
            !match_protected(ports, *pid, protected).is_empty()
                || runtime
                    .iter()
                    .find(|r| r.id == p.id)
                    .map(|r| r.ports.iter().any(|x| protected.contains(x)))
                    .unwrap_or(false)
        })
        .map(|(pid, _)| *pid)
        .collect()
}

/// 结束动作的上下文。保护名单是否放行，只由它决定 —— 判断只在这一个维度上，
/// 免得以后各调用点自己拍脑袋决定"这个应该能杀吧"。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum KillScope {
    /// 裸结束：调用方只递来一个 PID，不知道它属于谁。
    /// 进程页 / 端口页的「结束」按钮、控制口的裸 `kill_pid` 都走这条。
    /// 目标可能是用户的任何服务，受保护端口上一律拒绝。
    Blind,
    /// 项目自身的启停：目标已被"本项目"限死 —— 要么是本应用自己托管的任务进程，
    /// 要么是占用本项目声明端口的进程（`runtime_by_ports` 已把 pids 限定在声明端口上）。
    /// 这两种都不是"误杀无关服务"，受保护端口照样放行。
    Project,
}

/// 命中保护时的统一拒绝文案：必须同时说清"为什么被拒"和"去哪儿解除"，
/// 否则用户只会看到一句"结束失败"，找不到放开的地方。
fn protected_refuse(pid: u32, ports: &[u16]) -> String {
    format!(
        "PID {pid} 正在监听受保护端口 {}。DevToolkit 不会结束它；如确需操作，请先在「端口」页解除该端口的保护。",
        join_ports(ports)
    )
}

/// 端口列表转显示用字符串
fn join_ports(ports: &[u16]) -> String {
    ports
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[tauri::command]
fn check_project(project: Project) -> ProjectCheck {
    inspect_project(&project)
}

/// 端口占用预检（供前端在启动项目前调用）
#[tauri::command]
async fn check_ports(ports: String) -> Result<Vec<PortConflict>, String> {
    find_port_conflicts(&ports)
}

#[tauri::command]
fn list_processes() -> Result<Vec<ProcRow>, String> {
    platform::list_processes()
}

#[tauri::command]
fn process_detail(pid: u32) -> Result<ProcDetail, String> {
    Ok(ProcDetail {
        cmdline: platform::process_detail(pid)?,
    })
}

/// 裸结束：界面按钮与控制口的 `kill_pid` 都走这条，受保护端口一律拒绝。
#[tauri::command]
fn kill_pid(app: AppHandle, pid: u32) -> Result<KillResult, String> {
    kill_pid_scoped(&app, pid, KillScope::Blind)
}

/// 结束进程的公共实现。能否在受保护端口上动手，只由 `scope` 决定。
pub(crate) fn kill_pid_scoped(
    app: &AppHandle,
    pid: u32,
    scope: KillScope,
) -> Result<KillResult, String> {
    // 拒绝自杀：控制口请求由本进程处理，杀掉自己会让 AI 侧拿到一个没有回应的连接，
    // 而且会把界面托管的全部任务一起带走
    if pid == std::process::id() {
        return Ok(KillResult {
            ok: false,
            message: "不能结束 DevToolkit 自身进程。若需重启应用，请由用户在界面或任务栏操作。".into(),
        });
    }
    // PID 0 / 4 不是普通进程：0 是 System Idle Process，4 是内核态的 System。
    // 对它们执行 taskkill /T 的结果不可预期，直接挡在门口。
    if PROTECTED_PIDS.contains(&pid) {
        return Ok(KillResult {
            ok: false,
            message: format!("PID {pid} 属于系统关键进程，DevToolkit 不处理"),
        });
    }
    if !process_exists(pid) {
        return Ok(KillResult {
            ok: false,
            message: format!("PID {pid} 不存在或已退出"),
        });
    }

    // 受保护端口：只有"裸结束"才拦。
    //
    // 这道闸门原先对所有调用一视同仁，结果把工具自己关在了门外：用户注册的两个项目
    // （8000 / 9528）正好在默认保护名单上，于是「启动」说去接管、「接管」说端口受保护、
    // 「停止」连自己托管的任务也不让停、裸 kill 更被拒 —— 四条路全堵死，项目永远起不来。
    //
    // 区别在于调用方知不知道自己在动谁：Blind 只拿到一个 PID，可能是用户任何服务，必须拦；
    // Project 的目标已被"本项目声明端口"限死（见 `runtime_by_ports`），属于正常启停，放行。
    if scope == KillScope::Blind {
        match protected_hits(app, pid) {
            Ok(hit) if !hit.is_empty() => {
                return Ok(KillResult {
                    ok: false,
                    message: protected_refuse(pid, &hit),
                });
            }
            Err(e) => {
                return Ok(KillResult {
                    ok: false,
                    message: format!("为避免误杀，已拒绝结束 PID {pid}：{e}"),
                });
            }
            _ => {}
        }
    }

    let (ok, message) = platform::kill_process(pid)?;

    if ok {
        let name = platform::list_processes()
            .ok()
            .and_then(|l| l.into_iter().find(|p| p.pid == pid))
            .map(|p| p.name)
            .unwrap_or_default();
        let state = app.state::<AppState>();
        let _ = with_config(&state, |cfg| {
            cfg.kills.insert(
                0,
                KillItem {
                    ts: now_sec(),
                    pid,
                    name,
                },
            );
            cfg.kills.truncate(50);
        });
    }

    Ok(KillResult { ok, message })
}

#[tauri::command]
fn get_config(app: AppHandle) -> Result<Config, String> {
    let state = app.state::<AppState>();
    let path = lockx(&state.config_path).clone();
    Ok(read_config(&path))
}

/// 切换某个端口的保护状态，返回切换后的完整名单（前端直接拿它刷新界面，
/// 不必再整份拉一次配置）。
#[tauri::command]
fn set_port_protected(app: AppHandle, port: u16, protected: bool) -> Result<Vec<u16>, String> {
    if port == 0 {
        return Err("端口号无效".into());
    }
    let state = app.state::<AppState>();
    with_config(&state, |cfg| {
        cfg.protected_ports.retain(|p| *p != port);
        if protected {
            cfg.protected_ports.push(port);
            cfg.protected_ports.sort_unstable();
            cfg.protected_ports.truncate(MAX_PROTECTED_PORTS);
        }
        cfg.protected_ports.clone()
    })
}

#[tauri::command]
fn save_projects(app: AppHandle, projects: Vec<Project>) -> Result<(), String> {
    // 严格校验：脏配置一旦落盘，运行期到处都要做防御
    validate_projects(&projects)?;

    let state = app.state::<AppState>();
    // 正在运行的项目不能被"保存"掉：保存后任务表里还留着它，但配置里没有了，
    // 于是它既停不掉（界面无从展示）也无法再启动（查重仍认为在跑），成为幽灵任务
    {
        let tasks = lockx(&state.tasks);
        for (_, t) in tasks.iter() {
            if !projects.iter().any(|p| p.id == t.project.id) {
                return Err(format!(
                    "项目「{}」正在运行中，请先停止再删除",
                    t.project.name
                ));
            }
        }
    }

    with_config(&state, |cfg| {
        cfg.projects = projects;
    })
}

// ---------------- task (project runner) ----------------

/// 单个端口号：1-65535。0 不是可用的监听端口，写 0 通常是把默认值当真了
fn parse_port_num(s: &str) -> Option<u16> {
    let n: u32 = s.trim().parse().ok()?;
    if n == 0 || n > u16::MAX as u32 {
        return None;
    }
    Some(n as u16)
}

/// 宽松解析端口声明，用于**运行时判定**：`"3000,8080 9000"`、`"3000-3010"`。
///
/// 这里刻意不报错——扫描运行态不能因为配置里有一个写错的端口号就整体失败。
/// 严格校验走 `validate_port_spec`，在保存配置时执行。
fn parse_port_spec(spec: &str) -> Vec<u16> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in spec.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let s = raw.trim();
        if s.is_empty() {
            continue;
        }
        let (lo, hi) = match s.split_once('-') {
            Some((a, b)) => (a, b),
            None => (s, s),
        };
        let (Some(a), Some(b)) = (parse_port_num(lo), parse_port_num(hi)) else {
            continue;
        };
        if a > b || b - a > MAX_PORT_RANGE_SPAN {
            continue;
        }
        for p in a..=b {
            if seen.insert(p) {
                out.push(p);
            }
        }
    }
    out
}

/// 严格校验端口声明，用于**保存配置**时。返回展开后的端口列表或第一条错误原因。
///
/// 与宽松版分开的原因：以前非法项是被静默丢弃的，用户写 `3000-3010` 以为配了区间，
/// 实际一个端口都没生效，而界面上什么提示都没有。这类"静默失效"必须变成显式报错。
fn validate_port_spec(spec: &str) -> Result<Vec<u16>, String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in spec.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let s = raw.trim();
        if s.is_empty() {
            continue;
        }
        let (lo, hi) = match s.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (s, s),
        };
        let a = parse_port_num(lo).ok_or_else(|| format!("端口「{raw}」不是 1-65535 之间的数字"))?;
        let b = parse_port_num(hi).ok_or_else(|| format!("端口「{raw}」不是 1-65535 之间的数字"))?;
        if a > b {
            return Err(format!("端口区间「{raw}」起止颠倒，应写作 {b}-{a}"));
        }
        if b - a > MAX_PORT_RANGE_SPAN {
            return Err(format!(
                "端口区间「{raw}」跨度 {}(个) 过大，最多 {MAX_PORT_RANGE_SPAN} 个",
                b - a + 1
            ));
        }
        for p in a..=b {
            if seen.insert(p) {
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// 校验整份项目列表。**保存路径上唯一的守门人**：
/// 脏配置一旦落盘，运行期到处都要做防御，不如在入口拦住。
fn validate_projects(projects: &[Project]) -> Result<(), String> {
    if projects.len() > MAX_PROJECTS {
        return Err(format!(
            "项目数量 {} 超过上限 {MAX_PROJECTS}",
            projects.len()
        ));
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for p in projects {
        let label = if p.name.trim().is_empty() {
            format!("id={}", p.id.trim())
        } else {
            format!("「{}」", p.name.trim())
        };

        if p.id.trim().is_empty() {
            return Err(format!("项目 {label} 缺少 id"));
        }
        if !ids.insert(p.id.trim().to_string()) {
            return Err(format!("项目 id「{}」重复，请给它一个唯一 id", p.id.trim()));
        }
        if p.name.trim().is_empty() {
            return Err(format!("项目 {label} 缺少名称"));
        }
        if !names.insert(p.name.trim().to_lowercase()) {
            // 同名不致命，但会让 MCP 按名字定位时必须消歧。错误信息要指到具体名字上，
            // 否则用户在"改了 A 却因为 B 存不上"的循环里出不来。
            return Err(format!(
                "有多个项目都叫「{}」（至少两个），按名字调用时无法区分，请改名后再保存",
                p.name.trim()
            ));
        }
        if p.cwd.trim().is_empty() {
            return Err(format!("项目 {label} 缺少工作目录"));
        }

        match p.kind.trim() {
            "command" => {
                if p.command.trim().is_empty() {
                    return Err(format!("项目 {label} 是命令行模式，但启动命令为空"));
                }
            }
            "script" => {
                if p.script_path.trim().is_empty() {
                    return Err(format!("项目 {label} 是脚本模式，但脚本文件路径为空"));
                }
            }
            other => {
                return Err(format!(
                    "项目 {label} 的启动方式「{other}」无效，只能是 command 或 script"
                ));
            }
        }

        validate_port_spec(&p.ports).map_err(|e| format!("项目 {label} {e}"))?;

        for e in &p.env {
            let k = e.key.trim();
            if k.is_empty() {
                continue;
            }
            if !is_valid_env_key(k) {
                return Err(format!(
                    "项目 {label} 的环境变量名「{k}」非法（不能含空格或 = ）"
                ));
            }
        }
    }
    Ok(())
}

/// 环境变量名合法性：Windows 上 `set "K=V"` 的形式决定了名字里不能有空格和等号
fn is_valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && !key.contains('=')
        && !key.chars().any(|c| c.is_whitespace() || c == '"' || c == '\0')
}

/// 解释器 / 命令是否能在 PATH 里找到（也接受直接给的绝对路径）
fn command_in_path(cmd: &str) -> bool {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return false;
    }
    if Path::new(cmd).is_file() {
        return true;
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat", ".com"]
    } else {
        &[""]
    };
    std::env::split_paths(&path).any(|dir| {
        exts.iter()
            .any(|ext| dir.join(format!("{cmd}{ext}")).is_file())
    })
}

/// 项目体检：保存前把"一启动就会失败"的原因说清楚，而不是等点启动才报错
fn inspect_project(project: &Project) -> ProjectCheck {
    let mut out = ProjectCheck::default();

    if let Err(e) = validate_projects(std::slice::from_ref(project)) {
        out.errors.push(e);
    }

    if !project.cwd.trim().is_empty() && !Path::new(project.cwd.trim()).is_dir() {
        out.warnings
            .push(format!("工作目录不存在：{}", project.cwd.trim()));
    }

    if project.kind.trim() == "script" {
        let sp = project.script_path.trim();
        if !sp.is_empty() && !Path::new(sp).is_file() {
            out.warnings.push(format!("脚本文件不存在：{sp}"));
        }
        if !sp.is_empty() {
            match platform::resolve_interpreter(sp, project.interpreter.trim()) {
                Ok(interp) if !command_in_path(&interp) => out
                    .warnings
                    .push(format!("解释器「{interp}」不在 PATH 中，脚本可能无法启动")),
                Err(e) => out.errors.push(e),
                _ => {}
            }
        }
    }

    if project.kind.trim() == "command" && !project.command.trim().is_empty() {
        // 只认第一个词：`npm run dev` 需要 npm 可用，`set FOO=1 && npm run dev` 这种不管
        let head = project.command.trim().split_whitespace().next().unwrap_or("");
        if !head.is_empty() && !head.contains('=') && !head.starts_with('"') && !command_in_path(head)
        {
            out.warnings
                .push(format!("命令「{head}」不在 PATH 中，请确认已安装且可用"));
        }
    }

    if let Ok(ports) = validate_port_spec(&project.ports) {
        if ports.len() > 32 {
            out.warnings.push(format!(
                "声明了 {} 个端口，端口检测会变慢，建议只写关键端口",
                ports.len()
            ));
        }
    }

    out
}

/// TCP 只有 LISTENING 才算真正占坑；UDP 无连接态，绑定即计入
fn is_listening(p: &PortInfo) -> bool {
    p.proto == "UDP" || (p.proto == "TCP" && p.state == "LISTENING")
}

/// 解析端口声明，返回其中已被占用的端口
fn find_port_conflicts(spec: &str) -> Result<Vec<PortConflict>, String> {
    let wanted: HashSet<u16> = parse_port_spec(spec).into_iter().collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in platform::list_ports()? {
        if !wanted.contains(&p.local_port) || !is_listening(&p) {
            continue;
        }
        if !seen.insert(p.local_port) {
            continue;
        }
        out.push(PortConflict {
            port: p.local_port,
            pid: p.pid,
            process_name: p.process_name,
        });
    }
    out.sort_by_key(|c| c.port);
    Ok(out)
}

/// 依据「声明端口是否正在被监听」判定运行态（纯函数，不带系统调用，便于测试）
fn runtime_by_ports(projects: &[Project], ports: &[PortInfo]) -> Vec<ProjectRuntime> {
    let mut out: Vec<ProjectRuntime> = projects
        .iter()
        .map(|p| ProjectRuntime {
            id: p.id.clone(),
            name: p.name.clone(),
            task_id: String::new(),
            running: false,
            external: false,
            source: String::new(),
            ports: Vec::new(),
            pids: Vec::new(),
            process_name: String::new(),
        })
        .collect();

    for (i, p) in projects.iter().enumerate() {
        let wanted = parse_port_spec(&p.ports);
        if wanted.is_empty() {
            continue;
        }
        for info in ports.iter().filter(|x| is_listening(x)) {
            if !wanted.contains(&info.local_port) {
                continue;
            }
            let slot = &mut out[i];
            if !slot.ports.contains(&info.local_port) {
                slot.ports.push(info.local_port);
            }
            if !slot.pids.contains(&info.pid) {
                slot.pids.push(info.pid);
            }
            if slot.process_name.is_empty() {
                slot.process_name = info.process_name.clone();
            }
        }
        out[i].ports.sort_unstable();
        if !out[i].ports.is_empty() {
            out[i].running = true;
            out[i].source = SRC_PORT.to_string();
        }
    }
    out
}

/// 目录命中判定：精确等于项目目录，或位于项目目录之下（避免 D:\www\app 误匹配 D:\www\app2）
fn dir_hit(dir_lower: &str, candidate: &str) -> bool {
    let c = candidate
        .trim()
        .trim_matches('"')
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase();
    if c.is_empty() || dir_lower.is_empty() {
        return false;
    }
    c == dir_lower
        || c.starts_with(&format!("{dir_lower}\\"))
        || c.starts_with(&format!("{dir_lower}/"))
}

/// 未声明端口的项目：用「正在监听进程」的工作目录 / 命令行反查归属
fn apply_path_match(runtime: &mut [ProjectRuntime], projects: &[Project], ports: &[PortInfo]) {
    let targets: Vec<usize> = projects
        .iter()
        .enumerate()
        .filter(|(i, p)| {
            !runtime[*i].running && parse_port_spec(&p.ports).is_empty() && !p.cwd.trim().is_empty()
        })
        .map(|(i, _)| i)
        .collect();
    if targets.is_empty() {
        return;
    }

    // 只看正在监听的进程，避免把编译之类短命进程误认成服务
    let listen_pids: HashSet<u32> = ports
        .iter()
        .filter(|p| is_listening(p))
        .map(|p| p.pid)
        .collect();
    if listen_pids.is_empty() {
        return;
    }

    let sys = System::new_all();
    let self_pid = std::process::id();
    for i in targets {
        let dir_lower = projects[i]
            .cwd
            .trim()
            .trim_end_matches(['\\', '/'])
            .to_ascii_lowercase();
        for pid in &listen_pids {
            // 自己不算：DevToolkit 自身也监听控制口，且它的 exe 可能正落在项目目录下，
            // 一旦被认成"项目进程"，界面会把自身显示成外部运行，takeover 甚至会杀掉自己
            if *pid == self_pid {
                continue;
            }
            let Some(proc) = sys.process(sysinfo::Pid::from_u32(*pid)) else {
                continue;
            };
            let cwd = proc
                .cwd()
                .map(|c| c.to_string_lossy().to_string())
                .unwrap_or_default();
            let cmd = proc.cmd().join(" ");
            if !dir_hit(&dir_lower, &cwd) && !dir_hit(&dir_lower, &cmd) {
                continue;
            }

            let slot = &mut runtime[i];
            slot.running = true;
            slot.source = SRC_PATH.to_string();
            if !slot.pids.contains(pid) {
                slot.pids.push(*pid);
            }
            if slot.process_name.is_empty() {
                slot.process_name = ports
                    .iter()
                    .find(|x| x.pid == *pid)
                    .map(|x| x.process_name.clone())
                    .unwrap_or_else(|| format!("PID {pid}"));
            }
            for info in ports.iter().filter(|x| is_listening(x) && x.pid == *pid) {
                if !slot.ports.contains(&info.local_port) {
                    slot.ports.push(info.local_port);
                }
            }
        }
        runtime[i].ports.sort_unstable();
    }
}

/// 一次扫描同时产出「全量端口」和「项目运行态」：
/// 端口页与项目管理页都从这里取数，保证两页口径统一。
#[tauri::command]
async fn scan_ports(app: AppHandle, projects: Vec<Project>) -> Result<ScanResult, String> {
    let ports = cached_ports(&app.state::<AppState>())?;
    let mut runtime = runtime_by_ports(&projects, &ports);

    // 本应用启动的任务优先（有日志可看、可正常 stop）
    {
        let state = app.state::<AppState>();
        let tasks = lockx(&state.tasks);
        for (task_id, t) in tasks.iter() {
            if let Some(r) = runtime.iter_mut().find(|r| r.id == t.project.id) {
                r.running = true;
                r.task_id = task_id.clone();
                if r.source.is_empty() {
                    r.source = SRC_TASK.to_string();
                }
            }
        }
    }

    apply_path_match(&mut runtime, &projects, &ports);

    for r in runtime.iter_mut() {
        r.external = r.running && r.task_id.is_empty();
    }

    Ok(ScanResult { ports, runtime })
}

/// 按项目配置构造最终启动命令：脚本走解释器，命令行走 shell
fn build_project_command(project: &Project) -> Result<Command, String> {
    if project.kind == "script" {
        let path = project.script_path.trim();
        if path.is_empty() {
            return Err("脚本模式需要指定脚本文件路径".into());
        }
        platform::spawn_script_command(
            path,
            project.interpreter.trim(),
            project.args.trim(),
            &project.cwd,
        )
    } else {
        let command = project.command.trim();
        if command.is_empty() {
            return Err("命令行模式需要填写启动命令".into());
        }
        platform::spawn_shell_command(command, &project.cwd)
    }
}

/// 人类可读的启动命令描述，只用于日志展示
fn describe_project_cmd(project: &Project) -> String {
    if project.kind == "script" {
        let interp = if project.interpreter.trim().is_empty() {
            "自动"
        } else {
            project.interpreter.trim()
        };
        format!(
            "[{}] {} {}",
            interp,
            project.script_path.trim(),
            project.args.trim()
        )
        .trim_end()
        .to_string()
    } else {
        project.command.trim().to_string()
    }
}

fn spawn_task(app: AppHandle, task_id: String, project: Project) -> Result<TaskInfo, String> {
    let state = app.state::<AppState>();
    if !Path::new(&project.cwd).is_dir() {
        return Err(format!("工作目录不存在: {}", project.cwd));
    }

    // 「查重」与「登记」必须在同一个临界区完成。
    // 原先先查后插，中间夹着端口扫描和进程创建（几十到几百毫秒），批量启动或
    // 界面与 MCP 同时点同一个项目时，两个请求都能通过查重 → 同一项目被启两份。
    // 这里先落一个 pid=0 的占位，后面拿到真实 pid 再补上。
    {
        let mut tasks = lockx(&state.tasks);
        if tasks.values().any(|t| t.project.id == project.id) {
            return Err(format!("项目「{}」已在运行中", project.name));
        }
        tasks.insert(
            task_id.clone(),
            RunningTask {
                project: project.clone(),
                pid: 0,
                started_at: now_sec(),
            },
        );
    }

    // 从这里到 spawn 成功之间的任何失败都必须撤掉占位，否则项目会永久显示"运行中"、
    // 且再也启动不了（查重会一直拒绝）
    let prepared = (|| -> Result<std::process::Child, String> {
        // 启动前端口冲突检测：把「端口被占导致起不来」提前暴露，而不是等进程自己报错
        let conflicts = find_port_conflicts(&project.ports)?;
        if !conflicts.is_empty() {
            let detail = conflicts
                .iter()
                .map(|c| format!("{}（被 {} PID {} 占用）", c.port, c.process_name, c.pid))
                .collect::<Vec<_>>()
                .join("；");
            return Err(format!("端口冲突：{detail}"));
        }

        let mut cmd = build_project_command(&project)?;
        for e in &project.env {
            let key = e.key.trim();
            if !is_valid_env_key(key) {
                return Err(format!("环境变量名「{key}」非法（不能含空格或 = ）"));
            }
            cmd.env(key, e.value.as_str());
        }
        cmd.spawn().map_err(|e| format!("启动失败: {e}"))
    })();

    let mut child = match prepared {
        Ok(c) => c,
        Err(e) => {
            lockx(&state.tasks).remove(&task_id);
            return Err(e);
        }
    };
    let pid = child.id();

    // 日志桶回收：日志只对"还能读到的任务"有意义，避免长期运行后无限堆积
    {
        let running: HashSet<String> = lockx(&state.tasks).keys().cloned().collect();
        let mut logs = lockx(&state.logs);
        if logs.len() > TASK_LOG_BUCKETS {
            logs.retain(|k, _| running.contains(k));
        }
    }

    // 头一行记录启动命令，AI 排查"为什么起不来"时有据可依
    {
        let mut logs = lockx(&state.logs);
        logs.insert(
            task_id.clone(),
            VecDeque::from(vec![format!("[sys] 启动命令: {}", describe_project_cmd(&project))]),
        );
    }

    // stdout / stderr 读取线程
    for stream in ["stdout", "stderr"] {
        let pipe: Option<Box<dyn std::io::Read + Send>> = if stream == "stdout" {
            child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>)
        } else {
            child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>)
        };
        if let Some(pipe) = pipe {
            let app = app.clone();
            let task_id = task_id.clone();
            let stream = stream.to_string();
            thread::spawn(move || {
                let mut reader = BufReader::new(pipe);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match reader.read_until(b'\n', &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let text = platform::decode_output(&buf);
                            if !text.is_empty() {
                                let _ = app.emit(
                                    "task-log",
                                    serde_json::json!({
                                        "taskId": task_id,
                                        "stream": stream,
                                        "text": text,
                                        "ts": now_sec(),
                                    }),
                                );
                                // 同步进环形缓冲：MCP 侧靠它读日志，界面重载后也能补看
                                if let Some(state) = app.try_state::<AppState>() {
                                    let mut logs = lockx(&state.logs);
                                    let bucket = logs.entry(task_id.clone()).or_default();
                                    bucket.push_back(format!("[{stream}] {}", text.trim_end()));
                                    while bucket.len() > TASK_LOG_CAP {
                                        bucket.pop_front();
                                    }
                                }
                            }
                        }
                    }
                }
            });
        }
    }

    // 退出监听线程
    {
        let app = app.clone();
        let task_id = task_id.clone();
        thread::spawn(move || {
            let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
            if let Some(state) = app.try_state::<AppState>() {
                lockx(&state.tasks).remove(&task_id);
            }
            let _ = app.emit(
                "task-status",
                serde_json::json!({
                    "taskId": task_id,
                    "status": "exited",
                    "code": code,
                    "ts": now_sec(),
                }),
            );
        });
    }

    let started_at = now_sec();
    {
        let mut tasks = lockx(&state.tasks);
        match tasks.get_mut(&task_id) {
            Some(t) => {
                t.pid = pid;
                t.started_at = started_at;
            }
            // 占位已不在（极短命的进程，等待线程已经回收了它）：这不是错误，
            // 任务本来就该消失，如实返回启动信息即可，前端会随下一次刷新校正
            None => {}
        }
    }

    Ok(TaskInfo {
        id: task_id,
        project,
        pid,
        started_at,
    })
}

#[tauri::command]
async fn start_task(app: AppHandle, project: Project) -> Result<TaskInfo, String> {
    // 保存路径上已校验过，这里再校验一次：MCP 与控制口是另一条入口，
    // 且配置可能被手工改过。启动前的最后一道拦截比"起了再报错"便宜得多。
    validate_projects(std::slice::from_ref(&project))?;
    let task_id = next_task_id();
    spawn_task(app, task_id, project)
}

#[tauri::command]
async fn stop_task(app: AppHandle, task_id: String) -> Result<bool, String> {
    let state = app.state::<AppState>();
    let pid = {
        let tasks = lockx(&state.tasks);
        tasks
            .get(&task_id)
            .map(|t| t.pid)
            .ok_or_else(|| "任务未在运行".to_string())?
    };
    if pid == 0 {
        // 占位态：进程还没创建出来，此时 kill 0 的语义完全不对
        return Err("该任务正在启动中，请稍候再试".into());
    }

    // 这个 pid 是本应用自己拉起来的任务进程，归属明确（`tasks` 里记着它的项目），
    // 不属于"裸结束"要防的误杀，所以用 Project 上下文：受保护端口不再拦。
    // 否则会出现"工具在 8000 上启动了项目、却再也停不掉"这种自相矛盾的状态。
    let killed = kill_pid_scoped(&app, pid, KillScope::Project)?;
    let (ok, msg) = (killed.ok, killed.message);

    // 只在**确认进程已不在**时摘除任务：
    //  - 进程早就自己退出了（taskkill 会报错，但目的已达成）→ 必须摘除，
    //    否则任务永远卡在"运行中"，此后每次停止都必然失败，成一个死状态。
    //  - 进程还在、只是杀不掉（多为权限不足）→ **保留**记录，让界面如实显示
    //    它还在跑。若此处一并摘除，界面会显示成"任务消失了"，用户会以为
    //    服务已停，实际端口仍被占着——下次启动就撞端口。
    if ok || !process_exists(pid) {
        lockx(&state.tasks).remove(&task_id);
        return Ok(true);
    }

    Err(if msg.is_empty() {
        "停止失败，可能需要管理员权限".to_string()
    } else {
        msg
    })
}

/// 接管项目：结束占用其声明端口的外部进程，等端口释放，再按项目配置重新启动。
///
/// 界面（项目页 / 首页的「接管」按钮）与控制口的 `takeover_project` 都汇到这里 ——
/// 此前界面是在 store.js 里自己"逐个 kill_pid + 等 800ms + startProject"，
/// 那条路径绕过了项目语义：受保护端口上的 kill 会被拒、且被 `catch {}` 静默吞掉，
/// 用户最后只看到一句莫名其妙的"端口冲突"。逻辑收拢到后端，保护判断才有统一落点。
#[tauri::command]
async fn takeover_task(app: AppHandle, project: Project) -> Result<TakeoverResult, String> {
    takeover_project(app, project).await
}

/// 接管的公共实现（界面与控制口共用，签名与 `start_task` 对齐便于替换调用）
pub(crate) async fn takeover_project(
    app: AppHandle,
    project: Project,
) -> Result<TakeoverResult, String> {
    if !Path::new(&project.cwd).is_dir() {
        return Err(format!("工作目录不存在: {}", project.cwd));
    }

    let scanned = scan_ports(app.clone(), vec![project.clone()]).await?;
    let rt = scanned.runtime.into_iter().next();

    let mut killed_pids: Vec<u32> = Vec::new();
    if let Some(rt) = &rt {
        if !rt.task_id.is_empty() {
            return Err(format!(
                "项目「{}」已由本应用托管运行，无需接管",
                project.name
            ));
        }
        // rt.pids 只包含"监听本项目声明端口"的进程，杀掉它们不会波及无关服务
        for pid in &rt.pids {
            if kill_pid_scoped(&app, *pid, KillScope::Project)?.ok {
                killed_pids.push(*pid);
            }
        }
    }

    if !killed_pids.is_empty() {
        // 等端口真正释放，避免刚结束就启动又撞上残留占用
        thread::sleep(Duration::from_millis(TAKEOVER_RELEASE_WAIT_MS));
    }

    let task = start_task(app, project).await?;
    Ok(TakeoverResult { killed_pids, task })
}

/// 接管后等端口释放的时长。taskkill 返回不代表监听立刻消失（内核要回收），
/// 800ms 是实测够用且不至于让用户干等的折中值。
const TAKEOVER_RELEASE_WAIT_MS: u64 = 800;

/// 进程是否仍存在。仅在 kill 失败这条冷路径上调用，所以用全量枚举也无所谓。
fn process_exists(pid: u32) -> bool {
    System::new_all()
        .process(sysinfo::Pid::from_u32(pid))
        .is_some()
}

#[tauri::command]
fn list_tasks(app: AppHandle) -> Vec<TaskInfo> {
    let state = app.state::<AppState>();
    let tasks = lockx(&state.tasks);
    tasks
        .iter()
        .map(|(id, t)| TaskInfo {
            id: id.clone(),
            project: t.project.clone(),
            pid: t.pid,
            started_at: t.started_at,
        })
        .collect()
}

/// 一边跑、一边带超时收输出。
///
/// 不能用 `Command::output()`：它是"先等进程结束、再读管道"的同步模型，
/// 一条 `ping -t` 就能让处理该请求的线程永久阻塞，界面表现为整个页面卡死。
/// 这里改用线程分别啃 stdout/stderr（两管道必须并发读，否则子进程写满 stderr
/// 缓冲区而我们在等 stdout，双方互锁），主线程只负责轮询退出与计时。
fn run_command_with_timeout(
    mut cmd: Command,
    timeout: Duration,
) -> Result<ExecResult, String> {
    let start = Instant::now();
    let mut child = cmd.spawn().map_err(|e| format!("启动失败: {e}"))?;
    let pid = child.id();

    let stdout_buf = Arc::new(Mutex::new(String::new()));
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    let stdout_capped = Arc::new(AtomicBool::new(false));
    let stderr_capped = Arc::new(AtomicBool::new(false));
    let out_handle = child.stdout.take().map(|p| {
        let buf = stdout_buf.clone();
        let capped = stdout_capped.clone();
        thread::spawn(move || pump_output(p, buf, capped))
    });
    let err_handle = child.stderr.take().map(|p| {
        let buf = stderr_buf.clone();
        let capped = stderr_capped.clone();
        thread::spawn(move || pump_output(p, buf, capped))
    });

    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(-1),
            Ok(None) => {}
            Err(e) => return Err(format!("等待进程失败: {e}")),
        }
        if start.elapsed() >= timeout {
            timed_out = true;
            // 整棵进程树都要收掉：只杀 cmd.exe 会留下它启动的真正工作进程
            let _ = platform::kill_process(pid);
            let _ = child.wait();
            break -1;
        }
        thread::sleep(EXEC_POLL);
    };

    join_bounded(out_handle, EXEC_READER_GRACE);
    join_bounded(err_handle, EXEC_READER_GRACE);

    let stdout = take_buf(&stdout_buf);
    let stderr = take_buf(&stderr_buf);
    let duration_ms = start.elapsed().as_millis() as u64;
    // 截断判定要在两个流都收尾之后做：任一被削过，就要如实告诉用户
    let truncated = stdout_capped.load(Ordering::Relaxed)
        || stderr_capped.load(Ordering::Relaxed)
        || stdout.chars().count() >= EXEC_OUTPUT_CAP
        || stderr.chars().count() >= EXEC_OUTPUT_CAP;

    // 超时不当成 Err 抛：Err 在界面上只会变成一条 toast，两千字的现场输出
    // 塞进 toast 等于丢掉。正常返回 + timed_out 标记，输出照样进日志面板。
    Ok(ExecResult {
        stdout,
        stderr,
        code,
        duration_ms,
        timed_out,
        truncated,
    })
}

/// 读干净一个流并写进共享缓冲；按行 decode 以复用 GBK 兼容的解码逻辑。
/// `capped` 在丢弃数据时置位，让调用方能告诉用户"日志被截断了"而不是装作输出就这么长。
fn pump_output(pipe: impl std::io::Read, buf: Arc<Mutex<String>>, capped: Arc<AtomicBool>) {
    let mut reader = BufReader::new(pipe);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let text = platform::decode_output(&raw);
                if text.is_empty() && raw.is_empty() {
                    continue;
                }
                let mut guard = lockx(&buf);
                if guard.len() >= EXEC_OUTPUT_CAP {
                    capped.store(true, Ordering::Relaxed);
                    continue;
                }
                guard.push_str(&text);
                guard.push('\n');
            }
        }
    }
}

fn take_buf(buf: &Arc<Mutex<String>>) -> String {
    let s = lockx(buf).clone();
    let s = s.trim_end().to_string();
    if s.chars().count() > EXEC_OUTPUT_CAP {
        s.chars().take(EXEC_OUTPUT_CAP).collect()
    } else {
        s
    }
}

/// 有限等待读取线程收尾。管道理论上会随进程树结束而关闭，
/// 但"理论上"不值得赌——读不到就放弃，线程自行退出，不阻塞调用方。
fn join_bounded(handle: Option<thread::JoinHandle<()>>, wait: Duration) {
    let Some(h) = handle else { return };
    let deadline = Instant::now() + wait;
    while !h.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if h.is_finished() {
        let _ = h.join();
    }
}

#[tauri::command]
fn exec_command(app: AppHandle, cmd: String, cwd: String) -> Result<ExecResult, String> {
    let workdir = if cwd.trim().is_empty() { ".".to_string() } else { cwd };
    if !Path::new(&workdir).is_dir() {
        return Err(format!("工作目录不存在: {workdir}"));
    }

    let c = platform::spawn_shell_command(&cmd, &workdir)?;
    let result = run_command_with_timeout(c, EXEC_TIMEOUT);

    // 执行历史无论成败都记：失败与超时才是最需要回头看的那部分
    let state = app.state::<AppState>();
    let (code, duration_ms) = match &result {
        Ok(r) => (r.code, r.duration_ms),
        Err(_) => (-1, 0),
    };
    let _ = with_config(&state, |cfg| {
        cfg.history.insert(
            0,
            HistoryItem {
                ts: now_sec(),
                command: cmd.clone(),
                cwd: workdir.clone(),
                code,
                duration_ms,
            },
        );
        cfg.history.truncate(50);
    });

    result
}

#[tauri::command]
fn open_in_explorer(path: String) -> Result<bool, String> {
    if !Path::new(&path).is_dir() {
        return Err(format!("目录不存在: {path}"));
    }
    platform::open_in_explorer(&path)
}

// ---------------- 网络工具箱命令（源自 network-toolbox） ----------------

#[tauri::command]
async fn net_ping(host: String, count: Option<u32>, size: Option<u32>) -> Result<network::PingResult, String> {
    network::ping(&host, count.unwrap_or(4), size.unwrap_or(32))
}

#[tauri::command]
async fn net_traceroute(host: String) -> Result<network::TracerouteResult, String> {
    network::traceroute(&host)
}

#[tauri::command]
async fn net_port_scan(host: String, ports: String) -> Result<network::PortScanResult, String> {
    network::port_scan(&host, &ports)
}

#[tauri::command]
async fn net_arp_hosts() -> Result<serde_json::Value, String> {
    let hosts = network::arp_hosts()?;
    Ok(serde_json::json!({ "hosts": hosts, "total": hosts.len() }))
}

#[tauri::command]
async fn net_route_table() -> Result<serde_json::Value, String> {
    let routes = network::route_table()?;
    Ok(serde_json::json!({ "routes": routes, "count": routes.len() }))
}

#[tauri::command]
async fn net_wol(mac: String) -> Result<String, String> {
    network::wake_on_lan(&mac)
}

#[tauri::command]
async fn net_speedtest(source: Option<String>) -> Result<network::SpeedResult, String> {
    // 默认走中科大镜像：实测 Cloudflare 在国内网络下多半不可达，
    // 默认值不该把用户直接推到失败路径上
    network::speedtest(&source.unwrap_or_else(|| "ustc".into()))
}

#[tauri::command]
async fn net_dns_lookup(host: String, rtype: Option<String>) -> Result<network::DnsResult, String> {
    network::dns_lookup(&host, rtype.as_deref().unwrap_or("A"))
}

#[tauri::command]
async fn net_local_info() -> Result<network::LocalNetInfo, String> {
    network::local_info()
}

/// 本模块唯一的外发请求：查询公网出口 IP 与归属地（myip.ipip.net）
#[tauri::command]
async fn net_public_ip() -> Result<network::PublicIp, String> {
    network::public_ip()
}

/// 把诊断结果导出为 txt 到用户「下载」目录，返回完整路径。
/// 走后端是因为 WebView 里的 `<a download>` 不会触发保存对话框。
#[tauri::command]
fn net_export_report(name: String, content: String) -> Result<String, String> {
    network::export_report(&name, &content)
}

// ---------------- MCP 控制口状态 ----------------

/// 供界面展示「AI 接入」面板：控制口是否开启、端口、令牌、可执行文件位置、近期 AI 操作
/// 与本 exe 同目录的 MCP 代理可执行文件路径；找不到返回空串。
///
/// 由 `current_exe()` 推导而不是记在配置里：`cargo build --release` 会为 `src/bin/`
/// 下的每个文件各产出一个 exe，两者永远挨着；而"把路径记进配置"一旦换了构建目录就
/// 静默失效（客户端检测专门在防这件事）。统一转成正斜杠，写进各家客户端配置时
/// 不必再操心转义，也与既有配置的写法一致。
pub fn mcp_exe_path() -> String {
    let name = if cfg!(windows) {
        "devtoolkit-mcp.exe"
    } else {
        "devtoolkit-mcp"
    };
    std::env::current_exe()
        .ok()
        .map(|p| p.with_file_name(name))
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

#[tauri::command]
fn mcp_info(app: AppHandle) -> serde_json::Value {
    let state = app.state::<AppState>();
    let config_path = lockx(&state.config_path).clone();
    let cfg = read_config(&config_path);
    let mcp_exe = mcp_exe_path();

    let guard = lockx(&state.control);
    let (enabled, port, requests, errors, started_at) = match guard.as_ref() {
        Some(c) => (
            true,
            c.port,
            c.requests(),
            c.errors(),
            c.started_at,
        ),
        None => (false, 0, 0, 0, 0),
    };

    serde_json::json!({
        "enabled": enabled,
        "port": port,
        "requests": requests,
        "errors": errors,
        "started_at": started_at,
        "token": cfg.mcp_token,
        "config_path": config_path,
        "mcp_exe": mcp_exe,
        "mcp_exe_found": !mcp_exe.is_empty(),
        "audit_count": cfg.mcp_audit.len(),
        "recent_audit": cfg.mcp_audit.iter().take(8).collect::<Vec<_>>(),
        // 与上面的 port（实际监听）分开：端口被占用顺延后两者会不同，
        // 界面要能说清"你期望的"和"实际绑到的"分别是哪个
        "preferred_port": cfg.mcp_preferred_port,
    })
}

// ---------------- MCP 功能管理 ----------------

/// 逐个客户端检测 MCP 注册状态（文件在不在 / 条目在不在 / 条目对不对）
#[tauri::command]
fn mcp_clients_detect() -> serde_json::Value {
    let exe = mcp_exe_path();
    let clients: Vec<_> = mcp_clients::known_clients()
        .iter()
        .map(|s| mcp_clients::detect(s, &exe))
        .collect();
    serde_json::json!({ "mcp_exe": exe, "clients": clients })
}

/// 把本 MCP 写进指定客户端配置（合并，保留其它 server 条目）
#[tauri::command]
fn mcp_client_write(id: String) -> Result<serde_json::Value, String> {
    let exe = mcp_exe_path();
    let spec = mcp_clients::known_clients()
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("未知的客户端「{id}」"))?;
    let written = mcp_clients::write_entry(&spec, &exe)?;
    Ok(serde_json::json!({
        "id": spec.id,
        "name": spec.name,
        "path": written["path"],
        "preserved": written["preserved"],
    }))
}

/// 自检：另起 MCP 代理进程跑一次真实协议握手，把工具清单取回来。
/// 启动进程 + 协议往返都是阻塞操作，放阻塞线程池，别占住异步运行时。
#[tauri::command]
async fn mcp_selftest() -> Result<serde_json::Value, String> {
    let exe = mcp_exe_path();
    tauri::async_runtime::spawn_blocking(move || {
        mcp_clients::selftest(&exe, std::time::Duration::from_secs(15))
    })
    .await
    .map_err(|e| format!("自检任务执行失败: {e}"))
}

/// AI 操作审计。新的在前 —— 排查"刚才那次调用干了什么"比翻历史重要得多。
#[tauri::command]
fn mcp_audit_list(app: AppHandle, limit: Option<usize>) -> serde_json::Value {
    let state = app.state::<AppState>();
    let path = lockx(&state.config_path).clone();
    let cfg = read_config(&path);
    let n = limit.unwrap_or(500).min(5000);
    let items: Vec<&AuditItem> = cfg.mcp_audit.iter().rev().take(n).collect();
    serde_json::json!({ "total": cfg.mcp_audit.len(), "shown": items.len(), "items": items })
}

#[tauri::command]
fn mcp_audit_clear(app: AppHandle) -> Result<usize, String> {
    let state = app.state::<AppState>();
    with_config(&state, |c| {
        let n = c.mcp_audit.len();
        c.mcp_audit.clear();
        n
    })
}

/// 开/关控制口。关闭后 AI 侧全部失效，界面上的手工操作完全不受影响。
#[tauri::command]
fn mcp_control_set(app: AppHandle, enabled: bool) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();

    // 先取出句柄再释放锁，然后才做耗时的 stop —— 持锁阻塞会把界面轮询一起卡住
    let cur = {
        let mut g = lockx(&state.control);
        g.take()
    };
    if let Some(c) = &cur {
        control::stop(c)?;
    }

    if !enabled {
        return Ok(serde_json::json!({ "enabled": false, "port": 0 }));
    }

    let ctx = control::start(app.clone()).ok_or(
        "控制口启动失败：候选端口段均不可用。请到端口页确认是否被占用，或换一个起始端口",
    )?;
    let port = ctx.port;
    *lockx(&state.control) = Some(ctx);
    Ok(serde_json::json!({ "enabled": true, "port": port }))
}

/// 重置访问令牌 —— 怀疑令牌外泄时用。代价是当前存活的 MCP 进程会立刻失效。
#[tauri::command]
fn mcp_token_reset(app: AppHandle) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();
    let was_on = {
        let mut g = lockx(&state.control);
        let existed = g.is_some();
        if let Some(c) = g.take() {
            control::stop(&c)?;
        }
        existed
    };

    let token = control::rotate_token(&app)?;

    let port = if was_on {
        // 必须重启：Ctx 里的 token 是构造时拷进去的，不重启就还在拿旧值校验
        let ctx = control::start(app.clone()).ok_or("令牌已重置，但控制口重启失败")?;
        let p = ctx.port;
        *lockx(&state.control) = Some(ctx);
        p
    } else {
        0
    };

    Ok(serde_json::json!({
        "token_len": token.len(),
        "enabled": was_on,
        "port": port,
        // 说清楚"不用改客户端配置"，否则用户会以为要把新令牌抄到各处
        "note": "令牌已更换。MCP 进程每次启动都从配置文件读取最新令牌，客户端配置里只存 exe 路径，无需改动。",
    }))
}

/// 改控制口起始端口。返回实际端口，并明确标出是否发生顺延。
#[tauri::command]
fn mcp_port_set(app: AppHandle, port: u16) -> Result<serde_json::Value, String> {
    if port != 0 && port < 1024 {
        return Err(format!("端口 {port} 属于系统保留段（1-1023），请选 1024 及以上"));
    }
    if port != 0 && protected_port_list(&app).contains(&port) {
        return Err(format!(
            "端口 {port} 在受保护名单里 —— 保护名单的意义正是任何自动化都不许占用它"
        ));
    }

    let state = app.state::<AppState>();
    let cur = {
        let mut g = lockx(&state.control);
        g.take()
    };
    if let Some(c) = &cur {
        control::stop(c)?;
    }

    with_config(&state, |c| c.mcp_preferred_port = port)?;

    let ctx = control::start(app.clone())
        .ok_or("控制口重启失败：该端口段均不可用，请换一个端口")?;
    let actual = ctx.port;
    *lockx(&state.control) = Some(ctx);

    Ok(serde_json::json!({
        "requested": port,
        "port": actual,
        // 顺延必须显式说出来：否则用户看到"我设了 9600 它却显示 9601"会当成 bug
        "shifted": port != 0 && actual != port,
    }))
}

// ---------------- main ----------------

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            tasks: Mutex::new(HashMap::new()),
            config_path: Mutex::new(String::new()),
            control: Mutex::new(None),
            logs: Mutex::new(HashMap::new()),
            cfg_lock: Mutex::new(()),
            ports_cache: Mutex::new(Vec::new()),
            ports_cache_at: Mutex::new(None),
        })
        .setup(|app| {
            let handle = app.handle().clone();

            let dir = handle.path().app_config_dir().expect("无法定位配置目录");
            fs::create_dir_all(&dir).ok();
            let path = dir.join("devtoolkit.json").to_string_lossy().to_string();
            *lockx(&app.state::<AppState>().config_path) = path;

            // MCP 控制口：供 devtoolkit-mcp 进程（AI 侧）调用，失败不影响 GUI
            let ctx = control::start(handle.clone());
            *lockx(&app.state::<AppState>().control) = ctx;

            // 系统指标推送线程（只刷新 CPU/内存，避免全量进程扫描拖垮 CPU）
            thread::spawn(move || {
                let mut sys = System::new_all();
                loop {
                    thread::sleep(Duration::from_secs(3));
                    sys.refresh_cpu();
                    sys.refresh_memory();
                    let cpus = sys.cpus();
                    let cpu = if cpus.is_empty() {
                        0.0
                    } else {
                        cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32
                    };
                    let _ = handle.emit(
                        "sys-metrics",
                        Metrics {
                            cpu,
                            mem_total: sys.total_memory(),
                            mem_used: sys.used_memory(),
                            uptime: System::uptime(),
                            ts: now_sec(),
                        },
                    );
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                // 退出前结束所有托管任务，避免孤儿进程。
                // 但受保护端口上的任务要留下：用户的项目应当活过 DevToolkit 的开关，
                // 关个工具顺手把 8000 上的后端带走，正是"影响本地项目"。
                // 留下的进程下次启动会被按端口/目录识别成"外部启动"，不会变成幽灵任务。
                if let Some(state) = window.app_handle().try_state::<AppState>() {
                    let handle = window.app_handle().clone();
                    let protected = protected_port_list(&handle);
                    let tasks: Vec<(u32, Project)> = lockx(&state.tasks)
                        .values()
                        .filter(|t| t.pid != 0 && !PROTECTED_PIDS.contains(&t.pid))
                        .map(|t| (t.pid, t.project.clone()))
                        .collect();
                    // 端口表读不到时一律不杀：这段代码的唯一目的是"别带走用户的服务"，
                    // 不确定的时候宁可留下孤儿任务（下次启动会被识别成"外部启动"，可接管），
                    // 也不要赌一次"应该没事"
                    let ports = cached_ports(&state).unwrap_or_default();
                    let keep = keep_alive_tasks(&tasks, &ports, &protected);
                    for (pid, _) in &tasks {
                        if keep.contains(pid) {
                            continue;
                        }
                        let _ = platform::kill_process(*pid);
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            sys_info,
            list_ports,
            check_ports,
            scan_ports,
            list_processes,
            process_detail,
            kill_pid,
            get_config,
            save_projects,
            set_port_protected,
            check_project,
            start_task,
            stop_task,
            takeover_task,
            list_tasks,
            exec_command,
            open_in_explorer,
            net_ping,
            net_traceroute,
            net_port_scan,
            net_arp_hosts,
            net_route_table,
            net_wol,
            net_speedtest,
            net_dns_lookup,
            net_local_info,
            net_public_ip,
            net_export_report,
            mcp_info,
            mcp_clients_detect,
            mcp_client_write,
            mcp_selftest,
            mcp_audit_list,
            mcp_audit_clear,
            mcp_control_set,
            mcp_token_reset,
            mcp_port_set
        ])
        .run(tauri::generate_context!())
        .expect("error while running DevToolkit");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v1 旧配置必须能正常解析：新增字段全部带 serde default，不影响已托管项目
    #[test]
    fn legacy_config_still_parses() {
        let raw = r##"{
            "projects": [{
                "id": "p1", "name": "旧项目", "command": "npm run dev",
                "cwd": "E:\\demo", "color": "#4f8cff", "note": ""
            }],
            "history": [], "kills": []
        }"##;
        let cfg: Config = serde_json::from_str(raw).expect("旧配置应能解析");
        let p = &cfg.projects[0];
        assert_eq!(p.command, "npm run dev");
        assert_eq!(p.kind, "command", "旧项目应默认为命令行模式");
        assert!(p.env.is_empty());
        assert_eq!(p.ports, "");
        assert_eq!(p.script_path, "");
    }

    /// v2 脚本项目配置解析
    #[test]
    fn script_project_parses() {
        let raw = r##"{
            "projects": [{
                "id": "p2", "name": "构建脚本", "kind": "script",
                "script_path": "E:\\scripts\\build.ps1", "interpreter": "powershell",
                "args": "-Clean", "cwd": "E:\\demo", "color": "#22d3ee",
                "ports": "3000,8080",
                "env": [{"key": "NODE_ENV", "value": "development"}]
            }],
            "history": [], "kills": []
        }"##;
        let cfg: Config = serde_json::from_str(raw).expect("v2 配置应能解析");
        let p = &cfg.projects[0];
        assert_eq!(p.kind, "script");
        assert_eq!(p.script_path, "E:\\scripts\\build.ps1");
        assert_eq!(p.interpreter, "powershell");
        assert_eq!(p.args, "-Clean");
        assert_eq!(p.env.len(), 1);
        assert_eq!(p.env[0].key, "NODE_ENV");
    }

    /// 无有效端口声明时不应触发系统调用，直接返回空
    #[test]
    fn empty_port_spec_returns_no_conflict() {
        assert!(find_port_conflicts("").unwrap().is_empty());
        assert!(find_port_conflicts("   ").unwrap().is_empty());
        assert!(find_port_conflicts("abc,xyz").unwrap().is_empty());
    }

    fn mk_project(id: &str, name: &str, ports: &str) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            command: "npm run dev".into(),
            cwd: "D:\\WWW\\demo".into(),
            color: "#4f8cff".into(),
            note: String::new(),
            kind: "command".into(),
            script_path: String::new(),
            interpreter: String::new(),
            args: String::new(),
            env: vec![],
            group: String::new(),
            ports: ports.into(),
        }
    }

    fn mk_port(proto: &str, port: u16, state: &str, pid: u32, name: &str) -> PortInfo {
        PortInfo {
            proto: proto.into(),
            local_addr: "0.0.0.0".into(),
            local_port: port,
            remote_addr: "0.0.0.0:0".into(),
            state: state.into(),
            pid,
            process_name: name.into(),
        }
    }

    /// 声明端口正在监听 → 该项目判为运行中（即使不是本应用启动的）
    #[test]
    fn runtime_matches_declared_ports() {
        let projects = vec![
            mk_project("p1", "前端", "9528"),
            mk_project("p2", "后端", "8000, 9999"),
            mk_project("p3", "无声明", ""),
        ];
        let ports = vec![
            mk_port("TCP", 9528, "LISTENING", 111, "node.exe"),
            mk_port("TCP", 8000, "LISTENING", 222, "node.exe"),
            mk_port("TCP", 9000, "LISTENING", 333, "other.exe"), // 未声明，不应误挂
            mk_port("TCP", 6000, "ESTABLISHED", 444, "x.exe"),   // 非监听，不计入
        ];
        let rt = runtime_by_ports(&projects, &ports);

        assert!(rt[0].running);
        assert_eq!(rt[0].ports, vec![9528]);
        assert_eq!(rt[0].pids, vec![111]);
        assert_eq!(rt[0].source, SRC_PORT);
        assert_eq!(rt[0].process_name, "node.exe");

        // 两个声明端口中只有一个在监听 → 只报监听的
        assert!(rt[1].running);
        assert_eq!(rt[1].ports, vec![8000]);

        // 未声明端口的项目不应仅凭端口判定，交给目录匹配
        assert!(!rt[2].running);
        assert!(rt[2].ports.is_empty());
    }

    /// 端口声明解析与目录匹配边界
    #[test]
    fn port_spec_and_dir_match() {
        assert_eq!(parse_port_spec("3000,8080 9000;3000"), vec![3000, 8080, 9000]);
        assert!(parse_port_spec("abc,xyz").is_empty());
        assert!(parse_port_spec("").is_empty());

        assert!(dir_hit("d:\\www\\app", "D:\\WWW\\APP"));
        assert!(dir_hit("d:\\www\\app", "D:\\WWW\\APP\\src"));
        assert!(dir_hit("d:\\www\\app", "\"D:\\WWW\\app\""));
        assert!(dir_hit("d:\\www\\app", "D:\\WWW\\APP\\src\\index.js /c npm run dev"));
        assert!(!dir_hit("d:\\www\\app", "D:\\WWW\\app2"));
        assert!(!dir_hit("d:\\www\\app", ""));
    }

    // ---------------- 端口解析：宽松路径 ----------------

    /// 宽松解析必须"什么脏输入都吃得下、绝不 panic、绝不把好的丢掉"——
    /// 它跑在每次扫描里，配置被手工改坏也不该让扫描整体失败
    #[test]
    fn lenient_port_spec_never_panics_on_dirty_input() {
        assert!(parse_port_spec("").is_empty());
        assert!(parse_port_spec(",,,;;;   ").is_empty());
        assert!(parse_port_spec("-").is_empty());
        assert!(parse_port_spec("-1").is_empty(), "负数不是端口");
        assert!(parse_port_spec("0").is_empty(), "0 不是可监听端口");
        assert!(parse_port_spec("65536").is_empty(), "超过 u16 上限");
        assert!(parse_port_spec("99999999999999999999").is_empty());
        assert!(parse_port_spec("端口").is_empty());
        assert!(parse_port_spec(&"9".repeat(10_000)).is_empty());
        // 脏项不该污染合法项
        assert_eq!(parse_port_spec("3000, oops, 8080"), vec![3000, 8080]);
        assert_eq!(parse_port_spec("3000 # 后端的端口"), vec![3000]);
    }

    /// 区间写法必须被识别：以前 `3000-3010` 会被整串丢弃，
    /// 用户以为配了区间，实际一个端口都没生效且没有任何提示
    #[test]
    fn port_spec_supports_ranges() {
        assert_eq!(parse_port_spec("3000-3003"), vec![3000, 3001, 3002, 3003]);
        assert_eq!(parse_port_spec("8080,9000-9001"), vec![8080, 9000, 9001]);
        // 区间与单点重叠时去重
        assert_eq!(parse_port_spec("3000-3002,3001"), vec![3000, 3001, 3002]);
        // 起止颠倒 / 跨度过大：宽松路径忽略，严格路径报错
        assert!(parse_port_spec("3010-3000").is_empty());
        assert!(parse_port_spec("1-65535").is_empty());
    }

    #[test]
    fn validate_port_spec_accepts_valid_and_reports_reason() {
        assert_eq!(validate_port_spec("").unwrap(), Vec::<u16>::new());
        assert_eq!(validate_port_spec("8080").unwrap(), vec![8080]);
        assert_eq!(validate_port_spec(" 8000 , 9528 ").unwrap(), vec![8000, 9528]);
        assert_eq!(validate_port_spec("3000-3002").unwrap(), vec![3000, 3001, 3002]);

        // 报错必须说清"是哪个端口、错在哪"，否则用户只能猜
        let e = validate_port_spec("8000,abc").unwrap_err();
        assert!(e.contains("abc"), "{e}");
        let e = validate_port_spec("3000-3010-3020").unwrap_err();
        assert!(e.contains("3000-3010-3020"), "{e}");
        let e = validate_port_spec("3010-3000").unwrap_err();
        assert!(e.contains("颠倒"), "{e}");
        let e = validate_port_spec("1-65535").unwrap_err();
        assert!(e.contains("过大"), "{e}");
        assert!(validate_port_spec("0").is_err());
        assert!(validate_port_spec("65536").is_err());
    }

    // ---------------- 项目配置校验 ----------------

    #[test]
    fn validate_projects_rejects_dirty_config() {
        // 空列表合法
        assert!(validate_projects(&[]).is_ok());

        // 合法项目：不能因为"检查太严"把正常配置也拦下来
        let ok = vec![mk_project("p1", "前端", "3000"), mk_project("p2", "后端", "8000-8001")];
        assert!(validate_projects(&ok).is_ok(), "正常配置不应被拒绝");

        // 缺 id
        let mut a = mk_project("", "无 id", "");
        a.id = "  ".into();
        assert!(validate_projects(&[a]).unwrap_err().contains("缺少 id"));

        // 重复 id：运行态按 id 归类，重复会让两个项目抢同一份状态
        let dup_id = vec![mk_project("same", "甲", ""), mk_project("same", "乙", "")];
        let e = validate_projects(&dup_id).unwrap_err();
        assert!(e.contains("重复"), "{e}");

        // 重名：MCP 按名字定位时无法消歧。错误信息必须点出是哪个名字重了，
        // 否则用户在"改了 A 却因为 B 存不上"的循环里出不来
        let dup_name = vec![mk_project("a", "同名", ""), mk_project("b", "同名", "")];
        let e = validate_projects(&dup_name).unwrap_err();
        assert!(e.contains("同名"), "{e}");
        assert!(e.contains("无法区分"), "{e}");

        // 缺名称
        let mut no_name = mk_project("a", "x", "");
        no_name.name = "   ".into();
        assert!(validate_projects(&[no_name]).unwrap_err().contains("缺少名称"));

        // 缺工作目录
        let mut no_cwd = mk_project("a", "有名字", "");
        no_cwd.cwd = String::new();
        assert!(validate_projects(&[no_cwd]).unwrap_err().contains("工作目录"));

        // 命令行模式缺命令
        let mut no_cmd = mk_project("a", "有名字", "");
        no_cmd.command = "  ".into();
        assert!(validate_projects(&[no_cmd]).unwrap_err().contains("启动命令"));

        // 脚本模式缺脚本路径
        let mut no_script = mk_project("a", "有名字", "");
        no_script.kind = "script".into();
        assert!(validate_projects(&[no_script]).unwrap_err().contains("脚本文件路径"));

        // 非法 kind
        let mut bad_kind = mk_project("a", "有名字", "");
        bad_kind.kind = "whatever".into();
        let e = validate_projects(&[bad_kind]).unwrap_err();
        assert!(e.contains("whatever"), "{e}");

        // 端口写错
        let bad_ports = vec![mk_project("a", "有名字", "8000,nope")];
        assert!(validate_projects(&bad_ports).unwrap_err().contains("nope"));

        // 环境变量名带空格或等号：cmd 的 set 语法会直接坏掉
        let mut bad_env = mk_project("a", "有名字", "");
        bad_env.env = vec![EnvVar {
            key: "BAD KEY".into(),
            value: "1".into(),
        }];
        assert!(validate_projects(&[bad_env]).unwrap_err().contains("环境变量"));

        // 空 key 的环境变量按"没填"处理，不该拦
        let mut empty_env = mk_project("a", "有名字", "");
        empty_env.env = vec![EnvVar {
            key: "   ".into(),
            value: "1".into(),
        }];
        assert!(validate_projects(&[empty_env]).is_ok());
    }

    #[test]
    fn validate_projects_accepts_legacy_v1_entry() {
        // v1 配置没有 kind/ports/env 字段，解析后由 serde default 补齐（kind=command），
        // 必须仍然能通过校验——否则老用户一升级就存不了配置
        let raw = r##"[{
            "id": "p1", "name": "旧项目", "command": "npm run dev",
            "cwd": "E:\\demo", "color": "#4f8cff", "note": ""
        }]"##;
        let list: Vec<Project> = serde_json::from_str(raw).unwrap();
        assert!(validate_projects(&list).is_ok());
    }

    #[test]
    fn env_key_validation() {
        assert!(is_valid_env_key("NODE_ENV"));
        assert!(is_valid_env_key("A1"));
        assert!(!is_valid_env_key(""));
        assert!(!is_valid_env_key("A B"));
        assert!(!is_valid_env_key("A=B"));
        assert!(!is_valid_env_key("A\"B"));
    }

    // ---------------- 项目体检 ----------------

    #[test]
    fn inspect_project_separates_errors_from_warnings() {
        // 工作目录不存在只警告（目录可能稍后才建），不阻断
        let mut p = mk_project("a", "体检", "");
        p.cwd = "Z:\\绝对不存在的目录\\nope".into();
        let r = inspect_project(&p);
        assert!(r.errors.is_empty(), "目录不存在不该阻断: {:?}", r.errors);
        assert!(r.warnings.iter().any(|w| w.contains("工作目录不存在")), "{:?}", r.warnings);

        // 解释器无法识别是硬错误：脚本根本起不来
        let mut s = mk_project("b", "脚本", "");
        s.kind = "script".into();
        s.script_path = "E:\\x\\unknown.qqq".into();
        let r = inspect_project(&s);
        assert!(!r.errors.is_empty(), "无法识别的脚本类型应报错");
    }

    #[test]
    fn command_in_path_finds_shell_and_rejects_ghost() {
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(command_in_path(shell), "{shell} 应在 PATH 中");
        assert!(!command_in_path("definitely-not-a-command-zzz"));
        assert!(!command_in_path(""));
    }

    // ---------------- 任务 id ----------------

    /// 批量启动时任务 id 必须唯一：撞号会让先启动的任务被覆盖，
    /// 那个进程从此停不掉也看不见
    #[test]
    fn task_ids_are_unique_under_burst() {
        let mut seen = HashSet::new();
        for _ in 0..20_000 {
            assert!(seen.insert(next_task_id()), "task id 重复");
        }
    }

    // ---------------- 配置读写 ----------------

    fn temp_config_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dtk-{tag}-{}", next_task_id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 配置是"托管了哪些项目"的唯一真相：写入必须原子，损坏必须能恢复
    #[test]
    fn config_write_is_atomic_and_recovers_from_backup() {
        let dir = temp_config_dir("cfg");
        let path = dir.join("devtoolkit.json").to_string_lossy().to_string();

        let cfg1 = Config {
            projects: vec![mk_project("p1", "一号", "")],
            ..Config::default()
        };
        write_config(&path, &cfg1).unwrap();
        assert_eq!(read_config(&path).projects.len(), 1);

        let cfg2 = Config {
            projects: vec![mk_project("p1", "一号", ""), mk_project("p2", "二号", "")],
            ..Config::default()
        };
        write_config(&path, &cfg2).unwrap();
        assert_eq!(read_config(&path).projects.len(), 2);
        assert!(
            Path::new(&config_bak_path(&path)).exists(),
            "成功保存后应留下可用备份"
        );
        assert!(
            !Path::new(&format!("{path}.tmp")).exists(),
            "临时文件不该残留"
        );

        // 模拟"写到一半被打断"：主文件只剩半截 JSON。
        // 此时绝不能表现为"项目全没了"——那会诱导用户重新录一遍，然后覆盖掉备份
        fs::write(&path, "{\"projects\":[{\"id\":\"p1\"").unwrap();
        let recovered = read_config(&path);
        assert_eq!(
            recovered.projects.len(),
            2,
            "主配置损坏时应从备份恢复，而不是回退成空配置"
        );

        // 主文件与备份都坏：退回空配置，但不能 panic
        fs::write(&path, "!!!").unwrap();
        fs::write(config_bak_path(&path), "!!!").unwrap();
        assert!(read_config(&path).projects.is_empty());

        // 文件根本不存在时也不 panic
        let missing = dir.join("nope.json").to_string_lossy().to_string();
        assert!(read_config(&missing).projects.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_roundtrip_keeps_mcp_fields() {
        let dir = temp_config_dir("cfg2");
        let path = dir.join("devtoolkit.json").to_string_lossy().to_string();
        let cfg = Config {
            projects: vec![],
            mcp_token: "abc123".into(),
            mcp_port: 9527,
            ..Config::default()
        };
        write_config(&path, &cfg).unwrap();
        let back = read_config(&path);
        assert_eq!(back.mcp_token, "abc123");
        assert_eq!(back.mcp_port, 9527);
        let _ = fs::remove_dir_all(&dir);
    }

    // ---------------- 端口保护 ----------------

    /// 默认保护的就是用户的常驻项目：后端 8000、前端 9528
    #[test]
    fn default_protected_ports_cover_user_projects() {
        let list = default_protected_ports();
        assert!(list.contains(&8000), "默认应保护后端 8000");
        assert!(list.contains(&9528), "默认应保护前端 9528");
    }

    /// 旧配置（没有 protected_ports 字段）必须回落到默认名单。
    /// 否则升级后用户的项目会在"没人配置过"的状态下静默失去保护。
    #[test]
    fn legacy_config_without_protected_ports_falls_back_to_default() {
        let cfg: Config = serde_json::from_str(r#"{"projects":[]}"#).unwrap();
        assert_eq!(cfg.protected_ports, default_protected_ports());
    }

    /// 落盘再读回，名单不能丢
    #[test]
    fn config_roundtrip_keeps_protected_ports() {
        let dir = temp_config_dir("cfg3");
        let path = dir.join("devtoolkit.json").to_string_lossy().to_string();
        let cfg = Config {
            protected_ports: vec![8000, 9528, 3004],
            ..Config::default()
        };
        write_config(&path, &cfg).unwrap();
        assert_eq!(read_config(&path).protected_ports, vec![8000, 9528, 3004]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 只有"该 PID 自己占的、且在名单里"的端口才算命中：
    /// 别人占着 8000 不该牵连我，我占着 9999 也不该被拦
    #[test]
    fn match_protected_only_flags_own_listed_ports() {
        let ports = vec![
            mk_port("TCP", 8000, "LISTENING", 222, "php.exe"),
            mk_port("TCP", 9528, "LISTENING", 111, "node.exe"),
            mk_port("TCP", 9999, "LISTENING", 111, "node.exe"),
            mk_port("TCP", 3004, "LISTENING", 333, "node.exe"),
        ];
        let protected = [8000u16, 9528];

        // 前端进程只命中自己的 9528，不因为"别人占着 8000"被牵连
        assert_eq!(match_protected(&ports, 111, &protected), vec![9528]);
        // 后端进程命中 8000
        assert_eq!(match_protected(&ports, 222, &protected), vec![8000]);
        // 不在名单里的端口不算命中
        assert!(match_protected(&ports, 333, &protected).is_empty());
        // 名单为空时一律放行
        assert!(match_protected(&ports, 111, &[]).is_empty());
    }

    /// 同一进程同时占着多个受保护端口时全部列出（否则拒绝提示里会漏端口），
    /// 且重复行（多网卡/多连接各一行）要去重
    #[test]
    fn match_protected_lists_all_ports_sorted_and_deduped() {
        let ports = vec![
            mk_port("TCP", 9528, "LISTENING", 111, "node.exe"),
            mk_port("TCP", 8000, "LISTENING", 111, "node.exe"),
            mk_port("TCP", 8000, "LISTENING", 111, "node.exe"),
        ];
        assert_eq!(match_protected(&ports, 111, &[8000, 9528]), vec![8000, 9528]);
    }

    // ---------------- 关闭应用时的任务保留 ----------------

    /// 核心回归：任务的 PID 是 `cmd /C` 启动器，它自己不监听任何端口。
    /// 只看"进程自己的端口"会漏判，而结束是连树杀的 —— 于是关掉工具就把
    /// 用户 8000 上的后端一起带走。判定必须能借"项目声明端口"认出它。
    #[test]
    fn close_keeps_task_whose_project_listens_on_protected_port() {
        let tasks = vec![(700u32, mk_project("be", "后端", "8000"))];
        let ports = vec![mk_port("TCP", 8000, "LISTENING", 5676, "php.exe")];
        assert_eq!(keep_alive_tasks(&tasks, &ports, &[8000, 9528]), vec![700]);
    }

    /// 不受保护的任务照旧结束。不能因为有了保护逻辑就一律不杀，否则关一次窗口留一堆孤儿
    #[test]
    fn close_still_kills_unprotected_task() {
        let tasks = vec![
            (700u32, mk_project("be", "后端", "8000")),
            (800u32, mk_project("tmp", "临时服务", "3004")),
        ];
        let ports = vec![
            mk_port("TCP", 8000, "LISTENING", 5676, "php.exe"),
            mk_port("TCP", 3004, "LISTENING", 9001, "node.exe"),
        ];
        assert_eq!(keep_alive_tasks(&tasks, &ports, &[8000, 9528]), vec![700]);
    }

    /// 任务进程自己就是监听者时同样保留，不依赖"PID 一定是启动器"这个假设
    #[test]
    fn close_keeps_task_that_itself_listens_on_protected_port() {
        let tasks = vec![(5676u32, mk_project("be", "后端", ""))];
        let ports = vec![mk_port("TCP", 8000, "LISTENING", 5676, "php.exe")];
        assert_eq!(keep_alive_tasks(&tasks, &ports, &[8000]), vec![5676]);
    }

    /// 保护名单为空 = 关闭即全部结束，新逻辑不能把"全杀"变成"全不杀"
    #[test]
    fn close_kills_everything_when_no_protected_ports() {
        let tasks = vec![(700u32, mk_project("be", "后端", "8000"))];
        let ports = vec![mk_port("TCP", 8000, "LISTENING", 5676, "php.exe")];
        assert!(keep_alive_tasks(&tasks, &ports, &[]).is_empty());
    }

    /// 声明了受保护端口但端口没人监听时不算命中：进程已经不在了，没有保留的必要
    #[test]
    fn close_does_not_keep_project_whose_port_is_silent() {
        let tasks = vec![(700u32, mk_project("be", "后端", "8000"))];
        assert!(keep_alive_tasks(&tasks, &[], &[8000]).is_empty());
    }

    // ---------------- 运行态判定 ----------------

    /// 多个项目声明同一个端口时，两个都应被判为运行中（端口是共享真相，不是独占）
    #[test]
    fn runtime_shares_port_between_projects() {
        let projects = vec![mk_project("a", "甲", "8080"), mk_project("b", "乙", "8080")];
        let ports = vec![mk_port("TCP", 8080, "LISTENING", 42, "node.exe")];
        let rt = runtime_by_ports(&projects, &ports);
        assert!(rt[0].running && rt[1].running);
        assert_eq!(rt[0].pids, vec![42]);
        assert_eq!(rt[1].pids, vec![42]);
    }

    /// 区间声明同样参与运行态判定，且 UDP 绑定也算占用
    #[test]
    fn runtime_handles_range_spec_and_udp() {
        let projects = vec![mk_project("a", "区间", "3000-3002"), mk_project("b", "UDP", "5353")];
        let ports = vec![
            mk_port("TCP", 3001, "LISTENING", 7, "node.exe"),
            mk_port("UDP", 5353, "BOUND", 8, "svc.exe"),
        ];
        let rt = runtime_by_ports(&projects, &ports);
        assert!(rt[0].running);
        assert_eq!(rt[0].ports, vec![3001]);
        assert!(rt[1].running, "UDP 绑定即占用");
    }

    /// LISTENING 之外的 TCP 状态（ESTABLISHED / TIME_WAIT）不代表服务在监听
    #[test]
    fn runtime_ignores_non_listening_tcp() {
        let projects = vec![mk_project("a", "甲", "9200")];
        let ports = vec![
            mk_port("TCP", 9200, "ESTABLISHED", 9, "node.exe"),
            mk_port("TCP", 9200, "TIME_WAIT", 0, "-"),
        ];
        assert!(!runtime_by_ports(&projects, &ports)[0].running);
    }

    /// 一个未声明端口的项目不该被别的项目的端口带跑
    #[test]
    fn runtime_does_not_leak_between_projects() {
        let projects = vec![mk_project("a", "有声明", "7000"), mk_project("b", "无声明", "")];
        let ports = vec![mk_port("TCP", 7000, "LISTENING", 5, "node.exe")];
        let rt = runtime_by_ports(&projects, &ports);
        assert!(rt[0].running);
        assert!(!rt[1].running);
        assert!(rt[1].pids.is_empty() && rt[1].source.is_empty());
    }

    /// 目录匹配的边界：尾部分隔符、盘符大小写、前缀相同但不同的目录
    #[test]
    fn dir_hit_boundaries() {
        // 项目目录带尾部分隔符时也要能命中
        assert!(dir_hit("d:\\www\\app", "D:\\www\\app\\"));
        // 子目录、深层子目录
        assert!(dir_hit("d:\\www\\app", "d:\\www\\app\\a\\b"));
        // 前缀相同但并非子目录 —— 这是最容易误判的一种
        assert!(!dir_hit("d:\\www\\app", "d:\\www\\application"));
        assert!(!dir_hit("d:\\www\\app", "d:\\www\\app2\\x"));
        // 盘符大小写不敏感，但不同盘就是不命中
        assert!(!dir_hit("d:\\www\\app", "e:\\www\\app"));
        // 空目录配置永远不命中，避免把一切都匹配上
        assert!(!dir_hit("", "d:\\www"));
    }

    /// 日志缓冲容量常量与上限之间的关系必须是自洽的
    #[test]
    fn log_caps_are_sane() {
        assert!(TASK_LOG_CAP > 0);
        assert!(TASK_LOG_BUCKETS > 1);
        assert!(MAX_PROJECTS > 0);
        assert!(MAX_PORT_RANGE_SPAN >= 2);
        // 端口缓存不能长到"用户点刷新也看不到变化"
        assert!(PORTS_CACHE_TTL <= Duration::from_secs(3));
    }

    // ---------------- 命令执行：超时 / 截断 ----------------

    /// 一个跑不完的命令，用来验证超时路径。两边平台都得真能拖住。
    fn timeout_probe_command() -> String {
        if cfg!(windows) {
            "echo BEFORE_TIMEOUT && ping -n 20 127.0.0.1 > nul".to_string()
        } else {
            "echo BEFORE_TIMEOUT && sleep 20".to_string()
        }
    }

    /// 超时必须是"正常返回 + timed_out 标记"，而不是 Err。
    /// Err 在界面上只会变成一条 toast，两千字的现场输出等于直接丢掉——
    /// 而"命令卡住了"恰恰是最需要看现场的时候。
    #[test]
    fn exec_timeout_is_flagged_not_an_error() {
        let c = platform::spawn_shell_command(&timeout_probe_command(), ".")
            .expect("应能构造 shell 命令");
        let r = run_command_with_timeout(c, Duration::from_millis(900))
            .expect("超时不应返回 Err");
        assert!(r.timed_out, "必须标记为超时，否则界面只会显示 exit -1");
        assert_eq!(r.code, -1, "超时强制结束后的退出码约定为 -1");
        assert!(
            r.stdout.contains("BEFORE_TIMEOUT"),
            "超时前已经产生的输出必须保留，否则用户看不到现场：{:?}",
            r.stdout
        );
        assert!(!r.truncated, "这么短的输出不该被判成截断");
    }

    /// 正常退出的命令不能误报超时（timed_out 恒真会让这个字段失去意义）
    #[test]
    fn exec_normal_exit_is_not_flagged_as_timeout() {
        let c = platform::spawn_shell_command("echo OK_NORMAL", ".").expect("应能构造 shell 命令");
        let r = run_command_with_timeout(c, Duration::from_secs(30)).expect("正常命令不该失败");
        assert!(!r.timed_out);
        assert_eq!(r.code, 0);
        assert!(r.stdout.contains("OK_NORMAL"), "{:?}", r.stdout);
        assert!(!r.truncated);
    }

    /// 关键路径全在内存缓冲里判超长，这里直接喂一个超限输入验证置位逻辑。
    /// 不置位 = 用户看到半截日志却以为输出就这么多，排查时会被误导。
    #[test]
    fn pump_output_flags_truncation_when_data_is_dropped() {
        let buf = Arc::new(Mutex::new(String::new()));
        let capped = Arc::new(AtomicBool::new(false));
        let feed = format!("{}\n", "x".repeat(1024)).repeat(EXEC_OUTPUT_CAP / 1024 + 32);
        pump_output(feed.as_bytes(), buf.clone(), capped.clone());
        assert!(capped.load(Ordering::Relaxed), "丢弃数据后必须置位");
        assert!(lockx(&buf).len() >= EXEC_OUTPUT_CAP);
    }

    /// 反过来：没超限就不许置位，否则界面永远挂着一个"已截断"的假警告
    #[test]
    fn pump_output_does_not_flag_short_output() {
        let buf = Arc::new(Mutex::new(String::new()));
        let capped = Arc::new(AtomicBool::new(false));
        pump_output(&b"hello\nworld\n"[..], buf.clone(), capped.clone());
        assert!(!capped.load(Ordering::Relaxed));
        assert_eq!(lockx(&buf).trim_end(), "hello\nworld");
    }

    /// take_buf 是最后一道闸：pump 之后仍可能因为单行过长而超限
    #[test]
    fn take_buf_caps_runaway_output() {
        let buf = Arc::new(Mutex::new("y".repeat(EXEC_OUTPUT_CAP + 500)));
        assert_eq!(take_buf(&buf).chars().count(), EXEC_OUTPUT_CAP);
    }
}
