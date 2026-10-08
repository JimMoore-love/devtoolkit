/**
 * 通过 CDP 真机验证「MCP 管理」页面。
 *
 * 为什么不用合成鼠标点击 + PrintWindow 截图：
 *   1. 桌面被别的窗口铺满时，Chromium 的 CalculateNativeWinOcclusion 会停止渲染，
 *      PrintWindow 拿到的是全黑 —— 那测的是"窗口有没有被看见"，不是"页面画得对不对"。
 *   2. 从降采样截图上量坐标点鼠标，误差能到 20% 以上（本项目踩过）。
 *   CDP 直接问渲染进程要图，遮挡与否无关；点击走 Input 域，是按 DOM 坐标投递的真实输入。
 *
 * 用法（应用需带 --remote-debugging-port=9222 启动）：
 *   node scripts/mcp-verify.mjs [port]
 */

import fs from 'node:fs'
import path from 'node:path'

const PORT = process.argv[2] || '9222'
const BASE = `http://127.0.0.1:${PORT}`
const OUTDIR = path.resolve('.shots')
fs.mkdirSync(OUTDIR, { recursive: true })

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

/* ---------- 连接 ---------- */

// WebView2 的调试监听不保证一直在（实测：起来后几十秒会消失），
// 所以默认轮询等它出现，出现后立刻连着不放。
const WAIT_MS = (() => {
  const a = process.argv.find((x) => x.startsWith('--wait='))
  return a ? Number(a.slice(7)) * 1000 : 20000
})()

let list
const deadline = Date.now() + WAIT_MS
for (;;) {
  try {
    list = await (await fetch(`${BASE}/json/list`)).json()
    break
  } catch (e) {
    if (Date.now() > deadline) {
      console.error(`连不上调试端口 ${PORT}：${e.message}`)
      console.error('启动应用时需带 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=' + PORT)
      process.exit(2)
    }
    await sleep(400)
  }
}

// /json/list 里可能同时有 about:blank 和其它 target，挑真正的应用页面。
// 本地夹具（无头浏览器）走 127.0.0.1，真机走 tauri.localhost，两种都认。
const page =
  list.find((t) => t.type === 'page' && /tauri\.localhost|tauri:\/\/|127\.0\.0\.1:(4321|43\d\d)/.test(t.url || '')) ||
  list.find((t) => t.type === 'page' && !/^about:/.test(t.url || '')) ||
  list.find((t) => t.type === 'page') ||
  list[0]
console.log('--- targets ---')
for (const t of list) console.log(`  ${t.type.padEnd(8)} ${(t.title || '(无标题)').slice(0, 30)}  ${t.url}`)
console.log(`→ 使用：${page?.url}`)

if (!page?.webSocketDebuggerUrl) {
  console.error('没有可调试的 page target')
  process.exit(3)
}

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

// WebView2 的调试监听会中途消失（实测），没有超时就会永远挂在 await 上，
// 表现为 "Detected unsettled top-level await" 这种毫无信息的报错。
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

/** 按可见文字找元素，滚到可视区，然后用 Input 域投递真实鼠标事件。 */
async function clickByText(selector, text) {
  const rect = await evaluate(`
    (() => {
      const el = [...document.querySelectorAll(${JSON.stringify(selector)})]
        .find(e => (e.textContent || '').includes(${JSON.stringify(text)}) && e.offsetParent !== null)
      if (!el) return null
      el.scrollIntoView({ block: 'center' })
      const r = el.getBoundingClientRect()
      return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height,
               text: (el.textContent || '').trim().slice(0, 40) }
    })()
  `)
  if (!rect || rect.__err || rect.x == null) return null
  const p = { x: Math.round(rect.x), y: Math.round(rect.y), button: 'left', clickCount: 1 }
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: p.x, y: p.y })
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', ...p })
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...p })
  return rect
}

async function shot(name) {
  const r = await send('Page.captureScreenshot', { format: 'png', fromSurface: true })
  const data = r.result?.data
  if (!data) {
    console.error(`  截图失败：${JSON.stringify(r).slice(0, 200)}`)
    return null
  }
  const file = path.join(OUTDIR, name)
  fs.writeFileSync(file, Buffer.from(data, 'base64'))
  const kb = (Buffer.from(data, 'base64').length / 1024).toFixed(1)
  console.log(`  截图 → ${file} (${kb} KB)`)
  return file
}

/* ---------- 采集 ---------- */

