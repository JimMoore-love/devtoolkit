//! 网络工具箱：跨平台网络诊断与查询工具集。
//! Ping / 路由追踪 / 端口扫描 / DNS 查询 / 本机网络信息 / ARP 主机发现 /
//! 路由表 / WOL 唤醒 / 下载测速。
//!
//! 解析层一律兼容 Windows 中文版与英文版输出 —— 两者的单位（ms / 毫秒）、
//! 分隔符（空格 / Tab）与段落顺序（如 nslookup 把"非权威应答"放在服务器信息
//! 之前）都不同。各解析函数处标注了对应的实测样例，改动时请一并对齐。

use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::platform::decode_output;

// ---------------- 数据类型 ----------------

#[derive(Serialize, Debug)]
pub struct PingResult {
    pub raw: String,
    pub sent: u32,
    pub received: u32,
    pub loss_pct: f64,
    pub min_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub times: Vec<f64>,
}

#[derive(Serialize, Debug)]
pub struct TracerouteResult {
    /// 原始输出，用于"查看原文"
    pub raw: String,
    /// 原始行（旧调用方仍在用，保持不动）
    pub hops: Vec<String>,
    /// 结构化逐跳数据
    pub parsed: Vec<Hop>,
    pub target: String,
    /// 是否走完全程（tracert 输出含"跟踪完成"）
    pub reached: bool,
    /// 平均时延最高的一跳，形如 "7  36.46.145.89  11.7ms"
    pub slowest: Option<String>,
}

/// 路由追踪的一跳。
///
/// 实测样例（Windows 中文版）：
/// ```text
///   1     1 ms     8 ms     1 ms  10.0.212.254
///   2    <1 毫秒   <1 毫秒    *     10.0.254.1
///   3     *        *        *     请求超时。
/// ```
/// 要点：单位可能是 `ms` 也可能是 `毫秒`；`<1` 表示小于 1ms；`*` 表示该次超时；
/// 整跳超时时地址缺失且尾部是"请求超时。"而非地址。
#[derive(Serialize, Debug, Clone)]
pub struct Hop {
    pub idx: u32,
    /// 三次探测的往返时延（ms）；None 表示该次为 `*`（超时）
    pub rtts: Vec<Option<f64>>,
    /// 该跳地址；整跳超时时为 None
    pub addr: Option<String>,
    /// 三次探测全部超时
    pub timed_out: bool,
    /// 有效探测的平均时延（ms）
    pub avg_ms: Option<f64>,
}

#[derive(Serialize, Debug)]
pub struct PortScanResult {
    pub scanned: usize,
    pub open: Vec<OpenPort>,
    pub closed: usize,
}

#[derive(Serialize, Debug)]
pub struct OpenPort {
    pub port: u16,
    pub service: String,
}

#[derive(Serialize, Debug)]
pub struct ArpHost {
    pub ip: String,
    pub mac: String,
    pub kind: String,
}

#[derive(Serialize, Debug)]
pub struct RouteEntry {
    pub dest: String,
    pub gateway: String,
    pub mask: String,
    pub iface: String,
    pub metric: String,
}

#[derive(Serialize, Debug)]
pub struct SpeedResult {
    pub mbps: f64,
    pub kbps: f64,
    pub bytes: u64,
    pub duration_s: f64,
    pub source: String,
    // ---- 以下为新增的质量指标；上面的旧字段一律保持不变 ----
    pub host: Option<String>,
    /// 到测速源的平均延迟（ms）
    pub latency_ms: Option<f64>,
    /// 抖动：相邻两次 RTT 的平均绝对差（ms）
    pub jitter_ms: Option<f64>,
    /// 丢包率（%）
    pub loss_pct: Option<f64>,
}

// ---------------- DNS 查询 ----------------

/// 一条 DNS 记录
#[derive(Serialize, Debug, Clone)]
pub struct DnsAnswer {
    /// 记录类型：A / AAAA / CNAME / MX / NS / TXT / SOA
    pub rtype: String,
    pub value: String,
    /// 附加说明（MX 优先级等）
    pub extra: String,
}

#[derive(Serialize, Debug)]
pub struct DnsResult {
    pub host: String,
    pub rtype: String,
    /// 应答所用的 DNS 服务器，形如 "public1.114dns.com (114.114.114.114)"
    pub server: String,
    pub answers: Vec<DnsAnswer>,
    /// `名称:` 行给出的规范名
    pub canonical: Option<String>,
    /// `Aliases:` 行给出的别名
    pub aliases: Vec<String>,
    pub elapsed_ms: u64,
    pub raw: String,
}

// ---------------- 本机网络信息 ----------------

#[derive(Serialize, Debug, Clone, Default)]
pub struct NetInterface {
    /// 适配器类型前缀（以太网适配器 / 无线局域网适配器 / Ethernet adapter …）
    pub kind: String,
    pub name: String,
    pub desc: String,
    pub mac: String,
    pub ipv4: String,
    pub mask: String,
    pub gateway: String,
    pub dns: Vec<String>,
    /// 媒体状态（已连接 / 媒体已断开 / 空）
    pub state: String,
    pub dhcp: String,
}

#[derive(Serialize, Debug)]
pub struct LocalNetInfo {
    pub hostname: String,
    pub interfaces: Vec<NetInterface>,
    /// 首个带网关的适配器的网关地址
    pub primary_gateway: String,
    pub raw: String,
}

/// 公网出口 IP
#[derive(Serialize, Debug)]
pub struct PublicIp {
    pub ip: String,
    /// 归属地（数据源提供时才有）
    pub location: String,
    pub source: String,
}

// ---------------- 工具函数 ----------------

