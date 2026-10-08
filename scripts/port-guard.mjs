// 常驻端口体检（只读，零副作用）
//
// 目的很具体：**别让 DevToolkit 或它引起的任何调整，把用户正在跑的服务搞挂。**
//
// 这个脚本不杀进程、不改配置、不发任何破坏性控制口命令 —— 它只做三件事：
//   1. 把受保护端口 + 托管项目声明的端口列出来，逐个探 TCP 能不能连、HTTP 回什么
//   2. 静态核实保护闸门是否完好（名单非空、闸门排在杀进程之前）
//   3. 需要时可当断言用：--expect 8000,9528 会在服务不可达时以非零码退出
//
// 之所以不依赖 DevToolkit 界面：应用本身可能正被调整/重启，
// 而这恰恰是最需要确认"服务有没有受影响"的时刻。
//
// 用法：
//   node scripts/port-guard.mjs                      # 体检并报告
//   node scripts/port-guard.mjs --expect 8000,9528   # 顺带断言这两个必须在
//   node scripts/port-guard.mjs --guards-only        # 只核实保护闸门（不碰网络）
//   node scripts/port-guard.mjs --ports 8080,3306    # 追加要探的端口
//   node scripts/port-guard.mjs --json               # 输出 JSON，便于其它脚本消费
import fs from 'node:fs'
import net from 'node:net'
import http from 'node:http'
import path from 'node:path'
import { configPath } from './ensure-gui.mjs'
import { checkGuards, formatProblems } from './guard-checks.mjs'
// 端口声明解析直接用前端那一份（src/portSpec.js 零依赖，Node 能直接 import）。
// 以前这里自己写了一份：认 `|`、跨度无上限、起止颠倒还会自动翻转 —— 与后端规则不同。
import { parsePortSpec } from '../src/portSpec.js'

// ---------- 参数 ----------
const argv = process.argv.slice(2)
const flag = (name) => argv.includes(`--${name}`)
const value = (name) => {
  const i = argv.indexOf(`--${name}`)
  return i >= 0 && argv[i + 1] ? argv[i + 1] : ''
}
const jsonOut = flag('json')
const guardsOnly = flag('guards-only')
const extraPorts = parsePortSpec(value('ports'))
const expectPorts = parsePortSpec(value('expect'))

// ---------- 探测 ----------
function tcpProbe(port, timeout = 1500) {
  return new Promise((res) => {
    const t0 = Date.now()
    const s = net.connect({ host: '127.0.0.1', port })
    const finish = (ok, why = '') => {
      try { s.destroy() } catch {}
      res({ ok, ms: Date.now() - t0, why })
    }
    s.setTimeout(timeout)
    s.on('connect', () => finish(true))
    s.on('error', (e) => finish(false, e.code || 'ERROR'))
    s.on('timeout', () => finish(false, 'TIMEOUT'))
  })
}

function httpProbe(port, timeout = 4000) {
  return new Promise((res) => {
    const req = http.request(
      { host: '127.0.0.1', port, path: '/', method: 'GET', timeout },
      (r) => {
        const code = r.statusCode
        r.destroy() // 只要状态码，不读 body
        res({ ok: true, status: code })
      }
    )
    req.on('error', () => res({ ok: false }))
    req.on('timeout', () => { req.destroy(); res({ ok: false }) })
    req.end()
  })
}

// ---------- 读配置 ----------
let cfg = {}
try {
  cfg = JSON.parse(fs.readFileSync(configPath(), 'utf8'))
} catch {
  // 配置读不到不算致命：仍可探端口，只是拿不到名单
}

const guard = checkGuards(cfg)
const protectedPorts = guard.protectedPorts
const projects = Array.isArray(cfg.projects) ? cfg.projects : []

// 口径：受保护端口 ∪ 托管项目声明的端口 ∪ 额外指定
const targets = new Map() // port -> 来源说明
for (const p of projects) {
  for (const n of parsePortSpec(p.ports)) {
    if (!targets.has(n)) targets.set(n, `项目「${p.name}」`)
  }
}
for (const n of protectedPorts) {
  if (!targets.has(n)) targets.set(n, '受保护名单')
}
for (const n of extraPorts) {
  if (!targets.has(n)) targets.set(n, '命令行指定')
}