const SNAPSHOT = `
(() => {
  const txt = (el) => (el?.textContent || '').replace(/\\s+/g, ' ').trim()
  const page = document.querySelector('.mcp-page')
  return {
    url: location.href,
    hash: location.hash,
    navActive: txt(document.querySelector('.nav-item.active')),
    navItems: [...document.querySelectorAll('.nav-item')].map(e => txt(e)),
    onMcpPage: !!page,
    pageErr: txt(document.querySelector('.mcp-banner')),
    sectionTitles: [...document.querySelectorAll('.mcp-page .card-title')].map(e => txt(e)),
    stats: [...document.querySelectorAll('.mcp-page .stat-card')].map(c => ({
      title: txt(c.querySelector('.card-title')),
      value: txt(c.querySelector('.stat-val')),
      note: txt(c.querySelector('.stat-note')),
    })),
    exe: txt(document.querySelector('.mcp-exe')),
    clients: [...document.querySelectorAll('.mcp-page .client')].map(c => ({
      name: txt(c.querySelector('.client-head strong')),
      status: txt(c.querySelector('.client-head .tag')),
      path: txt(c.querySelector('.client-path')),
      cmd: txt(c.querySelector('.client-cmd')),
      warn: txt(c.querySelector('.client-warn')),
      others: txt(c.querySelector('.client-others')),
      button: txt(c.querySelector('.client-actions .btn')),
    })),
    selftest: {
      ok: txt(document.querySelector('.st-ok')),
      bad: txt(document.querySelector('.st-bad')),
      note: txt(document.querySelector('.mcp-page .card:nth-of-type(5) .mcp-note')),
      toolRows: document.querySelectorAll('.tool-tbl tbody tr').length,
      tools: [...document.querySelectorAll('.tool-tbl tbody tr')].slice(0, 20).map(r => ({
        name: txt(r.cells[0]), desc: txt(r.cells[1]).slice(0, 60),
      })),
      elapsed: txt(document.querySelector('.mcp-actions .mcp-note.inline')),
    },
    audit: {
      head: [...document.querySelectorAll('.mcp-page .card-title')].map(txt).find(t => t.includes('审计')),
      rows: document.querySelectorAll('.tbl tbody tr').length,
      first: [...document.querySelectorAll('.mcp-page table.tbl tbody tr')].slice(0, 3)
        .map(r => [...r.cells].map(c => txt(c)).join(' | ')),
      empty: txt(document.querySelector('.mcp-page .empty p')),
    },
  }
})()
`

/* ---------- 执行 ---------- */

await send('Runtime.enable')
await send('Log.enable')
await send('Page.enable')

// 关键教训：调试连接常常比页面挂载先就绪。早期版本一连上就快照，
// 于是把「还没加载完」记成了「白屏」（#app 空 + 无异常），白排查了很久。
// 这里先轮询等 Vue 真正挂载，超时才算真有问题。
console.log('=== 1. 等页面挂载 ===')
let mounted = false
for (let i = 0; i < 40; i++) {
  const st = await evaluate(`JSON.stringify({
    url: location.href,
    navCount: document.querySelectorAll('.nav-item').length,
    appChildCount: document.getElementById('app')?.children.length ?? -1,
  })`)
  if (i === 0) console.log('  起始：' + JSON.stringify(st))
  if (st && !st.__err && st.navCount > 0) {
    mounted = true
    console.log(`  第 ${(i * 0.4).toFixed(1)}s 已挂载：导航 ${st.navCount} 项`)
    break
  }
  await sleep(400)
}

console.log('\n=== 1b. 初始 DOM 状态 ===')
let snap0 = await evaluate(`JSON.stringify({
  url: location.href,
  title: document.title,
  appChildCount: document.getElementById('app')?.children.length ?? -1,
  appInnerLen: (document.getElementById('app')?.innerHTML || '').length,
  navCount: document.querySelectorAll('.nav-item').length,
  navItems: [...document.querySelectorAll('.nav-item')].map(e => (e.textContent||'').replace(/\\s+/g,' ').trim()),
  bodyHead: document.body.innerHTML.replace(/\\s+/g, ' ').slice(0, 400),
  tauriBridge: typeof window.__TAURI_INTERNALS__,
})`)
if (typeof snap0 === 'string') {
  try {
    snap0 = JSON.parse(snap0)
  } catch {
    snap0 = { __raw: snap0 }
  }
}
console.log(JSON.stringify(snap0, null, 2))

if (!snap0.navCount) {
  console.log('\n!! 等了 16 秒仍没挂载 —— 这次是真的没起来，重载一次取证')
  await send('Page.reload', { ignoreCache: true }, 15000)
  await sleep(5000)
  const again = await evaluate(`JSON.stringify({
    appChildCount: document.getElementById('app')?.children.length ?? -1,
    navCount: document.querySelectorAll('.nav-item').length,
    scripts: [...document.querySelectorAll('script')].map(s => s.src),
  })`)
  console.log('重载后：' + JSON.stringify(again))
  console.log('\n--- 启动期错误与日志 ---')
  if (!logs.length) console.log('（无 console / exception 记录）')
  for (const l of logs.slice(-30)) console.log('  ' + l)
  await shot('mcp-0-blank.png')
  ws.close()
  process.exit(4)
}