/// 构造外部命令：Windows 下加 CREATE_NO_WINDOW，避免弹出控制台黑窗
fn build_cmd(prog: &str, args: &[&str]) -> Command {
    let mut c = Command::new(prog);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn run_cmd(prog: &str, args: &[&str]) -> String {
    let out = build_cmd(prog, args).output();
    match out {
        Ok(o) => {
            let mut s = decode_output(&o.stdout);
            if s.trim().is_empty() {
                let e = decode_output(&o.stderr);
                if !e.trim().is_empty() {
                    s = e;
                }
            }
            s
        }
        Err(e) => format!("无法执行 {prog}: {e}"),
    }
}

/// 常见端口服务名映射
fn service_name(port: u16) -> String {
    let m: HashMap<u16, &str> = HashMap::from([
        (21, "FTP"), (22, "SSH"), (23, "Telnet"), (25, "SMTP"), (53, "DNS"),
        (67, "DHCP"), (69, "TFTP"), (80, "HTTP"), (110, "POP3"), (123, "NTP"),
        (137, "NetBIOS"), (139, "NetBIOS"), (143, "IMAP"), (161, "SNMP"),
        (389, "LDAP"), (443, "HTTPS"), (445, "SMB"), (465, "SMTPS"),
        (514, "Syslog"), (587, "SMTP"), (631, "IPP打印"), (636, "LDAPS"),
        (993, "IMAPS"), (995, "POP3S"), (1080, "SOCKS"), (1433, "MSSQL"),
        (1521, "Oracle"), (1883, "MQTT"), (2181, "ZooKeeper"), (2375, "Docker"),
        (3000, "DevServer"), (3306, "MySQL"), (3389, "RDP"), (4444, "MetaSploit"),
        (4840, "OPC-UA"), (5000, "upnp/Flask"), (5432, "PostgreSQL"),
        (554, "RTSP"), (5540, "RTSP-扩展"), (5601, "Spark"), (5672, "RabbitMQ/AMQP"),
        (6379, "Redis"), (6633, "邮件"), (7001, "WebLogic"), (7077, "Kodi"),
        (8000, "HTTP-alt"), (8008, "HTTP-alt"), (8009, "AJP"), (8010, "HTTP-alt"),
        (8069, "海康ISAPI"), (8080, "HTTP-proxy"), (8081, "HTTP-alt"),
        (8082, "HTTP-alt"), (8086, "InfluxDB"), (8088, "HTTP-alt"),
        (8090, "HTTP-alt"), (8443, "HTTPS-alt"), (8500, "Consul"), (8848, "Nacos"),
        (8888, "HTTP-alt"), (9000, "SonarQube/MinIO"), (9001, "Supervisor"),
        (9090, "Prometheus"), (9200, "Elasticsearch"), (9300, "ES-集群"),
        (9527, "Frp"), (9999, "HTTP-alt"), (10000, "Webmin"),
        (11211, "Memcached"), (15672, "RabbitMQ管理"), (18083, "Bi-Directional"),
        (27017, "MongoDB"), (50000, "SAP"), (61616, "ActiveMQ"),
    ]);
    m.get(&port).map(|s| s.to_string()).unwrap_or_default()
}

// ---------------- Ping ----------------

/// 从 ping 输出里提取每次探测的时延（ms）。
///
/// 兼容 `time=23ms` / `time=23.4 ms` / `时间=1ms` / `时间<1ms` / `耗时: 23ms`。
///
/// `时间<1ms` 这一种最容易被漏掉：中文版 Windows 在 RTT 低于 1ms 时输出的是
/// `时间<1ms`（**没有等号**，实测 `ping 127.0.0.1` 即是），若只匹配 `时间=`，
/// 这一整行会被跳过 —— 表现为 ping 本机或内网快设备时收包数偏少，
/// 甚至显示"丢包 100%"。所以这里匹配的是 `时间` 而不是 `时间=`。
fn extract_ping_times(raw: &str) -> Vec<f64> {
    let mut times = Vec::new();
    for line in raw.lines() {
        let l = line.to_lowercase();
        let idx = match l
            .find("time=")
            .or_else(|| l.find("time<"))
            .or_else(|| l.find("时间"))
            .or_else(|| l.find("耗时"))
        {
            Some(i) => i,
            None => continue,
        };
        let rest = &line[idx..];
        let digits: String = rest
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if digits.is_empty() {
            continue;
        }
        // 数字后面必须紧跟时间单位：否则可能只是行里恰好出现的其他数字
        // （统计行的 "round trip times in milli-seconds" 就含 time 但没有时延）
        let tail = rest
            .split_once(digits.as_str())
            .map(|(_, t)| t.trim_start().to_lowercase())
            .unwrap_or_default();
        if !(tail.starts_with("ms") || tail.starts_with("毫秒")) {
            continue;
        }
        if let Ok(v) = digits.parse::<f64>() {
            times.push(v);
        }
    }
    times
}

/// 由一串 RTT 计算 (平均延迟, 抖动, 丢包率)。
///
/// 抖动取相邻两次 RTT 的平均绝对差（RFC 3550 风格）——比标准差更贴近
/// 实际使用中的"卡顿感"，因为一次尖峰不会被后续平稳样本稀释掉。
fn rtt_stats(times: &[f64], sent: u32) -> (Option<f64>, Option<f64>, Option<f64>) {
    if times.is_empty() {
        return (None, None, Some(100.0));
    }
    let avg = times.iter().sum::<f64>() / times.len() as f64;
    let jitter = if times.len() < 2 {
        None
    } else {
        let deltas: Vec<f64> = times.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        Some((deltas.iter().sum::<f64>() / deltas.len() as f64 * 10.0).round() / 10.0)
    };
    let loss = if sent == 0 {
        0.0
    } else {
        ((sent - times.len() as u32) as f64 / sent as f64 * 100.0 * 10.0).round() / 10.0
    };
    (
        Some((avg * 10.0).round() / 10.0),
        jitter,
        Some(loss.max(0.0)),
    )
}

pub fn ping(host: &str, count: u32, size: u32) -> Result<PingResult, String> {
    if host.trim().is_empty() {
        return Err("目标地址不能为空".into());
    }
    let count = count.clamp(1, 50);
    let size = size.clamp(1, 1400);
    let count_s = count.to_string();
    let size_s = size.to_string();

    #[cfg(target_os = "windows")]
    let raw = run_cmd("ping", &["-n", &count_s, "-l", &size_s, "-w", "2000", host]);
    #[cfg(not(target_os = "windows"))]
    let raw = run_cmd("ping", &["-c", &count_s, "-s", &size_s, "-W", "2", host]);

    let times: Vec<f64> = extract_ping_times(&raw);
    let received = times.len() as u32;
    let (min_ms, max_ms, avg_ms) = if received > 0 {
        let sum: f64 = times.iter().sum();
        (Some(times.iter().cloned().fold(f64::MAX, f64::min)), Some(times.iter().cloned().fold(0.0, f64::max)), Some((sum / received as f64 * 10.0).round() / 10.0))
    } else {
        (None, None, None)
    };
    Ok(PingResult {
        sent: count,
        received,
        loss_pct: ((count - received) as f64 / count as f64 * 100.0 * 10.0).round() / 10.0,
        min_ms,
        max_ms,
        avg_ms,
        raw,
        times,
    })
}

// ---------------- 路由追踪 ----------------

pub fn traceroute(host: &str) -> Result<TracerouteResult, String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("目标地址不能为空".into());
    }
    // -d 关闭反向域名解析：逐跳 DNS 反查会把每次追踪拖慢数秒，而 IP 已足够定位
    #[cfg(target_os = "windows")]
    let raw = run_cmd("tracert", &["-d", "-h", "20", "-w", "1500", host]);
    #[cfg(not(target_os = "windows"))]
    let raw = run_cmd("traceroute", &["-n", "-m", "20", "-w", "2", host]);

    let hops: Vec<String> = raw
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
        })
        .map(|l| l.trim().to_string())
        .collect();

    let parsed = parse_tracert(&raw);
    let reached = raw.contains("跟踪完成") || raw.contains("Trace complete");
    let slowest = parsed
        .iter()
        .filter_map(|h| h.avg_ms.map(|v| (h, v)))
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(h, v)| {
            format!(
                "第 {} 跳 {} {:.1}ms",
                h.idx,
                h.addr.clone().unwrap_or_else(|| "-".into()),
                v
            )
        });

    Ok(TracerouteResult {
        raw,
        hops,
        parsed,
        target: host.to_string(),
        reached,
        slowest,
    })
}

/// 时延单位 token（中文版为 `毫秒`，英文版为 `ms`）
fn is_rtt_unit(t: &str) -> bool {
    let t = t.trim().trim_end_matches([',', '，']);
    t.eq_ignore_ascii_case("ms") || t == "毫秒"
}

/// 解析单个时延 token：`1` / `1.5` / `<1`
///
/// `<1` 表示"小于 1ms"，取值 0.5 以便参与均值与排序（不取 0，否则平均线会被压低）。
fn parse_rtt_token(t: &str) -> Option<f64> {
    let t = t.trim();
    if let Some(rest) = t.strip_prefix('<') {
        return rest.trim().parse::<f64>().ok().map(|v| v / 2.0);
    }
    t.parse::<f64>().ok()
}

