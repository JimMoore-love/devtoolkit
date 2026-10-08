//! AI 客户端 MCP 配置的检测、写入，以及功能自检。
//!
//! 背景：MCP 这东西最折磨人的地方不是"配不上"，而是**配了却看不到工具时没人告诉你卡在哪**。
//! 文件没写？路径过期了（换过构建目录）？被 `disabled` 了？还是配置都对、只是客户端没重新加载？
//! 这四种情况在用户眼里长得一模一样。所以这里把每个客户端拆成**三个可独立判定的事实**：
//! 文件在不在 → 条目在不在 → 条目对不对（路径有效 + 没被禁用），界面上分开展示。
//!
//! 写入策略刻意保守：只有"MCP 配置独占一个文件、且结构就是一张 server 表"的客户端才自动写；
//! 结构复杂的（Claude Code 的 `~/.claude.json` 里按项目分了多份 mcpServers，还混着会话历史）
//! 一律只检测并给出手工命令 —— 自动合并别人的复合配置文件是数据事故的常见来源。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// 创建进程时不弹控制台窗口（否则每点一次自检就闪一个黑框）
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// MCP 客户端约定俗成的 server 表键名，各家基本都用它
const SERVERS_KEY: &str = "mcpServers";
/// 我们在客户端配置里使用的 server 名，同时也是 MCP 工具前缀的来源
const SERVER_ID: &str = "devtoolkit";

const PROTOCOL: &str = "2024-11-05";

/// 一个 AI 客户端的配置位置与写入能力
pub struct ClientSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub path: PathBuf,
    /// 是否允许自动写入，见模块头注释的保守策略
    pub writable: bool,
    /// 不允许自动写入时给用户的手工命令
    pub hint: String,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// 已知客户端清单。
///
/// 之所以要列一个表而不是只认一家：用户很可能同时开着 WorkBuddy 和 Claude Code，
/// 只在其中一个配好，在另一个里就会问出同一句"我怎么看不到"。
pub fn known_clients() -> Vec<ClientSpec> {
    let Some(home) = home_dir() else {
        return vec![];
    };
    vec![
        ClientSpec {
            id: "workbuddy",
            name: "WorkBuddy",
            path: home.join(".workbuddy").join("mcp.json"),
            writable: true,
            hint: String::new(),
        },
        ClientSpec {
            id: "cursor",
            name: "Cursor",
            path: home.join(".cursor").join("mcp.json"),
            writable: true,
            hint: String::new(),
        },
        ClientSpec {
            id: "claude",
            name: "Claude Code",
            // 这个文件同时存着会话历史与按项目分身的 mcpServers，结构复杂 → 只检测
            path: home.join(".claude.json"),
            writable: false,
            hint: "claude mcp add devtoolkit -- \"<下面的 exe 路径>\"".to_string(),
        },
    ]
}

/// 检测单个客户端。返回的都是"事实"，判定交给界面，避免这里替用户下结论。
pub fn detect(spec: &ClientSpec, exe: &str) -> Value {
    let mut out = json!({
        "id": spec.id,
        "name": spec.name,
        "path": spec.path.to_string_lossy(),
        "writable": spec.writable,
        "hint": spec.hint,
        "exists": false,
        "broken": false,
        "registered": false,
        "disabled": false,
        "command": "",
        "command_ok": false,
        "others": Vec::<String>::new(),
    });

    let Ok(text) = fs::read_to_string(&spec.path) else {
        return out; // 文件没建过，exists 保持 false
    };
    out["exists"] = json!(true);

    let Ok(root) = serde_json::from_str::<Value>(&text) else {
        // 文件在但解析不了：这是用户自己要修的事，我们不能当成"未注册"糊过去
        out["broken"] = json!(true);
        return out;
    };

    let Some(servers) = root.get(SERVERS_KEY).and_then(|v| v.as_object()) else {
        return out;
    };

    let others: Vec<String> = servers
        .keys()
        .filter(|k| k.as_str() != SERVER_ID)
        .cloned()
        .collect();
    out["others"] = json!(others);

    let Some(entry) = servers.get(SERVER_ID) else {
        return out;
    };
    out["registered"] = json!(true);
    out["disabled"] = json!(entry.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false));

    let cmd = entry.get("command").and_then(|v| v.as_str()).unwrap_or("");
    out["command"] = json!(cmd);
    // 路径比对是这里最有价值的一条：构建目录换过之后，配置里指向的还是旧 exe，
    // 客户端会静默启动失败，而界面上看起来"配置明明写着"
    out["command_ok"] = json!(same_path(cmd, exe));

    out
}

