//! Windows 平台实现

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use super::{PortInfo, ProcRow};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 解码进程输出：优先 UTF-8，失败则按 GBK 解码（中文 Windows 控制台常见）
pub fn decode_output(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.trim_end_matches(['\r', '\n']).to_string();
    }
    let (cow, _, _) = encoding_rs::GBK.decode(bytes);
    cow.trim_end_matches(['\r', '\n']).to_string()
}

fn pid_name_map() -> HashMap<u32, String> {
    let mut m = HashMap::new();
    if let Ok(out) = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        let text = decode_output(&out.stdout);
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.trim_matches('"').split("\",\"").collect();
            if fields.len() >= 2 {
                if let Ok(pid) = fields[1].parse::<u32>() {
                    m.insert(pid, fields[0].to_string());
                }
            }
        }
    }
    m
}

pub fn list_ports() -> Result<Vec<PortInfo>, String> {
    let out = Command::new("netstat")
        .args(["-ano"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("netstat 执行失败: {e}"))?;

    let text = decode_output(&out.stdout);
    let names = pid_name_map();
    let mut list = Vec::new();

    for line in text.lines() {
        let t = line.trim();
        let parts: Vec<&str> = t.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let proto = parts[0];
        if proto != "TCP" && proto != "UDP" {
            continue;
        }
        let (local, remote, state, pid) = if proto == "TCP" {
            if parts.len() < 5 {
                continue;
            }
            (parts[1], parts[2], parts[3], parts[4])
        } else {
            (parts[1], parts[2], "BOUND", parts[3])
        };

        let pid: u32 = match pid.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let (local_addr, local_port) = match local.rsplit_once(':') {
            Some((a, p)) => (a.to_string(), p.parse::<u16>().unwrap_or(0)),
            None => (local.to_string(), 0),
        };
        let remote_addr = match remote.rsplit_once(':') {
            Some((a, p)) => format!("{a}:{p}"),
            None => remote.to_string(),
        };

        list.push(PortInfo {
            proto: proto.to_string(),
            local_addr,
            local_port,
            remote_addr,
            state: state.to_string(),
            pid,
            process_name: names.get(&pid).cloned().unwrap_or_else(|| "-".into()),
        });
    }
    Ok(list)
}

pub fn list_processes() -> Result<Vec<ProcRow>, String> {
    let out = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("tasklist 执行失败: {e}"))?;

    let text = decode_output(&out.stdout);
    let mut list = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.trim_matches('"').split("\",\"").collect();
        if fields.len() < 5 {
            continue;
        }
        let pid: u32 = match fields[1].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let mem_kb: u64 = fields[4]
            .replace(',', "")
            .replace(" K", "")
            .trim()
            .parse()
            .unwrap_or(0);
        list.push(ProcRow {
            pid,
            name: fields[0].to_string(),
            mem_kb,
        });
    }
    list.sort_by(|a, b| b.mem_kb.cmp(&a.mem_kb));
    Ok(list)
}

pub fn process_detail(pid: u32) -> Result<String, String> {
    let filter = format!("ProcessId={pid}");
    let script = format!(
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; (Get-CimInstance Win32_Process -Filter '{}').CommandLine",
        filter
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("PowerShell 执行失败: {e}"))?;
    Ok(decode_output(&out.stdout))
}

/// 结束进程（含进程树）。返回 (是否成功, 输出信息)
pub fn kill_process(pid: u32) -> Result<(bool, String), String> {
    let out = Command::new("taskkill")
        .args(["/F", "/T", "/PID"])
        .arg(pid.to_string())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("taskkill 执行失败: {e}"))?;

    let ok = out.status.success();
    let message = if ok {
        decode_output(&out.stdout)
    } else {
        format!("{}{}", decode_output(&out.stdout), decode_output(&out.stderr))
            .trim()
            .to_string()
    };
    Ok((ok, message))
}

/// 构造一个用于执行用户命令的 shell 进程（cmd /C，无窗口）
pub fn spawn_shell_command(user_cmd: &str, cwd: &str) -> Result<Command, String> {
    let mut cmd = Command::new("cmd");
    cmd.raw_arg("/C");
    cmd.raw_arg(user_cmd);
    cmd.current_dir(cwd);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    Ok(cmd)
}

pub fn open_in_explorer(path: &str) -> Result<bool, String> {
    Command::new("explorer")
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| true)
        .map_err(|e| e.to_string())
}

// ---------------- 脚本执行 ----------------

/// 按脚本扩展名推断默认解释器（Windows：cmd / powershell）
pub fn default_interpreter(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "bat" | "cmd" => Some("cmd"),
        "ps1" => Some("powershell"),
        "sh" => Some("bash"),
        "py" => Some("python"),
        "js" | "mjs" | "cjs" => Some("node"),
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
            format!("无法识别脚本类型 .{ext}，请在「解释器」中手动指定（如 python / node / bash）")
        })
}

/// 构造脚本执行进程（无窗口，权限为当前用户，不做提权）
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
    let extra: Vec<&str> = args.split_whitespace().collect();

    let mut cmd = match interp.to_ascii_lowercase().as_str() {
        // cmd 的参数需整体拼接，路径带空格时以引号包裹
        "cmd" => {
            let mut c = Command::new("cmd");
            c.raw_arg("/C");
            let mut line = format!("\"{path}\"");
            if !extra.is_empty() {
                line.push(' ');
                line.push_str(args);
            }
            c.raw_arg(line);
            c
        }
        // PowerShell 需绕过执行策略，否则 .ps1 默认被拦截
        "powershell" | "pwsh" => {
            let exe = if interp.eq_ignore_ascii_case("pwsh") {
                "pwsh"
            } else {
                "powershell"
            };
            let mut c = Command::new(exe);
            c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(path);
            c.args(extra);
            c
        }
        // node / python / bash 等：解释器 + 脚本路径 + 参数
        _ => {
            let mut c = Command::new(&interp);
            c.arg(path);
            c.args(extra);
            c
        }
    };

    cmd.current_dir(cwd);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    Ok(cmd)
}
