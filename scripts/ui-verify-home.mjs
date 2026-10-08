/**
 * 验证首页「运行中的项目」区块的渲染与数据。
 *
 * 为什么单独一个脚本：这块是本次唯一改动的落点，而它是否画对完全取决于
 * scan_ports.runtime 的真实内容（托管 / 外部启动两条分支渲染不同）。
 * 走无头夹具（ui-harness.mjs 把 invoke 转发给真实控制口）就能拿到真实运行态，
 * 又不必和真机窗口的抓图/遮挡问题纠缠。
 *
 * 用法：
 *   node scripts/ui-harness.mjs --port=4321          # 先起夹具
 *   msedge --headless=new --remote-debugging-port=9333 http://127.0.0.1:4321/
 *   node scripts/ui-verify-home.mjs 9333 --wait=15
 */

import fs from 'node:fs'
import path from 'node:path'

const PORT = process.argv[2] || '9333'
const BASE = `http://127.0.0.1:${PORT}`
const OUTDIR = path.resolve('.shots')
fs.mkdirSync(OUTDIR, { recursive: true })

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

const WAIT_MS = (() => {
  const a = process.argv.find((x) => x.startsWith('--wait='))
  return a ? Number(a.slice(7)) * 1000 : 15000
})()

/* ---------- 连接 ---------- */

let list
const deadline = Date.now() + WAIT_MS
for (;;) {
  try {
    list = await (await fetch(`${BASE}/json/list`)).json()
    break
  } catch (e) {
    if (Date.now() > deadline) {
      console.error(`连不上调试端口 ${PORT}：${e.message}`)
      process.exit(2)
    }
    await sleep(400)
  }
}

const page =
  list.find((t) => t.type === 'page' && !/^about:/.test(t.url || '')) ||
  list.find((t) => t.type === 'page') ||
  list[0]
if (!page?.webSocketDebuggerUrl) {
  console.error('没有可调试的 page target')
  process.exit(3)
}
console.log(`→ 目标页面：${page.url}`)

const ws = new WebSocket(page.webSocketDebuggerUrl)
const pending = new Map()
let seq = 0
const logs = []

ws.addEventListener('message', (ev) => {
  let m
  try {
    m = JSON.parse(ev.data)
  } catch {
    return
  }
  if (m.id && pending.has(m.id)) {
    pending.get(m.id)(m)
    pending.delete(m.id)
    return
  }
  if (m.method === 'Runtime.consoleAPICalled') {
    const t = (m.params.args || []).map((a) => a.value ?? a.description ?? '').join(' ')
    logs.push(`[console.${m.params.type}] ${t}`)
  } else if (m.method === 'Runtime.exceptionThrown') {
    const d = m.params.exceptionDetails || {}
    logs.push(`[exception] ${d.text || ''} ${d.exception?.description || ''}`)
  } else if (m.method === 'Log.entryAdded') {
    const e = m.params.entry || {}
    if (e.level === 'error' || e.level === 'warning') logs.push(`[log.${e.level}] ${e.text || ''}`)
  }
})

await new Promise((res, rej) => {
  ws.addEventListener('open', res)
  ws.addEventListener('error', rej)
})

function send(method, params = {}, timeoutMs = 8000) {
  const id = ++seq
  ws.send(JSON.stringify({ id, method, params }))
  return new Promise((res) => {
    const t = setTimeout(() => {
      pending.delete(id)
      res({ __timeout: true, method })
    }, timeoutMs)
    pending.set(id, (m) => {
      clearTimeout(t)
      res(m)
    })
  })
}

async function evaluate(expression) {
  const r = await send('Runtime.evaluate', {
    expression: `(() => { try { return JSON.stringify(${expression}) } catch (e) { return JSON.stringify({ __err: String(e) }) } })()`,
    returnByValue: true,
    awaitPromise: true,
  })
  if (r.__timeout) return { __err: 'CDP 请求超时（调试连接可能已断开）' }
  const raw = r.result?.result?.value
  if (raw == null) return { __err: JSON.stringify(r).slice(0, 300) }
  try {
    return JSON.parse(raw)
  } catch {
    return { __err: String(raw).slice(0, 300) }
  }
}

