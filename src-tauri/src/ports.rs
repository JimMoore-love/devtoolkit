//! 端口声明的**唯一**解析实现。
//!
//! ## 为什么必须只有一份
//!
//! 这段逻辑曾在仓库里有 5 份副本，规则各不相同：
//!
//! | 位置 | 分隔符 | 端口 0 | 跨度上限 | 起止颠倒 |
//! |---|---|---|---|---|
//! | `main.rs::parse_port_spec`（宽松） | `,` `;` 空白 | 拒绝 | 256 | 跳过 |
//! | `main.rs::validate_port_spec`（严格） | `,` `;` 空白 | 拒绝 | 256 | 报错 |
//! | `network.rs::parse_ports`（扫描） | `,` `，` 空格 | **接受** | **2000** | 跳过 |
//! | `src/api.js::parsePortSpec` | `,` `;` 空白 | 拒绝 | 256 | 跳过 |
//! | `scripts/port-guard.mjs` | `,` `;` `\|` 空白 | 接受 | 无 | **自动翻转** |
//!
//! 后果是用户能直接看到的矛盾：**端口扫描里写 `1-2000` 能跑，
//! 项目管理里写 `3000-4000` 却报「跨度过大，最多 256」** ——
//! 同一个用户、同样的输入，两个页面给两种答案。
//!
//! ## 现在的结构
//!
//! 规则只在一处：`tokenize` → `parse_item`（含全部合法性判定）→ 展开去重。
//! 两条出口**只决定「遇到非法项怎么办」**，不重复任何判定逻辑：
//!
//! - [`parse_lenient`]：跳过非法项、绝不报错。用于运行时判定与端口扫描 ——
//!   配置里有一个写错的端口号，不该让整个扫描失败。
//! - [`validate`]：非法项报错，且必须说清「是哪个端口、错在哪」。用于保存配置 ——
//!   脏配置一旦落盘，运行期到处都要做防御，不如在入口拦住。
//!
//! 前端镜像见 `src/portSpec.js`，两者的规则一致性由 `npm run check:guards` 的
//! 共享用例表守着（改了一边没改另一边会直接失败）。

use std::collections::HashSet;

/// 端口区间（`3000-3010`）一次最多展开多少个。
///
/// 取 2000 而不是原来的 256：一是必须兼容端口扫描既有的 `1-2000`
/// （上限收到 256 会让这个常用写法直接失效），二是 2000 个端口
/// 在 64 并发 / 1.2s 超时下最坏约 37 秒，仍在可接受范围。
pub const MAX_PORT_RANGE_SPAN: u16 = 2000;

/// 端口号合法区间。0 不是可用的监听/连接端口，写 0 通常是把「默认值」当真了。
const MIN_PORT: u32 = 1;
const MAX_PORT: u32 = u16::MAX as u32;

/// 分隔符：ASCII 逗号/分号、全角逗号/分号、竖线，外加任意空白。
///
/// 取并集而非交集：以前 `network.rs` 认全角逗号、`main.rs` 认分号，
/// 统一后两边都认，任何一份历史配置都不会因为这次收敛而突然失效。
const SEPARATORS: [char; 5] = [',', ';', '，', '；', '|'];

/// 单个端口号：1-65535 的十进制整数。
///
/// 刻意要求「全部是 ASCII 数字」而不直接用 `parse::<u32>()`：
/// 后者会接受 `+80` 这种写法，导致与前端 `^\d+$` 的判定不一致。
fn parse_port_num(s: &str) -> Option<u16> {
    let t = s.trim();
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = t.parse().ok()?;
    if !(MIN_PORT..=MAX_PORT).contains(&n) {
        return None;
    }
    Some(n as u16)
}

/// 单项（`8080` 或 `3000-3010`）的解析结果。
///
/// `Bad` 带原因：宽松路径忽略它，严格路径直接把它当错误信息返回，
/// 这样「什么算非法」与「非法了怎么办」被彻底分开。
enum Item {
    Ports(u16, u16),
    Bad(String),
}