/// 把 tracert / traceroute 文本解析成逐跳结构。
///
/// 此前只是把整行原样塞进表格，跳数 / 三次 RTT / 地址 / 超时都没有拆出来，
/// 前端因此画不出时延着色与瓶颈跳。这里逐个 token 区分三类内容：
/// 时延（数值 + 单位）、超时（`*` 或"请求超时"）、地址（其余）。
fn parse_tracert(raw: &str) -> Vec<Hop> {
    let mut out: Vec<Hop> = Vec::new();
    for line in raw.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 2 {
            continue;
        }
        let Ok(idx) = toks[0].parse::<u32>() else {
            continue;
        };
        if idx == 0 || idx > 64 {
            continue;
        }
        // 第二个 token 得长得像时延或星号，避免把含数字的说明行当成跳
        let looks_like_hop =
            toks[1] == "*" || toks[1].starts_with('<') || toks[1].parse::<f64>().is_ok();
        if !looks_like_hop {
            continue;
        }

        let mut rtts: Vec<Option<f64>> = Vec::new();
        let mut addr: Option<String> = None;
        let mut i = 1;
        while i < toks.len() {
            let tk = toks[i];
            if tk == "*" {
                rtts.push(None);
                i += 1;
            } else if is_rtt_unit(tk) {
                i += 1; // 单位偶尔单独成 token
            } else if let Some(v) = parse_rtt_token(tk) {
                // 仅当紧随其后是单位时才认定为时延，否则多半是别的数字
                if i + 1 < toks.len() && is_rtt_unit(toks[i + 1]) {
                    rtts.push(Some(v));
                    i += 2;
                } else {
                    i += 1;
                }
            } else if tk.contains("超时") || tk.to_ascii_lowercase().contains("timed") {
                i += 1; // "请求超时。" / "Request timed out."
            } else if addr.is_none() {
                addr = Some(tk.trim_matches(['\r', ',']).to_string());
                i += 1;
            } else {
                i += 1;
            }
        }

        let valid: Vec<f64> = rtts.iter().flatten().copied().collect();
        let avg_ms = if valid.is_empty() {
            None
        } else {
            Some((valid.iter().sum::<f64>() / valid.len() as f64 * 10.0).round() / 10.0)
        };
        let timed_out = !rtts.is_empty() && valid.is_empty();
        out.push(Hop { idx, rtts, addr, timed_out, avg_ms });
    }
    out
}

// ---------------- 端口扫描 ----------------

