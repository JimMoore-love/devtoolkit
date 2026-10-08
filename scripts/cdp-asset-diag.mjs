/**
 * 定位「页面没挂载」的真正原因：资源到底取没取到。
 *
 * 现象是 #app 空、无异常、无 console —— 这种"静默不执行"最常见的原因是
 * <script type="module"> 请求失败：模块加载失败不会走 window.onerror，
 * 如果错误监听又加晚了，就什么痕迹都留不下。
 * 这里直接在页面里 fetch 那几个资源，看真实状态码和字节数。
 *
 *   node scripts/cdp-asset-diag.mjs [port] [--wait=秒]
 */
const PORT = process.argv[2] || '9222'
const BASE = `http://127.0.0.1:${PORT}`
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
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
      console.error(`连不上 ${PORT}：${e.message}`)
      process.exit(2)
    }
    await sleep(300)
  }
}

const page = list.find((t) => t.type === 'page') || list[0]
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
  if (m.method === 'Runtime.exceptionThrown') {
    logs.push('[exception] ' + (m.params.exceptionDetails?.exception?.description || m.params.exceptionDetails?.text))
  }
  if (m.method === 'Runtime.consoleAPICalled') {
    logs.push('[console.' + m.params.type + '] ' + (m.params.args || []).map((a) => a.value ?? a.description ?? '').join(' '))
  }
  if (m.method === 'Log.entryAdded') {
    const e = m.params.entry || {}
    logs.push('[log.' + e.level + '] ' + (e.text || '') + (e.url ? ' @' + e.url : ''))
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
    expression: `(async () => { try { return JSON.stringify(await (${expression})) } catch (e) { return JSON.stringify({ __err: String(e) }) } })()`,
    returnByValue: true,
    awaitPromise: true,
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

console.log('=== 页面与引用 ===')
console.log(
  JSON.stringify(
    await evaluate(`(() => ({
      url: location.href,
      origin: location.origin,
      appChildCount: document.getElementById('app')?.children.length ?? -1,
      tauriBridge: typeof window.__TAURI_INTERNALS__,
      scripts: [...document.querySelectorAll('script')].map(s => ({ src: s.src, type: s.type, cross: s.crossOrigin })),
      links: [...document.querySelectorAll('link')].map(l => ({ href: l.href, rel: l.rel })),
      perf: performance.getEntriesByType('resource').map(e => ({ name: e.name, status: e.responseStatus, size: e.transferSize }))
    }))()`),
    null,
    2
  )
)

console.log('\n=== 逐个取资源 ===')
const probes = await evaluate(`(async () => {
  const urls = [
    ...new Set([
      ...document.querySelectorAll('script[src]'),
    ].map(s => s.src)),
    ...document.querySelectorAll('link[rel=stylesheet]'),
  ]
  const out = []
  for (const u of urls) {
    const url = typeof u === 'string' ? u : u.href
    try {
      const r = await fetch(url, { cache: 'reload' })
      const txt = await r.text()
      out.push({ url, status: r.status, type: r.headers.get('content-type'), bytes: txt.length, head: txt.slice(0, 70) })
    } catch (e) {
      out.push({ url, error: String(e) })
    }
  }
  return out
})()`)
console.log(JSON.stringify(probes, null, 2))

console.log('\n=== 手动重新插入模块脚本（带 onload/onerror） ===')
const inject = await evaluate(`new Promise((resolve) => {
  const s = document.createElement('script')
  s.type = 'module'
  s.src = '/assets/' + (document.querySelector('script[src]')?.src.split('/').pop() || '')
  s.onload = () => resolve({ result: 'onload', appChildCount: document.getElementById('app')?.children.length ?? -1 })
  s.onerror = (e) => resolve({ result: 'onerror', message: String(e.message || 'no message') })
  document.head.appendChild(s)
  setTimeout(() => resolve({ result: 'timeout', appChildCount: document.getElementById('app')?.children.length ?? -1 }), 5000)
})`)
console.log(JSON.stringify(inject, null, 2))

async function evaluate2(expression) {
  const r = await send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  })
  return null
}
await evaluate2('1')

await sleep(500)
console.log('\n=== 事件日志 ===')
if (!logs.length) console.log('（无）')
for (const l of logs.slice(-40)) console.log('  ' + l)

ws.close()
process.exit(0)