/// 解析单项，并在这里完成**全部**合法性判定。
fn parse_item(raw: &str) -> Item {
    let s = raw.trim();
    let (lo, hi) = match s.split_once('-') {
        Some((a, b)) => (a.trim(), b.trim()),
        None => (s, s),
    };
    let (Some(a), Some(b)) = (parse_port_num(lo), parse_port_num(hi)) else {
        return Item::Bad(format!("端口「{raw}」不是 1-65535 之间的数字"));
    };
    if a > b {
        return Item::Bad(format!("端口区间「{raw}」起止颠倒，应写作 {b}-{a}"));
    }
    // 按**个数**判定，与错误文案「跨度 N(个)」保持一致：`1-2000` 恰好 2000 个，合法
    if b - a + 1 > MAX_PORT_RANGE_SPAN {
        return Item::Bad(format!(
            "端口区间「{raw}」跨度 {}(个) 过大，最多 {MAX_PORT_RANGE_SPAN} 个",
            b - a + 1
        ));
    }
    Item::Ports(a, b)
}

/// 切分为非空项。所有分隔符与空项处理只在这里发生。
fn tokenize(spec: &str) -> impl Iterator<Item = &str> {
    spec.split(|c: char| SEPARATORS.contains(&c) || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// 展开区间并去重，保持**书写顺序**（前端按原样展示，排序交给需要它的调用方）。
fn push_range(a: u16, b: u16, seen: &mut HashSet<u16>, out: &mut Vec<u16>) {
    for p in a..=b {
        if seen.insert(p) {
            out.push(p);
        }
    }
}

/// 宽松解析：非法项**跳过**，绝不报错，绝不 panic。
///
/// 用于运行时判定与端口扫描 —— 配置被手工改坏也不该让整个功能失败。
pub fn parse_lenient(spec: &str) -> Vec<u16> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in tokenize(spec) {
        if let Item::Ports(a, b) = parse_item(raw) {
            push_range(a, b, &mut seen, &mut out);
        }
    }
    out
}

/// 严格校验：非法项**报错**，并返回第一条错误原因。
///
/// 用于保存配置 —— 把「静默失效」变成显式报错。以前非法项被默默丢掉，
/// 用户写 `3000-3010` 以为配了区间，实际一个端口都没生效而界面上毫无提示。
pub fn validate(spec: &str) -> Result<Vec<u16>, String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in tokenize(spec) {
        match parse_item(raw) {
            Item::Ports(a, b) => push_range(a, b, &mut seen, &mut out),
            Item::Bad(why) => return Err(why),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落库/展示用的「同一批输入 → 同一批结论」用例表。
    /// `scripts/guard-selftest.mjs` 里有字面相同的一张表在跑前端实现，
    /// 两边结论不一致时 `npm run check:guards` 会失败。
    const SHARED_CASES: &[(&str, &[u16])] = &[
        ("", &[]),
        ("   ", &[]),
        (",,;;||，，；；", &[]),
        ("8080", &[8080]),
        (" 8000 , 9528 ", &[8000, 9528]),
        ("3000,8080 9000;3000", &[3000, 8080, 9000]),
        ("3000，3010", &[3000, 3010]),
        ("3000；3010|3020", &[3000, 3010, 3020]),
        ("3000-3003", &[3000, 3001, 3002, 3003]),
        ("8080,9000-9001", &[8080, 9000, 9001]),
        ("3000-3002,3001", &[3000, 3001, 3002]),
        ("8080,3000", &[8080, 3000]),
        // 非法项
        ("0", &[]),
        ("-1", &[]),
        ("+80", &[]),
        ("65536", &[]),
        ("abc", &[]),
        ("3010-3000", &[]),
        ("1-2001", &[]),
        ("3000, oops, 8080", &[3000, 8080]),
    ];

    #[test]
    fn shared_case_table_holds() {
        for (spec, expect) in SHARED_CASES {
            assert_eq!(parse_lenient(spec), *expect, "宽松路径输入 {spec:?}");
        }
    }

    #[test]
    fn lenient_never_panics_on_dirty_input() {
        assert!(parse_lenient("").is_empty());
        assert!(parse_lenient(",,,;;;   ").is_empty());
        assert!(parse_lenient("-").is_empty());
        assert!(parse_lenient("-1").is_empty(), "负数不是端口");
        assert!(parse_lenient("0").is_empty(), "0 不是可监听端口");
        assert!(parse_lenient("65536").is_empty(), "超过 u16 上限");
        assert!(parse_lenient("99999999999999999999").is_empty());
        assert!(parse_lenient("端口").is_empty());
        assert!(parse_lenient(&"9".repeat(10_000)).is_empty());
        assert!(parse_lenient("+80").is_empty(), "不接受 + 号，与前端一致");
        // 脏项不该污染合法项
        assert_eq!(parse_lenient("3000, oops, 8080"), vec![3000, 8080]);
        assert_eq!(parse_lenient("3000 # 后端的端口"), vec![3000]);
    }

    #[test]
    fn lenient_keeps_written_order() {
        // 书写顺序即输出顺序（前端按原样回显），不是排序后的顺序
        assert_eq!(parse_lenient("8080,3000"), vec![8080, 3000]);
        assert_eq!(parse_lenient("9000-9001,8080"), vec![9000, 9001, 8080]);
    }

    #[test]
    fn range_span_boundary_is_inclusive() {
        // 上限按个数算：1-2000 恰好 2000 个，必须合法
        assert_eq!(parse_lenient("1-2000").len(), MAX_PORT_RANGE_SPAN as usize);
        // 差一个就超限：1-2001 共 2001 个
        assert!(parse_lenient("1-2001").is_empty());
        assert!(validate("1-2001").is_err());
    }

    #[test]
    fn reversed_and_oversize_ranges() {
        // 起止颠倒 / 跨度过大：宽松路径忽略，严格路径报错
        assert!(parse_lenient("3010-3000").is_empty());
        assert!(parse_lenient("1-65535").is_empty());
        let e = validate("3010-3000").unwrap_err();
        assert!(e.contains("颠倒"), "{e}");
        let e = validate("1-65535").unwrap_err();
        assert!(e.contains("过大"), "{e}");
    }

    #[test]
    fn validate_accepts_valid_and_reports_reason() {
        assert_eq!(validate("").unwrap(), Vec::<u16>::new());
        assert_eq!(validate("8080").unwrap(), vec![8080]);
        assert_eq!(validate(" 8000 , 9528 ").unwrap(), vec![8000, 9528]);
        assert_eq!(validate("3000-3002").unwrap(), vec![3000, 3001, 3002]);

        // 报错必须说清「是哪个端口、错在哪」，否则用户只能猜
        let e = validate("8000,abc").unwrap_err();
        assert!(e.contains("abc"), "{e}");
        let e = validate("3000-3010-3020").unwrap_err();
        assert!(e.contains("3000-3010-3020"), "{e}");
        assert!(validate("0").is_err());
        assert!(validate("65536").is_err());
    }

    #[test]
    fn lenient_and_strict_agree_on_what_is_legal() {
        // 同一批输入：宽松「非空」当且仅当严格「Ok 且非空」。
        // 这条断言就是防「两份规则又分叉」的机器化守护。
        let probes = [
            "", "8080", "0", "65536", "-1", "+80", "abc", "3000-3010", "3010-3000",
            "1-2000", "1-2001", "3000,8080", "3000-3010-3020", "99999999999999999999",
            "3000，3010", "3000；3010|3020", " 8000 , 9528 ",
        ];
        for spec in probes {
            if spec.is_empty() {
                assert!(validate(spec).unwrap().is_empty());
                assert!(parse_lenient(spec).is_empty());
                continue;
            }
            let strict = validate(spec);
            let lenient = parse_lenient(spec);
            assert_eq!(
                lenient.is_empty(),
                strict.as_ref().map(|v| v.is_empty()).unwrap_or(true),
                "宽松/严格对 {spec:?} 的合法性判定不一致：lenient={lenient:?} strict={strict:?}"
            );
        }
    }

    #[test]
    fn max_span_is_sane() {
        assert!(MAX_PORT_RANGE_SPAN >= 2);
        // 不能大到「一次扫描就挂住」：2000 端口 / 64 并发 × 1.2s ≈ 38s
        assert!(MAX_PORT_RANGE_SPAN as usize * 2 <= 4096 + 1);
    }
}
