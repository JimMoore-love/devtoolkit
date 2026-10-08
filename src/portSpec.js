/**
 * 端口声明的**唯一**解析实现（前端侧）。
 *
 * 与 `src-tauri/src/ports.rs` 同规则、同结论：
 *   - 分隔符：`,` `;` `，` `；` `|` 以及任意空白
 *   - 端口号：1-65535 的十进制整数（拒绝 0、拒绝 `+80`、拒绝 `-1`、拒绝 `65536`）
 *   - 区间 `3000-3010`：起止颠倒视为非法；一次最多展开 `MAX_PORT_RANGE_SPAN` 个
 *   - 非法项**跳过**（这是「宽松」语义）；保存配置时的严格报错在后端 `ports::validate`
 *   - 输出保持**书写顺序**并去重
 *
 * 为什么单独一个文件：这段逻辑以前在仓库里有 5 份副本（Rust 侧 3 份、JS 侧 2 份），
 * 分隔符、跨度上限、是否接受端口 0 各写各的，导致
 * 「端口扫描里写 `1-2000` 能跑，项目管理里写 `3000-4000` 报跨度过大」这种用户可见的矛盾。
 * 现在前端只有这一份：`src/api.js` 直接 re-export，`scripts/port-guard.mjs` 也 import 它。
 *
 * 规则一致性由 `npm run check:guards` 的共享用例表守着（与 `ports.rs` 里的
 * `SHARED_CASES` 是同一张表），只改一边会直接失败。
 */

/** 端口区间一次最多展开多少个。与后端 `ports::MAX_PORT_RANGE_SPAN` 必须相同 */
export const MAX_PORT_RANGE_SPAN = 2000

const MIN_PORT = 1
const MAX_PORT = 65535

/** 分隔符与任意空白。取并集：任何一个历史写法都不会因为这次收敛而失效 */
const SEPARATORS = /[,;，；|\s]+/

/** 单个端口号：1-65535 的十进制整数；否则 null */
function parsePortNum(s) {
  const t = String(s).trim()
  if (!/^\d+$/.test(t)) return null
  const n = Number(t)
  if (!(n >= MIN_PORT && n <= MAX_PORT)) return null
  return n
}

/** 解析单项，返回 `[lo, hi]` 或 `null`（对应 Rust 的 `Item::Ports` / `Item::Bad`） */
function parseItem(raw) {
  const s = String(raw).trim()
  const dash = s.indexOf('-')
  const lo = dash < 0 ? s : s.slice(0, dash).trim()
  const hi = dash < 0 ? s : s.slice(dash + 1).trim()
  const a = parsePortNum(lo)
  const b = parsePortNum(hi)
  if (a === null || b === null) return null
  if (a > b) return null
  if (b - a + 1 > MAX_PORT_RANGE_SPAN) return null
  return [a, b]
}

/**
 * 解析端口声明：`3000,8080 9000`、`3000-3010`、`8080，9000`。
 * 去重、保持书写顺序、跳过非法项（绝不抛错）。
 */
export function parsePortSpec(spec) {
  const out = []
  const seen = new Set()
  for (const raw of String(spec ?? '').split(SEPARATORS)) {
    if (!raw) continue
    const item = parseItem(raw)
    if (!item) continue
    for (let p = item[0]; p <= item[1]; p++) {
      if (!seen.has(p)) {
        seen.add(p)
        out.push(p)
      }
    }
  }
  return out
}
