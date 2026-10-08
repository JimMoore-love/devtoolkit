/**
 * 连上 WebView2 的远程调试端口，把页面的真实状态和错误取回来。
 *
 * 为什么需要它：Tauri 的 release 构建默认关掉 devtools，前端一旦白屏就完全是个黑箱 ——
 * 看不到 console、看不到异常、只能靠反复重建去二分定位（本项目为此耗过多轮）。
 * 给 WebView2 传 `--remote-debugging-port` 就能用 CDP 读，Node 21+ 自带 WebSocket，无需依赖。
 *
 * 用法：
 *   1. 带环境变量启动应用：
 *      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222" ./devtoolkit.exe
 *   2. node scripts/cdp-probe.mjs 9222
 */

const PORT = process.argv[2] || '9222'
const BASE = `http://127.0.0.1:${PORT}`
const RELOAD = process.argv.includes('--reload')

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

let list
try {
  list = await (await fetch(`${BASE}/json/list`)).json()
} catch (e) {
  console.log('无法连接调试端口：', e.message)
  console.log('请确认应用是用 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=' + PORT + ' 启动的')
  process.exit(2)
}

console.log('--- targets ---')
for (const t of list) console.log(` ${t.type.padEnd(8)} ${t.title || '(无标题)'}  ${t.url}`)

const page = list.find((t) => t.type === 'page') || list[0]
if (!page?.webSocketDebuggerUrl) {
  console.log('没有可调试的 page target')
  process.exit(3)
}

const ws = new WebSocket(page.webSocketDebuggerUrl)
const pending = new Map()
let seq = 0

function send(method, params = {}) {
  const id = ++seq
  ws.send(JSON.stringify({ id, method, params }))
  return new Promise((resolve) => pending.set(id, resolve))
}

const seen = []

ws.addEventListener('message', (ev) => {
  let msg
  try {
    msg = JSON.parse(ev.data)
  } catch {
    return
  }
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg)
    pending.delete(msg.id)
    return
  }
  if (msg.method === 'Runtime.consoleAPICalled') {
    const text = (msg.params.args || [])
      .map((a) => a.value ?? a.description ?? a.unserializableValue ?? '')
      .join(' ')
    seen.push(`[console.${msg.params.type}] ${text}`)
  } else if (msg.method === 'Runtime.exceptionThrown') {
    const d = msg.params.exceptionDetails || {}
    seen.push(
      `[exception] ${d.text || ''} ${d.exception?.description || ''} @${d.url || '?'}:${d.lineNumber ?? '?'}`
    )
  } else if (msg.method === 'Log.entryAdded') {
    const e = msg.params.entry || {}
    seen.push(`[log.${e.level}] ${e.text || ''} ${e.url ? '@' + e.url : ''}`)
  }
})

await new Promise((r, j) => {
  ws.addEventListener('open', r)
  ws.addEventListener('error', j)
})

await send('Runtime.enable')
await send('Log.enable')
await send('Page.enable')

if (RELOAD) {
  console.log('--- 重新加载页面以捕获加载期错误 ---')
  await send('Page.reload', { ignoreCache: true })
  await sleep(5000)
}

const probe = await send('Runtime.evaluate', {
  expression: `JSON.stringify({
    url: location.href,
    title: document.title,
    appExists: !!document.getElementById('app'),
    appInnerLen: (document.getElementById('app')?.innerHTML || '').length,
    appInnerHead: (document.getElementById('app')?.innerHTML || '').slice(0, 260),
    bodyLen: document.body.innerHTML.length,
    scripts: [...document.querySelectorAll('script')].map(s => s.src || '(inline)'),
    links: [...document.querySelectorAll('link')].map(l => l.href),
    navCount: document.querySelectorAll('.nav-item').length,
    sidebar: !!document.querySelector('.sidebar'),
  })`,
  returnByValue: true,
})

console.log('--- DOM 探针 ---')
const val = probe.result?.result?.value
if (val) {
  try {
    console.log(JSON.stringify(JSON.parse(val), null, 2))
  } catch {
    console.log(val)
  }
} else {
  console.log('evaluate 失败：', JSON.stringify(probe).slice(0, 500))
}

console.log('--- 错误与日志 ---')
if (!seen.length) console.log('（无）')
for (const line of seen.slice(0, 40)) console.log(line)

ws.close()
process.exit(0)
