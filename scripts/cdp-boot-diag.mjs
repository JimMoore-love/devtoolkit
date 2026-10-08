/**
 * 抓「启动期」错误 —— 页面挂载失败时必须用这个，别的都是猜。
 *
 * 为什么之前的探针看不到错误：Runtime.enable 是在页面已经加载完之后才开的，
 * 加载期抛的异常早就过去了，收不到。正确做法是用
 * Page.addScriptToEvaluateOnNewDocument 在**页面脚本之前**装一个收集器，
 * 然后重载，再读收集器。
 *
 *   node scripts/cdp-boot-diag.mjs [port] [--wait=秒]
 */
const PORT = process.argv[2] || '9222'
const BASE = `http://127.0.0.1:${PORT}`
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const WAIT_MS = (() => {
  const a = process.argv.find((x) => x.startsWith('--wait='))
  return a ? Number(a.slice(7)) * 1000 : 15000
})()

let list
const deadline = Date.now() + WAIT_MS
for (;;) {
  try {
    list = await (await fetch(`${BASE}/json/list`)).json()
    break
  } catch (e) {
    if (Date.now() > deadline) {
      console.error(`连不上 ${PORT}：${e.message}`)
      process.exit(2)
    }
    await sleep(300)
  }
}

const page =
  list.find((t) => t.type === 'page' && /tauri\.localhost|tauri:\/\//.test(t.url || '')) ||
  list.find((t) => t.type === 'page') ||
  list[0]
console.log(`目标：${page.url}`)

const ws = new WebSocket(page.webSocketDebuggerUrl)
const pending = new Map()
let seq = 0
const events = []

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
  if (m.method === 'Runtime.exceptionThrown') {
    const d = m.params.exceptionDetails || {}
    events.push(`[exception] ${d.text || ''} ${d.exception?.description || ''} @${d.url || '?'}:${d.lineNumber ?? '?'}`)
  } else if (m.method === 'Runtime.consoleAPICalled') {
    events.push(`[console.${m.params.type}] ` + (m.params.args || []).map((a) => a.value ?? a.description ?? '').join(' '))
  } else if (m.method === 'Log.entryAdded') {
    const e = m.params.entry || {}
    events.push(`[log.${e.level}] ${e.text || ''} ${e.url ? '@' + e.url : ''}`)
  }
})

await new Promise((res, rej) => {
  ws.addEventListener('open', res)
  ws.addEventListener('error', rej)
})

function send(method, params = {}, timeoutMs = 6000) {
  const id = ++seq
  ws.send(JSON.stringify({ id, method, params }))
  return new Promise((res) => {
    const t = setTimeout(() => {
      pending.delete(id)
      res({ __timeout: true })
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
  })
  if (r.__timeout) return { __err: 'CDP 超时' }
  const raw = r.result?.result?.value
  if (raw == null) return { __err: JSON.stringify(r).slice(0, 200) }
  try {
    return JSON.parse(raw)
  } catch {
    return { __raw: String(raw).slice(0, 200) }
  }
}

await send('Runtime.enable')
await send('Log.enable')
await send('Page.enable')

// 装收集器：必须是 addScriptToEvaluateOnNewDocument，才能在页面自己的
// 模块脚本之前执行；用捕获阶段才能收到 <script> 的加载失败事件。
const collector = `
window.__boot = { errors: [], loaded: [] };
window.addEventListener('error', function (e) {
  var t = e.target;
  if (t && t !== window && (t.tagName === 'SCRIPT' || t.tagName === 'LINK')) {
    window.__boot.errors.push('资源加载失败: ' + t.tagName + ' ' + (t.src || t.href));
  } else {
    window.__boot.errors.push('error: ' + e.message + ' @' + e.filename + ':' + e.lineno + ':' + e.colno);
  }
}, true);
window.addEventListener('unhandledrejection', function (e) {
  var r = e.reason;
  window.__boot.errors.push('rejection: ' + ((r && (r.stack || r.message)) || String(r)));
});
window.addEventListener('DOMContentLoaded', function () {
  window.__boot.loaded.push('DOMContentLoaded app=' + (document.getElementById('app') ? document.getElementById('app').children.length : 'no-app'));
});
`
const add = await send('Page.addScriptToEvaluateOnNewDocument', { source: collector })
console.log('收集器已装载：' + JSON.stringify(add.result || add))

console.log('\n=== 重载页面 ===')
await send('Page.reload', { ignoreCache: true }, 10000)

for (let i = 1; i <= 12; i++) {
  await sleep(1000)
  const st = await evaluate(`({
    readyState: document.readyState,
    url: location.href,
    appChildCount: document.getElementById('app') ? document.getElementById('app').children.length : -1,
    navCount: document.querySelectorAll('.nav-item').length,
    bootErrors: (window.__boot && window.__boot.errors) || [],
    bootLoaded: (window.__boot && window.__boot.loaded) || [],
    hasBridge: typeof window.__TAURI_INTERNALS__,
    scripts: [...document.querySelectorAll('script[src]')].map(function (s) { return s.src.replace(/^.*\\//, '') }),
  })`)
  console.log(`[${i}s] ${JSON.stringify(st)}`)
  if (st && st.navCount > 0) {
    console.log('  → 已挂载，结束等待')
    break
  }
}

console.log('\n=== CDP 事件 ===')
if (!events.length) console.log('（无）')
for (const e of events.slice(-40)) console.log('  ' + e)

ws.close()
process.exit(0)
