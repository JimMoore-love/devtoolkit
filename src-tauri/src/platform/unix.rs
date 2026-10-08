//! macOS / Linux 平台实现

use std::path::Path;
use std::process::{Command, Stdio};

use super::{PortInfo, ProcRow};

/// 解码进程输出（Unix 环境默认 UTF-8）
pub fn decode_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

/// 解析 "127.0.0.1:3306" / "*:3306" / "[::1]:80" 形式的地址
fn split_addr(name: &str) -> (String, u16) {
    let name = name.trim();
    if let Some(rest) = name.strip_prefix('[') {
        if let Some((ip, port)) = rest.split_once(']') {
            let port = port.trim_start_matches(':').parse().unwrap_or(0);
            return (ip.to_string(), port);
        }
    }
    if let Some((addr, port)) = name.rsplit_once(':') {
        let port = port.parse().unwrap_or(0);
        return (addr.to_string(), port);
    }
    (name.to_string(), 0)
}

fn parse_lsof(text: &str, proto: &str, state: &str) -> Vec<PortInfo> {
    let mut list = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 2 {
            continue;
        }
        let process_name = f[0].to_string();
        let pid: u32 = f[1].parse().unwrap_or(0);
        // NAME 是含 ':' 的地址字段（可能有 "(LISTEN)" 后缀）
        let name = f.iter().find(|t| t.contains(':')).cloned();
        if let Some(name) = name {
            let (addr, port) = split_addr(name);
            list.push(PortInfo {
                proto: proto.to_string(),
                local_addr: addr,
                local_port: port,
                remote_addr: "*".to_string(),
                state: state.to_string(),
                pid,
                process_name,
            });
        }
    }
    list
}

pub fn list_ports() -> Result<Vec<PortInfo>, String> {
    let mut list = Vec::new();

    let tcp = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN"])
        .output()
        .map_err(|e| format!("lsof 执行失败: {e}"))?;
    list.extend(parse_lsof(&decode_output(&tcp.stdout), "TCP", "LISTEN"));

    let udp = Command::new("lsof")
        .args(["-nP", "-iUDP"])
        .output()
        .map_err(|e| format!("lsof 执行失败: {e}"))?;
    list.extend(parse_lsof(&decode_output(&udp.stdout), "UDP", "BOUND"));

    Ok(list)
}

pub fn list_processes() -> Result<Vec<ProcRow>, String> {
    let out = Command::new("ps")
        .args(["-axo", "pid=,comm=,rss="])
        .output()
        .map_err(|e| format!("ps 执行失败: {e}"))?;

    let text = decode_output(&out.stdout);
    let mut list = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 3 {
            continue;
        }
        let pid: u32 = f[0].parse().unwrap_or(0);
        let name = f[1].to_string();
        let mem_kb: u64 = f[2].parse().unwrap_or(0);
        list.push(ProcRow { pid, name, mem_kb });
    }
    list.sort_by(|a, b| b.mem_kb.cmp(&a.mem_kb));
    Ok(list)
}

pub fn process_detail(pid: u32) -> Result<String, String> {
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .map_err(|e| format!("ps 执行失败: {e}"))?;
    Ok(decode_output(&out.stdout))
}

/// 结束进程（先结束子进程，再强杀本进程）。返回 (是否成功, 输出信息)
pub fn kill_process(pid: u32) -> Result<(bool, String), String> {
    // 尽力结束子进程树（失败忽略）
    let _ = Command::new("pkill")
        .args(["-TERM", "-P", &pid.to_string()])
        .output();
    let out = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output()
        .map_err(|e| format!("kill 执行失败: {e}"))?;
    let ok = out.status.success();
    let message = decode_output(&out.stderr);
    Ok((ok, message))
}

/// 构造一个用于执行用户命令的 shell 进程（/bin/sh -c）
pub fn spawn_shell_command(user_cmd: &str, cwd: &str) -> Result<Command, String> {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c");
    cmd.arg(user_cmd);
    cmd.current_dir(cwd);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    Ok(cmd)
}

pub fn open_in_explorer(path: &str) -> Result<bool, String> {
    Command::new("open")
        .arg(path)
        .spawn()
        .map(|_| true)
        .map_err(|e| e.to_string())
}

// ---------------- 脚本执行 ----------------

/// 按脚本扩展名推断默认解释器（macOS / Linux）
pub fn default_interpreter(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "sh" | "bash" => Some("bash"),
        "zsh" => Some("zsh"),
        "ps1" => Some("pwsh"),
        "py" => Some("python3"),
        "js" | "mjs" | "cjs" => Some("node"),
        "rb" => Some("ruby"),
        _ => None,
    }
}

fn script_ext(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// 解析最终解释器：显式指定优先，否则按扩展名推断
pub fn resolve_interpreter(path: &str, explicit: &str) -> Result<String, String> {
    let explicit = explicit.trim();
    if !explicit.is_empty() {
        return Ok(explicit.to_string());
    }
    let ext = script_ext(path);
    default_interpreter(&ext)
        .map(|s| s.to_string())
        .ok_or_else(|| {
            format!("无法识别脚本类型 .{ext}，请在「解释器」中手动指定（如 python3 / node / bash）")
        })
}

/// 构造脚本执行进程（权限为当前用户，不做提权）
pub fn spawn_script_command(
    path: &str,
    interpreter: &str,
    args: &str,
    cwd: &str,
) -> Result<Command, String> {
    if !Path::new(path).is_file() {
        return Err(format!("脚本文件不存在: {path}"));
    }
    let interp = resolve_interpreter(path, interpreter)?;

    let mut cmd = Command::new(&interp);
    cmd.arg(path);
    cmd.args(args.split_whitespace());
    cmd.current_dir(cwd);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    Ok(cmd)
}