/// 路径比较：忽略分隔符差异与大小写（Windows 路径两者都不敏感），但**不**做符号链接归并。
fn same_path(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let norm = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_lowercase();
    norm(a) == norm(b)
}

/// 原子写 JSON：同目录临时文件 → rename → 写前先备份原文件。
///
/// 备份语义与主配置相反：那边 `.bak` 是"最后一次成功保存"的副本（用于损坏恢复），
/// 这边 `.bak` 刻意保留**改写之前**的旧内容 —— 动的是别人的配置文件，能回滚才有意义。
fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| format!("序列化失败: {e}"))?;
    let dir = path.parent().ok_or("配置路径没有上级目录")?;
    if !dir.exists() {
        fs::create_dir_all(dir).map_err(|e| format!("创建目录失败 {}: {e}", dir.display()))?;
    }

    let p = path.to_string_lossy().to_string();
    if path.exists() {
        let bak = format!("{p}.bak");
        if let Err(e) = fs::copy(path, &bak) {
            // 备份失败不阻断：它是保险，不是前置条件（与主配置写入保持一致）
            eprintln!("[mcp-clients] 备份失败（忽略）: {e}");
        }
    }

    let tmp = format!("{p}.tmp");
    fs::write(&tmp, text).map_err(|e| format!("写入临时文件失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("替换配置文件失败: {e}"))?;
    Ok(())
}

/// 把 `devtoolkit` 条目写进客户端配置（不存在则创建），**保留其它 server 条目**。
///
/// 拒绝改写的原因都写明具体位置：这类操作一旦含糊，用户就不敢点第二次。
pub fn write_entry(spec: &ClientSpec, exe: &str) -> Result<Value, String> {
    if !spec.writable {
        return Err(format!(
            "{} 的配置文件结构复杂，为避免损坏只提供检测。请手工执行：{}",
            spec.name, spec.hint
        ));
    }
    if exe.is_empty() {
        return Err("没找到 devtoolkit-mcp 可执行文件，请先完成构建".into());
    }

    let mut root: Value = if spec.path.exists() {
        let text = fs::read_to_string(&spec.path)
            .map_err(|e| format!("读取失败 {}: {e}", spec.path.display()))?;
        if text.trim().is_empty() {
            json!({})
        } else {
            // 关键：解析失败**绝不**覆盖。用户手改坏的 JSON 里可能还有别的东西，
            // 直接重建等于替他把那些内容扔掉。
            serde_json::from_str(&text).map_err(|e| {
                format!(
                    "{} 不是合法 JSON，已放弃改写以免损坏原有内容（请手工修复）：{e}",
                    spec.path.display()
                )
            })?
        }
    } else {
        json!({})
    };

    if !root.is_object() {
        return Err(format!("{} 的根节点不是对象，已放弃改写", spec.path.display()));
    }

    let obj = root.as_object_mut().expect("已判定为对象");
    let servers = obj
        .entry(SERVERS_KEY)
        .or_insert_with(|| json!({}));
    let Some(servers) = servers.as_object_mut() else {
        return Err(format!("{SERVERS_KEY} 不是对象，已放弃改写"));
    };

    let preserved: Vec<String> = servers
        .keys()
        .filter(|k| k.as_str() != SERVER_ID)
        .cloned()
        .collect();

    servers.insert(
        SERVER_ID.to_string(),
        json!({ "command": exe, "args": [] }),
    );

    write_json_atomic(&spec.path, &root)?;

    Ok(json!({
        "path": spec.path.to_string_lossy(),
        "preserved": preserved,
    }))
}

// ---------------- 功能自检 ----------------

/// 自检结果里最多带回多少条工具说明，避免界面被刷屏
const MAX_TOOLS_SHOWN: usize = 40;

/// 真实拉起 MCP 代理进程，跑一次完整协议握手，把工具清单取回来。
///
/// 为什么不复用控制口而要另起进程：**工具清单的唯一真相在 MCP 二进制里**。
/// 在 GUI 里再抄一份必然随改动漂移，而"清单不准"的自检比没有自检更坏。
/// 让二进制自报还有个额外好处 —— 这一步同时验证了三件事：二进制能启动、
/// 协议实现正确、控制口可达。任何一件不成立都会在这条路径上暴露出来。
pub fn selftest(exe: &str, timeout: Duration) -> Value {
    let started = Instant::now();
    if exe.is_empty() {
        return json!({
            "ok": false,
            "stage": "binary",
            "error": "没找到 devtoolkit-mcp 可执行文件，请先构建（cargo build --release 会一起产出）",
            "elapsed_ms": 0,
            "tools": [],
        });
    }

    let msgs = match probe(exe, timeout) {
        Ok(v) => v,
        Err((stage, e)) => {
            return json!({
                "ok": false, "stage": stage, "error": e,
                "elapsed_ms": started.elapsed().as_millis() as u64, "tools": [],
            })
        }
    };

    let mut server = String::new();
    let mut version = String::new();
    let mut protocol = String::new();
    let mut tools: Vec<Value> = Vec::new();
    let mut saw_init = false;

    for m in &msgs {
        match m.get("id").and_then(|v| v.as_u64()) {
            Some(1) => {
                let r = m.get("result").unwrap_or(&Value::Null);
                saw_init = true;
                server = r
                    .pointer("/serverInfo/name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                version = r
                    .pointer("/serverInfo/version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                protocol = r
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
            }
            Some(2) => {
                if let Some(list) = m.pointer("/result/tools").and_then(|v| v.as_array()) {
                    tools = list
                        .iter()
                        .take(MAX_TOOLS_SHOWN)
                        .map(|t| {
                            json!({
                                "name": t.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                                "description": t.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                            })
                        })
                        .collect();
                }
            }
            _ => {}
        }
    }

    if !saw_init {
        return json!({
            "ok": false,
            "stage": "protocol",
            "error": "进程启动了但没有回应 initialize —— 二进制可能不是 MCP 服务，或启动即崩溃",
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "tools": [],
        });
    }
    if tools.is_empty() {
        return json!({
            "ok": false,
            "stage": "tools",
            "error": "握手成功但工具清单为空 —— 服务端没有暴露任何工具",
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "server": server, "version": version, "protocol": protocol,
            "tools": [],
        });
    }

    json!({
        "ok": true,
        "stage": "ok",
        "server": server,
        "version": version,
        "protocol": protocol,
        "tool_count": tools.len(),
        "tools": tools,
        "elapsed_ms": started.elapsed().as_millis() as u64,
    })
}

/// 用 stdio 发三条 JSON-RPC 并收结果。
/// 失败时返回 (阶段, 说明)，让上层能说清"断在哪一层"而不是笼统一句失败。
fn probe(exe: &str, timeout: Duration) -> Result<Vec<Value>, (&'static str, String)> {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(exe);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd
        .spawn()
        .map_err(|e| ("spawn", format!("无法启动 MCP 服务进程: {e}")))?;

    let reqs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize",
               "params":{"protocolVersion":PROTOCOL,"capabilities":{},
                         "clientInfo":{"name":"devtoolkit-gui","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    ];

    if let Some(mut si) = child.stdin.take() {
        let payload = reqs
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        // stdin 在这个块结束时 drop，服务端读到 EOF 会自行退出，不会留下僵留进程
        let _ = si.write_all(payload.as_bytes());
        let _ = si.flush();
    }

    let (tx, rx) = mpsc::channel::<String>();
    if let Some(so) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(so).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }

    let errbuf = Arc::new(Mutex::new(String::new()));
    if let Some(se) = child.stderr.take() {
        let buf = errbuf.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(se).lines().map_while(Result::ok) {
                let mut g = buf.lock().unwrap_or_else(|e| e.into_inner());
                if g.len() < 2000 {
                    g.push_str(&line);
                    g.push('\n');
                }
            }
        });
    }

    let deadline = Instant::now() + timeout;
    let mut msgs = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(line) => {
                if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                    msgs.push(v);
                }
            }
            Err(_) => break,
        }
    }

    // 收工：不管成没成都要收尸，否则进程会挂在后台
    let _ = child.kill();
    let _ = child.wait();

    let err = errbuf.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if msgs.is_empty() {
        let detail = if err.trim().is_empty() {
            format!("{} 秒内没有收到任何响应", timeout.as_secs())
        } else {
            format!("没有响应，服务进程输出：{}", err.trim())
        };
        return Err(("timeout", detail));
    }
    Ok(msgs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_at(id: &'static str, path: PathBuf, writable: bool) -> ClientSpec {
        ClientSpec {
            id,
            name: id,
            path,
            writable,
            hint: String::new(),
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dtk-mcpclients-{tag}-{}", crate::now_ms()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn detect_reports_missing_file_as_not_registered() {
        let d = tmp_dir("missing");
        let s = spec_at("t", d.join("nope.json"), true);
        let v = detect(&s, "C:/x/devtoolkit-mcp.exe");
        assert_eq!(v["exists"], json!(false));
        assert_eq!(v["registered"], json!(false));
    }

    #[test]
    fn detect_flags_broken_json_instead_of_pretending_unregistered() {
        let d = tmp_dir("broken");
        let p = d.join("mcp.json");
        fs::write(&p, "{ not json").unwrap();
        let v = detect(&spec_at("t", p, true), "C:/x/a.exe");
        assert_eq!(v["exists"], json!(true));
        assert_eq!(v["broken"], json!(true), "坏 JSON 必须单独报，不能混同于未注册");
        assert_eq!(v["registered"], json!(false));
    }

    #[test]
    fn detect_spots_stale_command_path() {
        let d = tmp_dir("stale");
        let p = d.join("mcp.json");
        fs::write(
            &p,
            json!({"mcpServers":{"devtoolkit":{"command":"D:/old/path/devtoolkit-mcp.exe","args":[]}}})
                .to_string(),
        )
        .unwrap();
        let v = detect(&spec_at("t", p, true), "E:/new/path/devtoolkit-mcp.exe");
        assert_eq!(v["registered"], json!(true));
        assert_eq!(v["command_ok"], json!(false), "路径过期必须被识别，这是最常见的隐性失效");
    }

    #[test]
    fn detect_treats_separator_and_case_differences_as_same_path() {
        let d = tmp_dir("norm");
        let p = d.join("mcp.json");
        fs::write(
            &p,
            json!({"mcpServers":{"devtoolkit":{"command":"E:\\ai_tools\\X\\devtoolkit-mcp.exe"}}})
                .to_string(),
        )
        .unwrap();
        let v = detect(&spec_at("t", p, true), "e:/ai_tools/x/devtoolkit-mcp.exe");
        assert_eq!(v["command_ok"], json!(true), "分隔符与大小写差异不该误报为路径失效");
    }

    #[test]
    fn detect_reads_disabled_flag() {
        let d = tmp_dir("disabled");
        let p = d.join("mcp.json");
        fs::write(
            &p,
            json!({"mcpServers":{"devtoolkit":{"command":"a.exe","disabled":true}}}).to_string(),
        )
        .unwrap();
        let v = detect(&spec_at("t", p, true), "a.exe");
        assert_eq!(v["disabled"], json!(true));
    }

    #[test]
    fn write_creates_file_and_keeps_other_servers() {
        let d = tmp_dir("keep");
        let p = d.join("mcp.json");
        fs::write(
            &p,
            json!({"mcpServers":{"other":{"command":"other.exe"}}}).to_string(),
        )
        .unwrap();
        let r = write_entry(&spec_at("t", p.clone(), true), "E:/x/devtoolkit-mcp.exe").unwrap();
        assert_eq!(r["preserved"], json!(["other"]));

        let after: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(after["mcpServers"]["devtoolkit"]["command"], json!("E:/x/devtoolkit-mcp.exe"));
        assert_eq!(
            after["mcpServers"]["other"]["command"],
            json!("other.exe"),
            "合并写入必须保留其它 server —— 覆盖掉它们等于替用户删配置"
        );
    }

    #[test]
    fn write_keeps_unrelated_top_level_keys() {
        let d = tmp_dir("toplevel");
        let p = d.join("mcp.json");
        fs::write(&p, json!({"mcpServers":{}, "theme":"dark", "n":1}).to_string()).unwrap();
        write_entry(&spec_at("t", p.clone(), true), "E:/x/a.exe").unwrap();
        let after: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(after["theme"], json!("dark"));
        assert_eq!(after["n"], json!(1));
    }

    #[test]
    fn write_refuses_on_broken_json() {
        let d = tmp_dir("refuse");
        let p = d.join("mcp.json");
        fs::write(&p, "{ 半截").unwrap();
        let e = write_entry(&spec_at("t", p.clone(), true), "E:/x/a.exe").unwrap_err();
        assert!(e.contains("不是合法 JSON"), "实际错误：{e}");
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "{ 半截",
            "拒绝改写时原文件必须一字不动"
        );
    }

    #[test]
    fn write_refuses_non_object_root() {
        let d = tmp_dir("array");
        let p = d.join("mcp.json");
        fs::write(&p, "[1,2,3]").unwrap();
        assert!(write_entry(&spec_at("t", p.clone(), true), "E:/x/a.exe").is_err());
        assert_eq!(fs::read_to_string(&p).unwrap(), "[1,2,3]");
    }

    #[test]
    fn write_refuses_when_servers_key_is_not_object() {
        let d = tmp_dir("badservers");
        let p = d.join("mcp.json");
        fs::write(&p, json!({"mcpServers":"oops"}).to_string()).unwrap();
        assert!(write_entry(&spec_at("t", p.clone(), true), "E:/x/a.exe").is_err());
        let after: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(after["mcpServers"], json!("oops"), "拒绝时必须原样保留");
    }

    #[test]
    fn write_refuses_non_writable_client_and_points_at_manual_command() {
        let d = tmp_dir("readonly");
        let mut s = spec_at("claude", d.join("claude.json"), false);
        s.hint = "claude mcp add ...".into();
        let e = write_entry(&s, "E:/x/a.exe").unwrap_err();
        assert!(e.contains("claude mcp add"), "拒绝时必须给出替代手工方案：{e}");
    }

    #[test]
    fn write_backs_up_previous_content() {
        let d = tmp_dir("bak");
        let p = d.join("mcp.json");
        fs::write(&p, json!({"mcpServers":{"keep":{"command":"k.exe"}}}).to_string()).unwrap();
        write_entry(&spec_at("t", p.clone(), true), "E:/x/a.exe").unwrap();

        let bak = PathBuf::from(format!("{}.bak", p.to_string_lossy()));
        let b: Value = serde_json::from_str(&fs::read_to_string(&bak).unwrap()).unwrap();
        assert!(
            b["mcpServers"].get("devtoolkit").is_none(),
            "备份应是改写之前的旧内容，才具备回滚价值"
        );
    }

    #[test]
    fn write_creates_missing_file_without_backup() {
        let d = tmp_dir("create");
        let p = d.join("nested").join("mcp.json");
        write_entry(&spec_at("t", p.clone(), true), "E:/x/a.exe").unwrap();
        assert!(p.exists(), "父目录不存在时应自动创建");
        assert!(!PathBuf::from(format!("{}.bak", p.to_string_lossy())).exists());
    }

    #[test]
    fn selftest_reports_binary_stage_when_exe_missing() {
        let v = selftest("", Duration::from_secs(1));
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["stage"], json!("binary"));
    }

    #[test]
    fn selftest_reports_spawn_failure_for_bogus_path() {
        let v = selftest("Z:/definitely/not/here.exe", Duration::from_secs(2));
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["stage"], json!("spawn"));
    }

    #[test]
    fn known_clients_marks_complex_client_as_read_only() {
        let list = known_clients();
        if let Some(claude) = list.iter().find(|c| c.id == "claude") {
            assert!(!claude.writable, "复杂结构的客户端必须是只读");
            assert!(claude.path.ends_with(".claude.json"));
        }
        if let Some(wb) = list.iter().find(|c| c.id == "workbuddy") {
            assert!(wb.writable);
        }
    }
}
