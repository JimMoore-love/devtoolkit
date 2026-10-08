/**
 * 列出 devtoolkit-mcp.exe 暴露给 AI 的工具清单。
 *
 * 存在意义：`tools/list` 完全由 MCP 进程本地应答，**不连控制口**，
 * 所以可以在 GUI 没启动（或正在运行旧版本）时独立核对"新构建到底暴露了哪些工具"。
 * 这是新增/改名工具后第一个该跑的检查——比启动整个应用快得多，也不会打扰在跑的服务。
 *
 * 用法：
 *   node scripts/mcp-tools-list.mjs [exe 路径]
 *   node scripts/mcp-tools-list.mjs --expect list_ports,check_ports
 */
import { spawn } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const argv = process.argv.slice(2)

const expectIdx = argv.indexOf('--expect')
const expect = expectIdx >= 0 ? (argv[expectIdx + 1] || '').split(',').filter(Boolean) : []
const exeArg = argv.find((a, i) => !a.startsWith('--') && i !== expectIdx + 1)
const exe =
  exeArg ||
  path.join(ROOT, 'src-tauri', 'target', 'release', 'devtoolkit-mcp.exe')

const TIMEOUT = 8000

// 明确关掉自动拉起：本检查只问本地清单，不该因为 GUI 没开就去启动它
const child = spawn(exe, [], {
  env: { ...process.env, DEVTOOLKIT_NO_AUTOSTART: '1' },
  stdio: ['pipe', 'pipe', 'pipe'],
})

let buf = ''
let names = null
let err = ''

child.stdout.on('data', (d) => {
  buf += d.toString()
  const lines = buf.split('\n')
  buf = lines.pop() || ''
  for (const line of lines) {
    const t = line.trim()
    if (!t) continue
    let msg
    try {
      msg = JSON.parse(t)
    } catch {
      // MCP 走 stdio，任何非 JSON 输出都是噪音（日志混进来会污染协议）
      err += `非 JSON 输出：${t.slice(0, 200)}\n`
      continue
    }
    if (msg.id === 2) {
      names = (msg.result?.tools || []).map((x) => x.name)
    }
  }
})

child.stderr.on('data', (d) => {
  err += d.toString()
})

const send = (o) => child.stdin.write(JSON.stringify(o) + '\n')

send({
  jsonrpc: '2.0',
  id: 1,
  method: 'initialize',
  params: {
    protocolVersion: '2024-11-05',
    capabilities: {},
    clientInfo: { name: 'mcp-tools-list', version: '1.0.0' },
  },
})
setTimeout(() => send({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }), 250)

const done = setTimeout(() => {
  console.log('✘ 超时：没等到 tools/list 应答')
  if (err) console.log('  进程输出：', err.slice(0, 400))
  child.kill()
  process.exit(1)
}, TIMEOUT)

const tick = setInterval(() => {
  if (!names) return
  clearInterval(tick)
  clearTimeout(done)
  console.log(`生效的二进制：${exe}`)
  console.log(`工具总数：${names.length}`)
  console.log(names.join(', '))

  const missing = expect.filter((n) => !names.includes(n))
  if (names.length === 0) {
    console.log('✘ 工具列表为空')
    child.kill()
    process.exit(1)
  }
  if (missing.length) {
    console.log(`✘ 缺少期望的工具：${missing.join(', ')}`)
    child.kill()
    process.exit(1)
  }
  if (err.trim()) console.log(`⚠ 进程有额外输出：\n${err.trim().slice(0, 400)}`)
  console.log(expect.length ? '✔ 期望的工具全部存在' : '✔ 清单读取成功')
  child.kill()
  process.exit(0)
}, 120)
