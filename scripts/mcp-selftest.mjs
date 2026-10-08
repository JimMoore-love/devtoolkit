// DevToolkit MCP 端到端自检
// 直接以 stdio 方式拉起 devtoolkit-mcp，跑一遍真实握手与工具调用，验证链路可用。
// 用法： node scripts/mcp-selftest.mjs [--keep]
import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { ensureGui } from './ensure-gui.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const exe = resolve(root, 'src-tauri/target/release/devtoolkit-mcp.exe')

if (!existsSync(exe)) {
  console.error(`✖ 未找到 MCP 可执行文件：${exe}\n  先执行 cargo build --release`)
  process.exit(1)
}

try {
  const port = await ensureGui()
  console.log(`· DevToolkit 控制口就绪：127.0.0.1:${port}\n`)
} catch (e) {
  console.error(`✖ ${e.message}`)
  process.exit(1)
}

const child = spawn(exe, [], { stdio: ['pipe', 'pipe', 'pipe'] })

let buf = ''
let seq = 0
const pending = new Map()
let stderrTail = []

child.stderr.on('data', (d) => {
  const s = d.toString('utf8')
  stderrTail.push(s.trim())
  if (stderrTail.length > 20) stderrTail.shift()
})

child.stdout.on('data', (d) => {
  buf += d.toString('utf8')
  let i
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i).trim()
    buf = buf.slice(i + 1)
    if (!line) continue
    let msg
    try {
      msg = JSON.parse(line)
    } catch {
      console.error('✖ 响应不是合法 JSON:', line.slice(0, 200))
      continue
    }
    if (msg.id != null && pending.has(msg.id)) {
      const done = pending.get(msg.id)
      pending.delete(msg.id)
      done(msg)
    }
  }
})

function rpc(method, params, timeoutMs = 90000) {
  const id = ++seq
  return new Promise((res, rej) => {
    pending.set(id, res)
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n')
    setTimeout(() => {
      if (pending.has(id)) {
        pending.delete(id)
        rej(new Error(`超时：${method}`))
      }
    }, timeoutMs)
  })
}

function notify(method, params) {
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method, params }) + '\n')
}

let pass = 0
let fail = 0
function check(name, cond, detail = '') {
  if (cond) {
    pass++
    console.log(`✔ ${name}${detail ? ' — ' + detail : ''}`)
  } else {
    fail++
    console.log(`✖ ${name}${detail ? ' — ' + detail : ''}`)
  }
}

function textOf(resp) {
  return resp?.result?.content?.[0]?.text ?? ''
}

try {
  const init = await rpc('initialize', {
    protocolVersion: '2024-11-05',
    capabilities: {},
    clientInfo: { name: 'mcp-selftest', version: '1' },
  })
  check('initialize 返回 serverInfo', init.result?.serverInfo?.name === 'devtoolkit',
    JSON.stringify(init.result?.serverInfo))
  check('initialize 回传 capabilities.tools', !!init.result?.capabilities?.tools)
  notify('notifications/initialized', {})

  const list = await rpc('tools/list', {})
  const names = (list.result?.tools || []).map((t) => t.name)
  check('tools/list 返回工具', names.length >= 13, `${names.length} 个：${names.join(', ')}`)

  const status = await rpc('tools/call', { name: 'devtoolkit_status', arguments: {} })
  check('devtoolkit_status 可用', status.result?.isError === false)
  check('devtoolkit_status 报告 running=true', JSON.parse(textOf(status) || '{}').running === true)
  console.log('   ' + textOf(status).split('\n').slice(0, 12).join('\n   '))

  const running = await rpc('tools/call', { name: 'list_running', arguments: {} })
  check('list_running 可用', running.result?.isError === false)
  const rt = JSON.parse(textOf(running) || '{}')
  console.log(
    `   运行中项目 ${rt.total} 个：` +
      (rt.running || [])
        .map((r) => `${r.name}[${r.source}${r.external ? '/外部' : ''} pid=${(r.pids || []).join('|')}]`)
        .join(', ')
  )

  const cp = await rpc('tools/call', { name: 'check_ports', arguments: { ports: '8000,9528' } })
  check('check_ports 可用', cp.result?.isError === false)
  console.log('   ' + textOf(cp).replace(/\n\s*/g, ' ').slice(0, 300))

  const projects = await rpc('tools/call', { name: 'list_projects', arguments: {} })
  const pj = JSON.parse(textOf(projects) || '{}')
  check('list_projects 可用', Array.isArray(pj.projects), `${pj.total} 个项目`)

  const badLog = await rpc('tools/call', { name: 'get_task_log', arguments: { task_id: '不存在的任务' } })
  check('get_task_log 对未知任务不报错', badLog.result?.isError === false,
    textOf(badLog).replace(/\n\s*/g, ' ').slice(0, 120))

  const badTool = await rpc('tools/call', { name: 'no_such_tool', arguments: {} })
  check('未知工具返回 isError', badTool.result?.isError === true, textOf(badTool).slice(0, 60))

  const badRpc = await rpc('no/such/method', {})
  check('未知方法返回 -32601', badRpc.error?.code === -32601)
} catch (e) {
  fail++
  console.error(`✖ 自检中断：${e.message}`)
} finally {
  if (!process.argv.includes('--keep')) child.kill()
  console.log(`\n结果：${pass} 通过 / ${fail} 失败`)
  if (stderrTail.length) console.log('MCP stderr:\n  ' + stderrTail.join('\n  '))
  process.exit(fail ? 1 : 0)
}