const result = {
  protectedPorts,
  guardProblems: guard.problems,
  guardChecks: guard.checks,
  ports: [],
  failures: [],
}

if (!jsonOut) {
  console.log('\n════ 常驻端口体检 ════')
  console.log(`  配置文件: ${configPath()}`)
  console.log(`  受保护名单: ${protectedPorts.length ? `[${protectedPorts.join(', ')}]` : '（空！）'}`)
}

// ---------- 1. 保护闸门 ----------
if (!jsonOut) console.log('\n— 保护闸门（静态核实）—')
if (guard.problems.length) {
  if (!jsonOut) {
    console.log('  ⛔ 闸门不可信：')
    console.log(formatProblems(guard.problems))
    console.log('  → 在修好之前，受保护端口上的进程**随时可能被误杀**。')
  }
  result.failures.push('保护闸门被破坏')
} else if (!jsonOut) {
  // 打印 checkGuards 自己报的清单，而不是在这里另写一份 —— 上一版就是这里
  // 手写的三条 ✔ 文案，闸门语义改了之后它还印着"入口拦截"，看着正常实则过期
  for (const c of guard.checks) console.log(`  ✔ ${c}`)
}

// ---------- 2. 端口连通性 ----------
if (!guardsOnly) {
  if (!jsonOut) {
    console.log('\n— 端口连通性（这才是"能不能打开"的真相）—')
    if (!targets.size) console.log('  （没有目标端口：配置里既无托管项目也无保护名单）')
  }

  const list = [...targets.entries()].sort((a, b) => a[0] - b[0])
  for (const [port, from] of list) {
    const tcp = await tcpProbe(port)
    let http = null
    if (tcp.ok) http = await httpProbe(port)

    const entry = { port, from, reachable: tcp.ok, connectMs: tcp.ms, why: tcp.why, http }
    result.ports.push(entry)

    if (!jsonOut) {
      const label = `:${String(port).padEnd(5)}`
      if (tcp.ok) {
        const httpTxt = http?.ok ? `HTTP ${http.status}` : '非 HTTP / 无响应'
        console.log(`  ✔ ${label} 可连接 ${String(tcp.ms).padStart(4)}ms  ${httpTxt}   ← ${from}`)
      } else {
        const mustBe = expectPorts.includes(port)
        const mark = mustBe ? '✖' : '○'
        console.log(`  ${mark} ${label} 连不上（${tcp.why || '未知'}）                ← ${from}`)
      }
    }

    if (!tcp.ok && expectPorts.includes(port)) {
      result.failures.push(`:${port} 不可达（--expect 要求必须可用）`)
    }
  }

  // ---------- 3. 期望之外的补充断言 ----------
  for (const port of expectPorts) {
    if (!targets.has(port)) {
      const tcp = await tcpProbe(port)
      result.ports.push({ port, from: '--expect', reachable: tcp.ok, connectMs: tcp.ms, why: tcp.why, http: null })
      if (!tcp.ok) {
        result.failures.push(`:${port} 不可达（--expect 要求必须可用）`)
        if (!jsonOut) console.log(`  ✖ :${port} 连不上（${tcp.why}）                ← --expect`)
      } else if (!jsonOut) {
        console.log(`  ✔ :${port} 可连接 ${String(tcp.ms).padStart(4)}ms                   ← --expect`)
      }
    }
  }
}

// ---------- 收尾 ----------
if (jsonOut) {
  console.log(JSON.stringify(result, null, 2))
} else {
  const down = result.ports.filter((p) => !p.reachable)
  const up = result.ports.filter((p) => p.reachable)
  console.log('\n───────────────────────')
  console.log(`  可连接 ${up.length} 个 / 连不上 ${down.length} 个`)
  if (down.length) {
    console.log(`  未在监听: ${down.map((p) => ':' + p.port).join(', ')}`)
    console.log('  （若这些本该在跑，说明服务掉了；本脚本只做只读探测，未做任何改动）')
  }
  if (result.failures.length) {
    console.log('\n  失败项:')
    for (const f of result.failures) console.log(`    · ${f}`)
  } else {
    console.log('  ✔ 无失败项')
  }
  console.log('')
}

process.exit(result.failures.length ? 1 : 0)