/// 解析端口表达式："21,22,80,8000-8005"
fn parse_ports(spec: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for part in spec.split(&[',', '，', ' '][..]) {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if let Some((a, b)) = p.split_once('-') {
            if let (Ok(sa), Ok(sb)) = (a.trim().parse::<u16>(), b.trim().parse::<u16>()) {
                if sa <= sb && sb - sa < 2000 {
                    for x in sa..=sb {
                        ports.push(x);
                    }
                }
            }
        } else if let Ok(x) = p.parse::<u16>() {
            ports.push(x);
        }
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

pub fn port_scan(host: &str, ports_spec: &str) -> Result<PortScanResult, String> {
    let ports = parse_ports(ports_spec);
    if ports.is_empty() {
        return Err("端口列表为空或格式错误（示例: 21,22,80,8000-8005）".into());
    }
    // 解析主机地址
    let addrs: Vec<std::net::SocketAddr> = format!("{host}:0")
        .to_socket_addrs()
        .map_err(|e| format!("无法解析主机 {host}: {e}"))?
        .filter(|a| a.is_ipv4())
        .collect();
    let ip = addrs
        .first()
        .map(|a| a.ip())
        .ok_or_else(|| format!("无法解析主机 {host}"))?;

    let (tx, rx) = mpsc::channel();
    let chunk = 64usize;
    for group in ports.chunks(chunk) {
        let tx = tx.clone();
        let group = group.to_vec();
        thread::spawn(move || {
            for port in group {
                let sa = std::net::SocketAddr::new(ip, port);
                let ok = TcpStream::connect_timeout(&sa, Duration::from_millis(1200)).is_ok();
                let _ = tx.send((port, ok));
            }
        });
    }
    drop(tx);
    let mut open: Vec<OpenPort> = Vec::new();
    let mut closed = 0usize;
    for (port, ok) in rx {
        if ok {
            open.push(OpenPort {
                port,
                service: service_name(port),
            });
        } else {
            closed += 1;
        }
    }
    open.sort_by_key(|p| p.port);
    Ok(PortScanResult {
        scanned: ports.len(),
        open,
        closed,
    })
}

// ---------------- ARP 主机发现 ----------------

pub fn arp_hosts() -> Result<Vec<ArpHost>, String> {
    #[cfg(target_os = "windows")]
    let raw = run_cmd("arp", &["-a"]);
    #[cfg(not(target_os = "windows"))]
    let raw = run_cmd("arp", &["-an"]);

    let mut hosts = Vec::new();
    for line in raw.lines() {
        let t = line.trim();
        // Windows: "192.168.1.1  aa-bb-cc-dd-ee-ff  dynamic"
        let cols: Vec<&str> = t.split_whitespace().collect();
        #[cfg(target_os = "windows")]
        {
            if cols.len() >= 3 && cols[0].split('.').count() == 4 {
                let mac = cols[1].replace('-', ":").to_uppercase();
                if mac.len() == 17 && mac.contains(':') {
                    hosts.push(ArpHost {
                        ip: cols[0].to_string(),
                        mac,
                        kind: cols[2].to_string(),
                    });
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            // Unix: "? (192.168.1.1) at aa:bb:cc:dd:ee:ff on en0"
            let inner = t.trim_start_matches("? (").trim_start_matches("(");
            if let Some(ip_end) = inner.find(')') {
                let ip = &inner[..ip_end];
                if ip.split('.').count() == 4 {
                    if let Some(at_pos) = t.find(" at ") {
                        let rest = &t[at_pos + 4..];
                        let mac: String = rest
                            .chars()
                            .take_while(|c| c.is_ascii_hexdigit() || *c == ':')
                            .collect();
                        if mac.len() == 17 {
                            let kind = rest
                                .split_whitespace()
                                .nth(2)
                                .unwrap_or("dynamic")
                                .to_string();
                            hosts.push(ArpHost {
                                ip: ip.to_string(),
                                mac: mac.to_uppercase(),
                                kind,
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(hosts)
}

// ---------------- 路由表 ----------------

pub fn route_table() -> Result<Vec<RouteEntry>, String> {
    #[cfg(target_os = "windows")]
    let raw = run_cmd("route", &["print", "-4"]);
    #[cfg(not(target_os = "windows"))]
    let raw = run_cmd("netstat", &["-rn"]);

    let mut routes = Vec::new();
    let mut in_table = false;
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        #[cfg(target_os = "windows")]
        {
            if t.contains("Active Routes") || t.contains("活动路由") {
                in_table = true;
                continue;
            }
            if t.starts_with('=') || t.contains("Persistent") || t.contains("永久路由") {
                in_table = false;
                continue;
            }
            if !in_table {
                continue;
            }
            let cols: Vec<&str> = t.split_whitespace().collect();
            if cols.len() >= 5 && cols[0].split('.').count() == 4 {
                routes.push(RouteEntry {
                    dest: cols[0].to_string(),
                    gateway: cols[1].to_string(),
                    mask: cols[2].to_string(),
                    iface: cols[3].to_string(),
                    metric: cols[4].to_string(),
                });
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            // netstat -rn: "default  192.168.1.1  UGSc  en0" / "192.168.1/24  link#5  UCS  en0"
            let cols: Vec<&str> = t.split_whitespace().collect();
            if cols.len() >= 3 && (t.starts_with("default") || cols[0].parse::<std::net::Ipv4Addr>().is_ok()) {
                routes.push(RouteEntry {
                    dest: cols[0].to_string(),
                    gateway: cols[1].to_string(),
                    mask: "—".to_string(),
                    iface: cols.get(3).unwrap_or(&"-").to_string(),
                    metric: "—".to_string(),
                });
            }
        }
    }
    routes.truncate(80);
    Ok(routes)
}

// ---------------- WOL 网络唤醒 ----------------

pub fn wake_on_lan(mac: &str) -> Result<String, String> {
    let clean: String = mac
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if clean.len() != 12 {
        return Err("MAC 格式错误（示例: 00:11:22:33:44:55）".into());
    }
    let mut mac_bytes = [0u8; 6];
    for i in 0..6 {
        mac_bytes[i] = u8::from_str_radix(&clean[i * 2..i * 2 + 2], 16)
            .map_err(|_| "MAC 解析失败".to_string())?;
    }
    // 魔术包: FF*6 + MAC*16
    let mut packet = Vec::with_capacity(102);
    packet.extend_from_slice(&[0xFF; 6]);
    for _ in 0..16 {
        packet.extend_from_slice(&mac_bytes);
    }

    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("绑定 UDP 失败: {e}"))?;
    sock.set_broadcast(true).map_err(|e| format!("开启广播失败: {e}"))?;
    let mut sent = 0;
    for port in [9u16, 7] {
        for dst in ["255.255.255.255", "192.168.255.255"] {
            if sock.send_to(&packet, format!("{dst}:{port}")).is_ok() {
                sent += 1;
            }
        }
    }
    if sent == 0 {
        return Err("魔术包发送失败（可能被防火墙拦截）".into());
    }
    Ok(format!("魔术包已发送 {sent} 次（广播 255.255.255.255 与 192.168.255.255 的 9/7 端口）。目标设备需开启 WOL（BIOS/电源管理）并使用有线网络。"))
}

// ---------------- 下载测速 ----------------

/// 测速源：(显示名, 下载 URL, 用于测延迟的主机名)
///
/// 默认走国内镜像 —— 实测境外站点在国内网络下大多连不通
/// （api.ipify.org / ipinfo.io 均超时），Cloudflare 保留为"国际"选项。
fn speed_source(source: &str) -> (&'static str, &'static str, &'static str) {
    match source {
        "aliyun" => (
            "阿里云镜像",
            "https://mirrors.aliyun.com/ubuntu-releases/24.04/ubuntu-24.04.1-desktop-amd64.iso",
            "mirrors.aliyun.com",
        ),
        "tuna" => (
            "清华镜像",
            "https://mirrors.tuna.tsinghua.edu.cn/ubuntu-releases/24.04/ubuntu-24.04.1-desktop-amd64.iso",
            "mirrors.tuna.tsinghua.edu.cn",
        ),
        "cloudflare" => (
            "Cloudflare（国际）",
            "https://speed.cloudflare.com/__down?bytes=10000000",
            "speed.cloudflare.com",
        ),
        _ => (
            "中科大镜像",
            "https://mirrors.ustc.edu.cn/ubuntu-releases/24.04/ubuntu-24.04.1-desktop-amd64.iso",
            "mirrors.ustc.edu.cn",
        ),
    }
}

pub fn speedtest(source: &str) -> Result<SpeedResult, String> {
    let (name, url, host) = speed_source(source);

    // 先测延迟 / 抖动：只有带宽没有延迟的网络评价是不完整的，
    // 而这两项恰好决定"视频会议卡不卡、远程桌面跟不跟手"
    let (latency_ms, jitter_ms, loss_pct) = probe_link_quality(host);

    let start = Instant::now();
    let out = build_cmd(
        "curl",
        &["-sL", "--max-time", "15", "--range", "0-9999999", "-o", "-", url],
    )
    .output()
    .map_err(|e| format!("无法启动 curl（Win10+/macOS 自带）: {e}"))?;
    let bytes = out.stdout.len() as u64;
    let dur = start.elapsed().as_secs_f64();
    if bytes < 100_000 {
        return Err(format!(
            "下载量过小（{bytes} 字节），测速失败。该源当前不可达，建议换一个测速源。"
        ));
    }
    let mbps = bytes as f64 * 8.0 / dur / 1_048_576.0;
    Ok(SpeedResult {
        mbps: (mbps * 100.0).round() / 100.0,
        kbps: (bytes as f64 / 1024.0 / dur).round(),
        bytes,
        duration_s: (dur * 10.0).round() / 10.0,
        source: name.to_string(),
        host: Some(host.to_string()),
        latency_ms,
        jitter_ms,
        loss_pct,
    })
}

/// ping 目标主机以取得延迟 / 抖动 / 丢包。
///
/// 探测失败不阻断主流程（测速结果仍然有用），返回全 None 交给调用方降级展示。
fn probe_link_quality(host: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
    const COUNT: u32 = 6;
    let count_s = COUNT.to_string();
    #[cfg(target_os = "windows")]
    let raw = run_cmd("ping", &["-n", &count_s, "-w", "1200", host]);
    #[cfg(not(target_os = "windows"))]
    let raw = run_cmd("ping", &["-c", &count_s, "-W", "2", host]);

    let times = extract_ping_times(&raw);
    if times.is_empty() {
        return (None, None, None);
    }
    rtt_stats(&times, COUNT)
}

// ---------------- DNS 查询 ----------------

/// 若行以 keys 之一开头（忽略大小写）则返回其后的值
fn strip_key_ci<'a>(s: &'a str, keys: &[&str]) -> Option<&'a str> {
    let low = s.to_ascii_lowercase();
    for k in keys {
        if low.starts_with(&k.to_ascii_lowercase()) {
            // k 为 ASCII，前缀已匹配，按字节切分安全
            return Some(s[k.len()..].trim());
        }
    }
    None
}

/// 若行以 keys 之一开头（区分大小写，用于中文冒号）则返回其后的值
fn strip_key<'a>(s: &'a str, keys: &[&str]) -> Option<&'a str> {
    for k in keys {
        if s.starts_with(k) {
            return Some(s[k.len()..].trim());
        }
    }
    None
}

/// 把暂存的记录落盘（值为空的丢弃，TXT 折行未接上时会出现）
fn flush_answer(pending: &mut Option<DnsAnswer>, out: &mut Vec<DnsAnswer>) {
    if let Some(a) = pending.take() {
        if !a.value.trim().is_empty() {
            out.push(a);
        }
    }
}

/// nslookup 的参数向量。
///
/// **记录类型必须写成 `-type=A`（等号），不能写成 `-type A`（空格）。**
/// nslookup 的选项只认等号形式，写成空格时它会把 `A` 当成**要查询的 DNS 服务器名**，
/// 于是去解析主机名 "A"、超时三次（每次 2 秒），最后返回 0 条记录 —— 界面上看起来
/// 就像"域名不存在"。实测复现（Windows 中文版）：
/// ```text
/// > nslookup -type A www.baidu.com     ← 错
/// *** 请求 UnKnown 超时 / timeout was 2 seconds.  ×3，耗时 6s，无任何记录
/// > nslookup -type=A www.baidu.com     ← 对
/// 服务器:  public1.114dns.com / Address: 114.114.114.114 …
/// ```
/// 抽成纯函数是为了能被单测盯住：这条拼写规则错了不会报错，只会静默查空。
fn dns_args(rtype: &str, host: &str) -> Vec<String> {
    vec![format!("-type={rtype}"), host.to_string()]
}

/// DNS 查询。
///
/// 实测中文版 nslookup 的输出顺序是 **非权威应答 → 服务器 → Address**，
/// 且记录行用 Tab 分隔、MX 一行里有两个 `=`、TXT 的值还会折到下一行：
/// ```text
/// 非权威应答:
/// 服务器:  public1.114dns.com
/// Address:  114.114.114.114        ← 这是 DNS 服务器，不是查询结果
///
/// 名称:    www.a.shifen.com
/// Address:  183.2.172.177          ← 这才是应答地址
/// Aliases:  www.baidu.com
/// ```
/// 因此必须靠"是否已进入应答区"来区分两种 Address，不能简单取第一个。
/// 记录类型则按整行关键词判定（MX 的关键词在等号右侧，只看等号左边会漏）。
pub fn dns_lookup(host: &str, rtype: &str) -> Result<DnsResult, String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("域名不能为空".into());
    }
    // 拼进命令行前先挡住注入：域名只允许安全字符
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err("域名含非法字符（仅支持字母、数字、点、连字符与下划线）".into());
    }
    let rt = match rtype.trim().to_ascii_uppercase().as_str() {
        "" | "A" => "A",
        "AAAA" => "AAAA",
        "CNAME" => "CNAME",
        "MX" => "MX",
        "NS" => "NS",
        "TXT" => "TXT",
        "SOA" => "SOA",
        other => {
            return Err(format!(
                "不支持的记录类型「{other}」（支持 A / AAAA / CNAME / MX / NS / TXT / SOA）"
            ))
        }
    };

    let start = Instant::now();
    let args = dns_args(rt, host);
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let raw = run_cmd("nslookup", &refs);
    let elapsed_ms = start.elapsed().as_millis() as u64;

    let p = parse_nslookup(&raw, rt);
    let server = match (p.server_name.is_empty(), p.server_addr.is_empty()) {
        (false, false) => format!("{} ({})", p.server_name, p.server_addr),
        (false, true) => p.server_name,
        _ => p.server_addr,
    };

    Ok(DnsResult {
        host: host.to_string(),
        rtype: rt.to_string(),
        server,
        answers: p.answers,
        canonical: p.canonical,
        aliases: p.aliases,
        elapsed_ms,
        raw,
    })
}

/// nslookup 的结构化解析结果
struct DnsParsed {
    server_name: String,
    server_addr: String,
    answers: Vec<DnsAnswer>,
    canonical: Option<String>,
    aliases: Vec<String>,
}

/// 把 nslookup 文本解析成结构化记录。
///
/// 实测中文版输出顺序是 **非权威应答 → 服务器 → Address**，且记录行用 Tab
/// 分隔、MX 一行里有两个 `=`、TXT 的值还会折到下一行：
/// ```text
/// 非权威应答:
/// 服务器:  public1.114dns.com
/// Address:  114.114.114.114        ← 这是 DNS 服务器，不是查询结果
///
/// 名称:    www.a.shifen.com
/// Address:  183.2.172.177          ← 这才是应答地址
/// Aliases:  www.baidu.com
/// ```
/// 因此必须靠"是否已进入应答区"区分两种 Address，不能简单取第一个；
/// 记录类型也按整行关键词判定（MX 的关键词在等号右侧，只看等号左边会漏）。
fn parse_nslookup(raw: &str, rt: &str) -> DnsParsed {
    let mut server_name = String::new();
    let mut server_addr = String::new();
    let mut answers: Vec<DnsAnswer> = Vec::new();
    let mut canonical: Option<String> = None;
    let mut aliases: Vec<String> = Vec::new();
    let mut in_answer = false;
    let mut pending: Option<DnsAnswer> = None;

    for line in raw.lines() {
        let s = line.trim();
        if s.is_empty() {
            continue;
        }
        let low = s.to_ascii_lowercase();

        // 1) 折行续值：TXT 的值会单独占下一行，且自身可能含 `=`（如 v=spf1），
        //    所以必须排在"按 `=` 切记录"之前处理
        if let Some(p) = pending.as_mut() {
            let indented = line.len() != line.trim_start().len();
            if p.value.trim().is_empty() && (indented || s.starts_with('"')) {
                p.value = s.trim_matches('"').trim().to_string();
                continue;
            }
        }

        // 2) DNS 服务器块（进入应答区之前出现的才算）
        if let Some(v) = strip_key(s, &["服务器:", "Server:"]) {
            if !in_answer {
                server_name = v.to_string();
                continue;
            }
        }
        if let Some(v) = strip_key_ci(s, &["addresses:", "address:"]) {
            if !in_answer && server_addr.is_empty() {
                server_addr = v.to_string();
            } else {
                in_answer = true;
                flush_answer(&mut pending, &mut answers);
                for ip in v.split_whitespace() {
                    answers.push(DnsAnswer {
                        rtype: if rt == "AAAA" { "AAAA" } else { "A" }.to_string(),
                        value: ip.to_string(),
                        extra: String::new(),
                    });
                }
            }
            continue;
        }
        if let Some(v) = strip_key_ci(s, &["名称:", "name:"]) {
            in_answer = true;
            flush_answer(&mut pending, &mut answers);
            canonical = Some(v.to_string());
            continue;
        }
        if let Some(v) = strip_key_ci(s, &["aliases:", "alias:"]) {
            aliases.extend(v.split_whitespace().map(|x| x.to_string()));
            continue;
        }
        // 3) 超时提示行，不属于结果
        if low.starts_with("dns request timed out") || low.starts_with("timeout was") {
            continue;
        }

        // 4) 记录行：`<name>\t<field> = <value>`，MX 一行内有两个 `=`，
        //    故取最后一个 `=` 作为值分隔符
        if let Some(first_eq) = s.find('=') {
            let last_eq = s.rfind('=').unwrap_or(first_eq);
            let value_raw = s[last_eq + 1..].trim().to_string();

            let rtype = if low.contains("mail exchanger") || low.contains("mx preference") {
                "MX"
            } else if low.contains("canonical name") {
                "CNAME"
            } else if low.contains("nameserver") {
                "NS"
            } else if low.contains("text =") || low.contains("text=") {
                "TXT"
            } else if low.contains("internet address") {
                "A"
            } else if low.contains("primary name server") {
                "SOA"
            } else {
                "记录"
            };

            // MX 的优先级夹在两个 `=` 之间：`MX preference = 20, mail exchanger = mx2.qq.com`
            let extra = if rtype == "MX" && last_eq > first_eq {
                s[first_eq + 1..last_eq]
                    .split(',')
                    .next()
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .map(|p| format!("优先级 {p}"))
                    .unwrap_or_default()
            } else {
                String::new()
            };

            in_answer = true;
            flush_answer(&mut pending, &mut answers);
            pending = Some(DnsAnswer {
                rtype: rtype.to_string(),
                value: value_raw,
                extra,
            });
            continue;
        }

        // 5) 裸地址续行（多值应答）
        if in_answer && s.split('.').count() == 4 {
            flush_answer(&mut pending, &mut answers);
            answers.push(DnsAnswer {
                rtype: "A".to_string(),
                value: s.to_string(),
                extra: String::new(),
            });
        }
    }
    flush_answer(&mut pending, &mut answers);

    DnsParsed {
        server_name,
        server_addr,
        answers,
        canonical,
        aliases,
    }
}

// ---------------- 本机网络信息 ----------------

/// 读取本机网络配置（解析 `ipconfig /all`）。
///
/// 实测中文版结构：
/// ```text
/// 以太网适配器 vEthernet (WSL (Hyper-V firewall)):
///
///    描述. . . . . . . . . . . . . . . : Hyper-V Virtual Ethernet Adapter
///    物理地址. . . . . . . . . . . . . : 00-15-5D-F7-06-E2
///    IPv4 地址 . . . . . . . . . . . . : 172.26.224.1(首选)
///    DNS 服务器  . . . . . . . . . . . : 114.114.114.114
///                                        8.8.8.8        ← 续行
/// ```
/// 三个坑：适配器名自身含空格与括号（上面的 WSL 示例）、值带 `(首选)` 后缀、
/// 多值会折到下一行。故按首个冒号切分后还要做规范化与续行合并。
pub fn local_info() -> Result<LocalNetInfo, String> {
    #[cfg(target_os = "windows")]
    {
        let raw = run_cmd("ipconfig", &["/all"]);
        parse_ipconfig(&raw)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let raw = run_cmd("ifconfig", &["-a"]);
        Ok(LocalNetInfo {
            hostname: String::new(),
            interfaces: Vec::new(),
            primary_gateway: String::new(),
            raw,
        })
    }
}

#[cfg(target_os = "windows")]
fn parse_ipconfig(raw: &str) -> Result<LocalNetInfo, String> {
    const ADAPTER_PREFIXES: &[&str] = &[
        "以太网适配器",
        "无线局域网适配器",
        "未知适配器",
        "隧道适配器",
        "Ethernet adapter",
        "Wireless LAN adapter",
        "Unknown adapter",
        "Tunnel adapter",
    ];

    let mut hostname = String::new();
    let mut interfaces: Vec<NetInterface> = Vec::new();
    let mut cur: Option<NetInterface> = None;
    let mut last_key = String::new();

    for line in raw.lines() {
        let s = line.trim_end();
        if s.trim().is_empty() {
            continue;
        }

        // 适配器标题行：无缩进且以冒号结尾（如 "以太网适配器 以太网 11:"）
        if !line.starts_with(' ') && !line.starts_with('\t') && s.trim_end().ends_with(':') {
            if let Some(prev) = cur.take() {
                interfaces.push(prev);
            }
            let head = s.trim().trim_end_matches(':').trim();
            let mut kind = String::new();
            let mut name = head.to_string();
            for p in ADAPTER_PREFIXES {
                if let Some(rest) = head.strip_prefix(p) {
                    kind = (*p).to_string();
                    name = rest.trim().to_string();
                    break;
                }
            }
            cur = Some(NetInterface { kind, name, ..Default::default() });
            last_key.clear();
            continue;
        }

        let Some((k, v)) = s.split_once(':') else {
            // 无冒号 → 续行（DNS 多值、网关为空时不影响）
            if let Some(c) = cur.as_mut() {
                let v = s.trim();
                if !v.is_empty() {
                    if last_key == "dns" {
                        c.dns.push(v.to_string());
                    } else if last_key == "gateway" && c.gateway.is_empty() {
                        c.gateway = v.to_string();
                    }
                }
            }
            continue;
        };

        // key 去掉填充点与空格："IPv4 地址 " → "IPv4地址"
        let key = k.replace(['.', ' '], "");
        let val = v.trim().to_string();
        match key.as_str() {
            _ if key.contains("主机名") || key.eq_ignore_ascii_case("hostname") => {
                hostname = val;
            }
            _ if key.starts_with("IPv4") || key.eq_ignore_ascii_case("ipaddress") => {
                if let Some(c) = cur.as_mut() {
                    // 去掉 "(首选)" 之类的后缀
                    c.ipv4 = val.split('(').next().unwrap_or("").trim().to_string();
                }
                last_key = "ipv4".into();
            }
            _ if key.contains("子网掩码") || key.eq_ignore_ascii_case("subnetmask") => {
                if let Some(c) = cur.as_mut() {
                    c.mask = val;
                }
            }
            _ if key.contains("默认网关") || key.eq_ignore_ascii_case("defaultgateway") => {
                if let Some(c) = cur.as_mut() {
                    c.gateway = val;
                }
                last_key = "gateway".into();
            }
            _ if key.contains("物理地址") || key.eq_ignore_ascii_case("physicaladdress") => {
                if let Some(c) = cur.as_mut() {
                    c.mac = val.replace('-', ":").to_uppercase();
                }
            }
            _ if key.contains("描述") || key.eq_ignore_ascii_case("description") => {
                if let Some(c) = cur.as_mut() {
                    c.desc = val;
                }
            }
            _ if key.contains("DNS服务器") || key.eq_ignore_ascii_case("dnsservers") => {
                if let Some(c) = cur.as_mut() {
                    if !val.is_empty() {
                        c.dns.push(val);
                    }
                }
                last_key = "dns".into();
            }
            _ if key.contains("媒体状态") || key.eq_ignore_ascii_case("mediastate") => {
                if let Some(c) = cur.as_mut() {
                    c.state = val;
                }
            }
            _ if key.contains("DHCP已启用") || key.eq_ignore_ascii_case("dhcpenabled") => {
                if let Some(c) = cur.as_mut() {
                    c.dhcp = val;
                }
            }
            _ => {}
        }
    }
    if let Some(prev) = cur.take() {
        interfaces.push(prev);
    }

    // 丢掉既无 IPv4 也无 MAC 的空条目；有 IPv4 的排前面（正在使用的网卡优先）
    interfaces.retain(|i| !i.ipv4.is_empty() || !i.mac.is_empty());
    interfaces.sort_by_key(|i| i.ipv4.is_empty());

    let primary_gateway = interfaces
        .iter()
        .map(|i| i.gateway.as_str())
        .find(|g| !g.is_empty())
        .unwrap_or("")
        .to_string();

    Ok(LocalNetInfo {
        hostname,
        interfaces,
        primary_gateway,
        raw: raw.to_string(),
    })
}

// ---------------- 公网出口 IP ----------------

/// 查询公网出口 IP 与归属地。
///
/// 数据源只用国内可达的 myip.ipip.net（实测 api.ipify.org / ipinfo.io 均超时），
/// 返回形如：`当前 IP：111.20.167.66  来自于：中国 陕西 西安  移动`
///
/// 注意这是本模块唯一的**外发**请求，界面上应明确告知用户。
pub fn public_ip() -> Result<PublicIp, String> {
    let out = build_cmd("curl", &["-s", "--max-time", "8", "https://myip.ipip.net"])
        .output()
        .map_err(|e| format!("无法启动 curl: {e}"))?;
    let text = decode_output(&out.stdout);
    let text = text.trim();
    if text.is_empty() {
        return Err("查询超时或不可达（需要能访问 myip.ipip.net）".into());
    }
    let ip = text
        .split("IP：")
        .nth(1)
        .or_else(|| text.split("IP:").nth(1))
        .map(|s| s.split_whitespace().next().unwrap_or("").to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("未能从返回内容中解析出 IP：{text}"))?;
    let location = text
        .split("来自于：")
        .nth(1)
        .or_else(|| text.split("来自:").nth(1))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    Ok(PublicIp { ip, location, source: "myip.ipip.net".into() })
}

// ---------------- 结果导出 ----------------

/// 把诊断结果导出为 txt 到用户「下载」目录，返回完整路径。
///
/// WebView 里 `<a download>` 不会触发保存对话框，所以导出走后端写盘。
/// 这是用户主动触发的写入（不是清理动作）：文件名经清洗、已存在时自动加序号，
/// 不会覆盖任何既有文件；「下载」目录不存在则退回用户主目录。
pub fn export_report(name: &str, content: &str) -> Result<String, String> {
    if content.trim().is_empty() {
        return Err("没有可导出的内容".into());
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map_err(|_| "无法定位用户主目录".to_string())?;
    let downloads = std::path::Path::new(&home).join("Downloads");
    let dir = if downloads.is_dir() {
        downloads
    } else {
        std::path::Path::new(&home).to_path_buf()
    };

    // 只保留安全字符，避免路径穿越与非法文件名
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(40)
        .collect();
    let stem = if cleaned.is_empty() {
        "network-report".to_string()
    } else {
        cleaned
    };

    let mut path = dir.join(format!("{stem}.txt"));
    let mut n = 2;
    while path.exists() && n <= 99 {
        path = dir.join(format!("{stem}-{n}.txt"));
        n += 1;
    }

    std::fs::write(&path, content).map_err(|e| format!("写入失败: {e}"))?;
    Ok(path.display().to_string())
}

// ---------------- 命令行直读（测速的 HTTP 直连备用，未用） ----------------

#[allow(dead_code)]
fn http_download(_url: &str) -> Result<(u64, f64), String> {
    // 保留接口：后续如需脱离 curl 可换 reqwest
    let mut buf = [0u8; 65536];
    let mut total = 0u64;
    let start = Instant::now();
    let mut stream = TcpStream::connect("127.0.0.1:80").map_err(|e| e.to_string())?;
    while start.elapsed().as_secs() < 8 {
        let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total += n as u64;
    }
    Ok((total, start.elapsed().as_secs_f64()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------- 路由追踪解析 ----------------
    // 样本取自本机 Windows 中文版的真实输出

    #[test]
    fn tracert_parses_normal_hop() {
        let hops = parse_tracert("  1     1 ms     8 ms     1 ms  10.0.212.254 \n");
        assert_eq!(hops.len(), 1);
        let h = &hops[0];
        assert_eq!(h.idx, 1);
        assert_eq!(h.addr.as_deref(), Some("10.0.212.254"));
        assert_eq!(h.rtts, vec![Some(1.0), Some(8.0), Some(1.0)]);
        assert!(!h.timed_out);
        assert_eq!(h.avg_ms, Some(3.3));
    }

    #[test]
    fn tracert_treats_sub_ms_as_half() {
        // `<1 毫秒` 表示小于 1ms。取 0.5 而不是 0，否则均值会被压低
        let hops = parse_tracert("  2    <1 毫秒   <1 毫秒    *     10.0.254.1 \n");
        let h = &hops[0];
        assert_eq!(h.rtts, vec![Some(0.5), Some(0.5), None]);
        assert_eq!(h.addr.as_deref(), Some("10.0.254.1"));
        assert!(!h.timed_out, "只有部分星号不算整跳超时");
        assert_eq!(h.avg_ms, Some(0.5));
    }

    #[test]
    fn tracert_parses_full_timeout_hop() {
        let hops = parse_tracert("  3     *        *        *     请求超时。\n");
        let h = &hops[0];
        assert_eq!(h.rtts, vec![None, None, None]);
        assert!(h.timed_out);
        assert!(h.addr.is_none(), "别把「请求超时。」当成地址");
        assert_eq!(h.avg_ms, None);
    }

    #[test]
    fn tracert_skips_header_and_blank_lines() {
        let raw = "通过最多 20 个跃点跟踪到 223.5.5.5 的路由\n\n  1     1 ms     1 ms     1 ms  10.0.212.254 \n\n跟踪完成。\n";
        assert_eq!(parse_tracert(raw).len(), 1);
    }

    #[test]
    fn tracert_parses_english_output() {
        let raw = "  1     1 ms     1 ms     1 ms  10.0.212.254\n  2     *        *        *     Request timed out.\n";
        let hops = parse_tracert(raw);
        assert_eq!(hops.len(), 2);
        assert!(hops[1].timed_out);
    }

    #[test]
    fn tracert_never_misreads_a_numbered_line_as_hop() {
        // 第二列不像时延就应跳过，避免把含数字的说明行当成跳
        assert!(parse_tracert("  12 items found in cache\n").is_empty());
    }

    // ---------------- ping 时延提取 ----------------

    #[test]
    fn extracts_times_from_chinese_ping() {
        let raw = "来自 223.5.5.5 的回复: 字节=32 时间=7ms TTL=114\n\
                   来自 223.5.5.5 的回复: 字节=32 时间<1ms TTL=114\n\
                   来自 223.5.5.5 的回复: 字节=32 时间=8.5ms TTL=114";
        assert_eq!(extract_ping_times(raw), vec![7.0, 1.0, 8.5]);
    }

    #[test]
    fn extracts_times_from_english_ping() {
        let raw = "Reply from 8.8.8.8: bytes=32 time=23.4 ms TTL=114\n\
                   Reply from 8.8.8.8: bytes=32 time=25ms TTL=114";
        assert_eq!(extract_ping_times(raw), vec![23.4, 25.0]);
    }

    #[test]
    fn extracts_nothing_on_full_loss() {
        assert!(extract_ping_times("请求超时。\n请求超时。").is_empty());
    }

    #[test]
    fn extracts_sub_ms_samples_from_localhost() {
        // 实测 `ping 127.0.0.1`：低于 1ms 时输出没有等号。
        // 这是最容易被漏的一行——漏掉会让 ping 本机显示"丢包 100%"
        let raw = "来自 127.0.0.1 的回复: 字节=32 时间<1ms TTL=128\n\
                   来自 127.0.0.1 的回复: 字节=32 时间<1ms TTL=128";
        assert_eq!(extract_ping_times(raw), vec![1.0, 1.0]);
    }

    #[test]
    fn ignores_ping_summary_lines() {
        // 英文统计行含 "times"，但后面没有时延，不能被混进逐次采样
        let raw = "Reply from 8.8.8.8: bytes=32 time=10ms TTL=114\n\
                   Approximate round trip times in milli-seconds:\n\
                       Minimum = 10ms, Maximum = 10ms, Average = 10ms";
        assert_eq!(extract_ping_times(raw), vec![10.0]);
    }

    // ---------------- 延迟统计 ----------------

    #[test]
    fn rtt_stats_computes_average_jitter_and_loss() {
        let (avg, jit, loss) = rtt_stats(&[10.0, 20.0, 10.0], 4);
        assert_eq!(avg, Some(13.3));
        // |20-10| + |10-20| = 20，除以 2 个间隔 → 10
        assert_eq!(jit, Some(10.0));
        assert_eq!(loss, Some(25.0));
    }

    #[test]
    fn rtt_stats_reports_total_loss() {
        let (avg, jit, loss) = rtt_stats(&[], 4);
        assert!(avg.is_none() && jit.is_none());
        assert_eq!(loss, Some(100.0));
    }

    #[test]
    fn rtt_stats_single_sample_has_no_jitter() {
        let (_, jit, loss) = rtt_stats(&[5.0], 1);
        assert!(jit.is_none(), "单次探测算不出抖动");
        assert_eq!(loss, Some(0.0));
    }

    // ---------------- nslookup 解析 ----------------
    // 样本是本机实测的中文版输出：注意「非权威应答」在最前、服务器信息在它之后

    const NSLOOKUP_A: &str = "非权威应答:\n\
服务器:  public1.114dns.com\n\
Address:  114.114.114.114\n\
\n\
名称:    www.a.shifen.com\n\
Address:  183.2.172.177\n\
Aliases:  www.baidu.com\n";

    #[test]
    fn dns_keeps_server_address_out_of_answers() {
        let p = parse_nslookup(NSLOOKUP_A, "A");
        assert_eq!(p.server_name, "public1.114dns.com");
        assert_eq!(p.server_addr, "114.114.114.114");
        assert_eq!(p.answers.len(), 1, "DNS 服务器地址不能被当成查询结果");
        assert_eq!(p.answers[0].value, "183.2.172.177");
    }

    #[test]
    fn dns_reads_canonical_and_aliases() {
        let p = parse_nslookup(NSLOOKUP_A, "A");
        assert_eq!(p.canonical.as_deref(), Some("www.a.shifen.com"));
        assert_eq!(p.aliases, vec!["www.baidu.com"]);
    }

    /// 拼错这个参数**不会报错**，只会静默查空（超时 6 秒、0 条记录），
    /// 界面上还会显示成"域名可能不存在"——所以必须用测试钉住等号写法。
    #[test]
    fn dns_args_must_use_equals_form_for_type() {
        assert_eq!(dns_args("A", "www.baidu.com"), vec!["-type=A", "www.baidu.com"]);
        assert_eq!(dns_args("MX", "qq.com"), vec!["-type=MX", "qq.com"]);
        // 退化成 `["-type", "A", ...]` 时 nslookup 会把 "A" 当成 DNS 服务器
        for rt in ["A", "AAAA", "CNAME", "MX", "NS", "TXT", "SOA"] {
            let args = dns_args(rt, "example.com");
            assert!(
                args[0].starts_with("-type="),
                "nslookup 的 -type 必须带等号，否则会被当成服务器参数: {args:?}"
            );
            assert_eq!(args.len(), 2, "除 -type 外只能有域名这一个参数: {args:?}");
        }
    }

    #[test]
    fn dns_splits_mx_on_the_second_equals() {
        let raw = "非权威应答:\n\
服务器:  dns\n\
Address:  114.114.114.114\n\
\n\
qq.com\tMX preference = 20, mail exchanger = mx2.qq.com\n";
        let p = parse_nslookup(raw, "MX");
        assert_eq!(p.answers.len(), 1);
        assert_eq!(p.answers[0].rtype, "MX");
        assert_eq!(p.answers[0].value, "mx2.qq.com");
        assert_eq!(p.answers[0].extra, "优先级 20");
    }

    #[test]
    fn dns_joins_wrapped_txt_value() {
        let raw = "非权威应答:\n\
服务器:  dns\n\
Address:  114.114.114.114\n\
\n\
qq.com\ttext =\n\
\n\
\t\"v=spf1 include:spf.mail.qq.com -all\"\n";
        let p = parse_nslookup(raw, "TXT");
        assert_eq!(p.answers.len(), 1);
        assert_eq!(p.answers[0].rtype, "TXT");
        assert_eq!(p.answers[0].value, "v=spf1 include:spf.mail.qq.com -all");
    }

    #[test]
    fn dns_reads_nameserver_records() {
        let raw = "非权威应答:\n\
服务器:  dns\n\
Address:  114.114.114.114\n\
\n\
qq.com\tnameserver = ns4.qq.com\n\
qq.com\tnameserver = ns3.qq.com\n";
        let p = parse_nslookup(raw, "NS");
        assert_eq!(p.answers.len(), 2);
        assert!(p.answers.iter().all(|a| a.rtype == "NS"));
        assert_eq!(p.answers[0].value, "ns4.qq.com");
    }

    #[test]
    fn dns_skips_timeout_notice_lines() {
        // CNAME 查询实测会出现夹在中间的超时提示，不能被当成记录
        let raw = "非权威应答:\n\
DNS request timed out.\n\
    timeout was 2 seconds.\n\
服务器:  UnKnown\n\
Address:  114.114.114.114\n\
\n\
www.qq.com\tcanonical name = ins-r23tsuuf.ias.tencent-cloud.net\n";
        let p = parse_nslookup(raw, "CNAME");
        assert_eq!(p.answers.len(), 1);
        assert_eq!(p.answers[0].rtype, "CNAME");
        assert_eq!(p.answers[0].value, "ins-r23tsuuf.ias.tencent-cloud.net");
    }

    #[test]
    fn dns_reports_empty_answers_for_missing_domain() {
        let raw = "服务器:  UnKnown\n\
Address:  114.114.114.114\n\
\n\
*** UnKnown 找不到 www.not-exist-xyz.cn: Non-existent domain\n";
        let p = parse_nslookup(raw, "A");
        assert!(p.answers.is_empty());
        // 解析不到记录时，服务器地址仍要拿得到，便于判断是哪台服务器答的
        assert_eq!(p.server_addr, "114.114.114.114");
    }

    // ---------------- ipconfig 解析（仅 Windows） ----------------
    // 样本取自本机实测输出；适配器名含空格与括号是真实情况

    #[cfg(target_os = "windows")]
    mod win {
        use super::*;

        const IPCONFIG: &str = "\n\
Windows IP 配置\n\
\n\
   主机名  . . . . . . . . . . . . . : DESKTOP-TEST\n\
\n\
以太网适配器 vEthernet (WSL (Hyper-V firewall)):\n\
\n\
   描述. . . . . . . . . . . . . . . : Hyper-V Virtual Ethernet Adapter\n\
   物理地址. . . . . . . . . . . . . : 00-15-5D-F7-06-E2\n\
   IPv4 地址 . . . . . . . . . . . . : 172.26.224.1(首选) \n\
   子网掩码  . . . . . . . . . . . . : 255.255.240.0\n\
   默认网关. . . . . . . . . . . . . : \n\
\n\
以太网适配器 以太网 11:\n\
\n\
   描述. . . . . . . . . . . . . . . : Realtek PCIe GbE Family Controller #7\n\
   物理地址. . . . . . . . . . . . . : 20-53-8D-03-A3-31\n\
   IPv4 地址 . . . . . . . . . . . . : 10.0.212.58(首选) \n\
   子网掩码  . . . . . . . . . . . . : 255.255.255.0\n\
   默认网关. . . . . . . . . . . . . : 10.0.212.254\n\
   DNS 服务器  . . . . . . . . . . . : 114.114.114.114\n\
                                       8.8.8.8\n";

        #[test]
        fn splits_adapters_with_brackets_in_name() {
            let info = parse_ipconfig(IPCONFIG).expect("解析不应失败");
            assert_eq!(info.hostname, "DESKTOP-TEST");
            assert_eq!(info.interfaces.len(), 2, "适配器名里的括号不应导致误切");
        }

        #[test]
        fn cleans_preferred_suffix_and_mac_dashes() {
            let info = parse_ipconfig(IPCONFIG).unwrap();
            let eth = info
                .interfaces
                .iter()
                .find(|i| i.name == "以太网 11")
                .expect("应解析出「以太网 11」");
            assert_eq!(eth.ipv4, "10.0.212.58", "(首选) 后缀应被去掉");
            assert_eq!(eth.mask, "255.255.255.0");
            assert_eq!(eth.mac, "20:53:8D:03:A3:31", "MAC 应统一为冒号分隔大写");
            assert_eq!(eth.kind, "以太网适配器");
        }

        #[test]
        fn merges_multiline_dns() {
            let info = parse_ipconfig(IPCONFIG).unwrap();
            let eth = info.interfaces.iter().find(|i| i.name == "以太网 11").unwrap();
            assert_eq!(
                eth.dns,
                vec!["114.114.114.114", "8.8.8.8"],
                "折行的第二个 DNS 要接上"
            );
        }

        #[test]
        fn primary_gateway_skips_empty_adapters() {
            // 第一个适配器网关为空，应取到第二个的网关
            let info = parse_ipconfig(IPCONFIG).unwrap();
            assert_eq!(info.primary_gateway, "10.0.212.254");
        }
    }

    // ---------------- 导出与测速源 ----------------

    #[test]
    fn export_rejects_empty_content() {
        assert!(export_report("x", "   ").is_err());
    }

    #[test]
    fn unknown_speed_source_falls_back_to_domestic() {
        let (name, url, host) = speed_source("no-such-source");
        assert_eq!(name, "中科大镜像");
        assert!(url.starts_with("https://mirrors.ustc.edu.cn"), "兜底必须是国内源");
        assert_eq!(host, "mirrors.ustc.edu.cn");
    }
}