console.log('\n=== 2. 打开「MCP 管理」 ===')
const nav = await clickByText('.nav-item', 'MCP 管理')
if (!nav) {
  console.error('侧栏里找不到「MCP 管理」入口 —— 导航没注册上')
  console.error('现有导航：' + (snap0.navItems || []).join(' / '))
  process.exit(4)
}
console.log(`  已点击导航（${nav.text}）`)
await sleep(2000)

let snap = await evaluate(SNAPSHOT)
console.log('\n=== 2. 页面结构 ===')
console.log(`  URL           : ${snap.url}`)
console.log(`  当前高亮导航  : ${snap.navActive}`)
console.log(`  导航项(${snap.navItems.length})    : ${snap.navItems.join(' / ')}`)
console.log(`  落在 MCP 页   : ${snap.onMcpPage ? '是' : '否 ← 问题'}`)
if (snap.pageErr) console.log(`  页面错误横幅  : ${snap.pageErr}`)
console.log(`  区块(${snap.sectionTitles.length})        :`)
for (const t of snap.sectionTitles) console.log(`    · ${t}`)

console.log('\n=== 3. 状态总览 ===')
for (const s of snap.stats) console.log(`  ${s.title.padEnd(12)} ${s.value.padEnd(18)} ${s.note}`)
console.log(`  MCP 服务行    : ${snap.exe}`)

console.log('\n=== 4. 客户端接入 ===')
if (!snap.clients.length) console.log('  （没有客户端卡片 ← 检测命令可能没返回）')
for (const c of snap.clients) {
  console.log(`  ${(c.name || '?').padEnd(14)} ${(c.status || '?').padEnd(10)} ${c.path}`)
  if (c.cmd) console.log(`       ${c.cmd}`)
  if (c.warn) console.log(`       ⚠ ${c.warn}`)
  if (c.others) console.log(`       ${c.others}`)
  if (c.button) console.log(`       按钮：${c.button}`)
}

await shot('mcp-1-overview.png')

console.log('\n=== 5. 连通自检 ===')
const st = await clickByText('.mcp-page .btn', '运行自检')
if (!st) {
  console.log('  找不到「运行自检」按钮')
} else {
  // 自检要起一个进程跑协议握手，轮询等结果而不是死等固定时间
  let out = null
  for (let i = 0; i < 30; i++) {
    await sleep(500)
    const s = await evaluate(SNAPSHOT)
    if (s.selftest?.ok || s.selftest?.bad) {
      out = s.selftest
      break
    }
  }
  if (!out) {
    console.log('  15 秒内没等到自检结果 ← 卡住了')
  } else if (out.ok) {
    console.log(`  ✓ ${out.ok}`)
    console.log(`  耗时 ${out.elapsed}`)
    console.log(`  工具清单（${out.toolRows} 个）：`)
    for (const t of out.tools) console.log(`    · ${t.name.padEnd(26)} ${t.desc}`)
  } else {
    console.log(`  ✗ ${out.bad}`)
    console.log(`  提示：${out.note}`)
  }
}

console.log('\n=== 6. AI 操作审计 ===')
console.log(`  卡片标题：${snap.audit.head}`)
if (snap.audit.empty) console.log(`  空状态：${snap.audit.empty}`)
console.log(`  表格行数：${snap.audit.rows}`)
for (const r of snap.audit.first) console.log(`    ${r}`)

await shot('mcp-2-selftest.png')

// 滚到底部再取一张，确认审计表渲染完整
await evaluate(`(() => {
  const c = document.querySelector('.mcp-page')
  const sc = [...document.querySelectorAll('*')].find(e => e.scrollHeight > e.clientHeight + 40 && getComputedStyle(e).overflowY !== 'visible')
  if (sc) sc.scrollTop = sc.scrollHeight
  window.scrollTo(0, document.body.scrollHeight)
  if (c) c.scrollIntoView({ block: 'end' })
  return true
})()`)
await sleep(600)
await shot('mcp-3-audit.png')

console.log('\n=== 7. 加载期错误 ===')
const errs = logs.filter((l) => !l.startsWith('[log.info]'))
if (!errs.length) console.log('  （无 error / warning）')
for (const l of errs.slice(0, 20)) console.log(`  ${l}`)

ws.close()
process.exit(0)