async function shot(name) {
  const r = await send('Page.captureScreenshot', { format: 'png', fromSurface: true }, 20000)
  const b64 = r.result?.data
  if (!b64) {
    console.log(`  截图失败：${JSON.stringify(r).slice(0, 200)}`)
    return null
  }
  const p = path.join(OUTDIR, name)
  fs.writeFileSync(p, Buffer.from(b64, 'base64'))
  console.log(`  已保存 ${p}`)
  return p
}

await send('Runtime.enable')
await send('Log.enable')
await send('Page.enable')

/* ---------- 等页面挂载 ---------- */

console.log('\n=== 1. 等页面挂载 ===')
let mounted = false
for (let i = 0; i < 30; i++) {
  const n = await evaluate(`document.getElementById('app')?.children.length ?? -1`)
  if (typeof n === 'number' && n > 0) {
    mounted = true
    console.log(`  已挂载（#app 子节点 ${n}）`)
    break
  }
  await sleep(500)
}
if (!mounted) {
  console.log('  !! 页面没挂载 —— 下面 dump 日志')
  for (const l of logs.slice(-30)) console.log('  ' + l)
  await shot('home-blank.png')
  ws.close()
  process.exit(4)
}

// 等一轮运行态回来（全局轮询 5 秒一次）
await sleep(7000)

/* ---------- 读渲染结果 ---------- */

console.log('\n=== 2. 首页渲染结果 ===')
const snap = await evaluate(`(() => {
  const cards = [...document.querySelectorAll('.stat-grid .stat-card')].map((c) => ({
    k: (c.querySelector('.k')?.textContent || '').trim(),
    v: (c.querySelector('.v')?.textContent || '').trim(),
    foot: (c.querySelector('.foot')?.textContent || '').trim().replace(/\\s+/g, ' '),
  }))
  const rows = [...document.querySelectorAll('.task-row')].map((r) => ({
    name: (r.querySelector('.n')?.textContent || '').trim().replace(/\\s+/g, ' '),
    sub: (r.querySelector('.s')?.textContent || '').trim().replace(/\\s+/g, ' '),
    btn: (r.querySelector('button')?.textContent || '').trim().replace(/\\s+/g, ' '),
  }))
  const emptyBlock = [...document.querySelectorAll('.empty p')].map((e) => e.textContent.trim())
  const titles = [...document.querySelectorAll('.card-title')].map((t) => (t.textContent || '').trim().replace(/\\s+/g, ' '))
  return { url: location.href, cards, rows, emptyBlock, titles }
})()`)

if (snap.__err) {
  console.log('  读取失败：' + snap.__err)
} else {
  console.log('  状态卡：')
  for (const c of snap.cards) console.log(`    ${c.k.padEnd(12)} ${c.v.padEnd(8)} ${c.foot}`)
  console.log(`  运行中的项目：${snap.rows.length} 行`)
  for (const r of snap.rows) {
    console.log(`    · ${r.name}`)
    console.log(`      ${r.sub}`)
    console.log(`      按钮: ${r.btn}`)
  }
  if (snap.emptyBlock.length) console.log('  空态文案：' + snap.emptyBlock.join(' | '))
}

console.log('\n=== 3. 整页截图 ===')
await shot('home-verified.png')

/* ---------- 日志 ---------- */

console.log('\n=== 4. 页面日志 ===')
const bad = logs.filter((l) => /exception|\.error|\.warn/.test(l))
if (!bad.length) console.log('  （无 error / warning）')
else for (const l of bad) console.log('  ' + l)

ws.close()
process.exit(0)
