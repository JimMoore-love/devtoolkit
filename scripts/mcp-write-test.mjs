// DevToolkit MCP 写操作链路自检
//
// 目的：验证 AI 侧的「启动 / 停止 / 读日志 / 审计」是否真的打通，而不是只读接口能跑。
//
// 做法：临时往配置里注入一个不占端口、可随时杀掉的测试项目，走完整流程后**无论成败都还原配置**。
// 全程不碰用户已有的项目；对已占用端口的真实项目只做"应该被拒绝"的负向验证。
//
// 用法： node scripts/mcp-write-test.mjs
import { spawn } from 'node:child_process'
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { tmpdir } from 'node:os'
import { ensureGui } from './ensure-gui.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const mcpExe = resolve(root, 'src-tauri/target/release/devtoolkit-mcp.exe')
const cfgPath = join(process.env.APPDATA, 'cn.devtoolkit.app', 'devtoolkit.json')
const backupPath = cfgPath + '.mcp-test-backup'

const TEST_ID = '__mcp_selftest__'
const MARKER = 'MCP_SELFTEST_ALIVE'

if (!existsSync(mcpExe)) {
  console.error(`✖ 未找到 ${mcpExe}`)
  process.exit(1)
}
if (!existsSync(cfgPath)) {
  console.error(`✖ 未找到配置 ${cfgPath}`)
  process.exit(1)
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

function readCfg() {
  return JSON.parse(readFileSync(cfgPath, 'utf8'))
}
function writeCfg(c) {
  writeFileSync(cfgPath, JSON.stringify(c, null, 2), 'utf8')
}

copyFileSync(cfgPath, backupPath)

try {
  const port = await ensureGui()
  console.log(`· DevToolkit 控制口就绪：127.0.0.1:${port}\n`)
} catch (e) {
  console.error(`✖ ${e.message}`)
  process.exit(1)
}

const child = spawn(mcpExe, [], { stdio: ['pipe', 'pipe', 'pipe'] })
let buf = ''
let seq = 0
const pending = new Map()
const stderrTail = []
child.stderr.on('data', (d) => {
  stderrTail.push(d.toString('utf8').trim())
  if (stderrTail.length > 25) stderrTail.shift()
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
      continue
    }
    if (msg.id != null && pending.has(msg.id)) {
      const done = pending.get(msg.id)
      pending.delete(msg.id)
      done(msg)
    }
  }
})

function rpc(method, params, timeoutMs = 120000) {
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

async function call(name, args = {}) {
  const r = await rpc('tools/call', { name, arguments: args })
  return { isError: r.result?.isError === true, text: r.result?.content?.[0]?.text ?? '' }
}
const parse = (t) => {
  try {
    return JSON.parse(t)
  } catch {
    return null
  }
}
const flat = (t) => t.replace(/\s+/g, ' ').trim()

try {
  await rpc('initialize', {
    protocolVersion: '2024-11-05',
    capabilities: {},
    clientInfo: { name: 'mcp-write-test', version: '1' },
  })

  // ---- 取 GUI 自身 pid，后面用于验证"拒绝结束自身进程" ----
  const status0 = parse((await call('devtoolkit_status')).text)
  const guiPid = status0?.pid
  check('devtoolkit_status 报告 running=true', status0?.running === true, `pid=${guiPid}`)

  // ---- 注入测试项目（无端口声明，避免与真实服务冲突） ----
  // cwd 必须用一个干净的空目录：目录反查是"进程目录在项目目录之下"即命中，
  // 若用仓库根目录，DevToolkit 自己的 exe 路径就会命中，测出来的不是产品行为
  const workDir = join(tmpdir(), 'devtoolkit-mcp-selftest')
  mkdirSync(workDir, { recursive: true })
  const cfg = readCfg()
  cfg.projects = cfg.projects.filter((p) => p.id !== TEST_ID)
  cfg.projects.push({
    id: TEST_ID,
    name: TEST_ID,
    command: `echo ${MARKER} && ping -n 300 127.0.0.1 > nul`,
    cwd: workDir,
    color: '#a78bfa',
    note: 'MCP 写操作自检用，脚本结束后会被移除',
    kind: 'command',
    ports: '',
    group: 'selftest',
  })
  writeCfg(cfg)

  // ---- 启动 ----
  const start = await call('start_project', { id: TEST_ID })
  const started = parse(start.text)
  check('start_project 成功启动测试项目', !start.isError && !!started?.id, `task_id=${started?.id} pid=${started?.pid}`)
  const taskId = started?.id

  // ---- 读日志 ----
  await new Promise((r) => setTimeout(r, 1500))
  if (taskId) {
    const log = await call('get_task_log', { task_id: taskId, limit: 50 })
    const lj = parse(log.text)
    check(
      'get_task_log 读到进程输出',
      !log.isError && (lj?.lines || []).some((l) => l.includes(MARKER)),
      flat(log.text).slice(0, 160)
    )
  }

  // ---- 任务列表 ----
  const tasks = await call('list_tasks')
  const tj = parse(tasks.text)
  check('list_tasks 含新任务', (tj?.tasks || []).some((t) => t.task_id === taskId), `共 ${tj?.total} 个`)

  // ---- 重复启动应被拒绝 ----
  const again = await call('start_project', { id: TEST_ID })
  const againJ = parse(again.text)
  check('重复 start_project 不重复启动', againJ?.started === false || again.isError, flat(again.text).slice(0, 120))

  // ---- 停止 ----
  const stop = await call('stop_project', { id: TEST_ID })
  check('stop_project 成功停止', !stop.isError && parse(stop.text)?.stopped === true, flat(stop.text).slice(0, 120))
  await new Promise((r) => setTimeout(r, 1200))
  const after = await call('list_tasks')
  const afterJ = parse(after.text)
  check('停止后任务已消失', !(afterJ?.tasks || []).some((t) => t.task_id === taskId))

  // ---- 负向：真实项目端口占用时应拒绝而不是硬撞 ----
  // 只在"它此刻确实在运行"时才测，否则 start_project 会真的把它启动起来，属于对用户环境的副作用
  const running = parse((await call('list_running')).text)
  const runningIds = new Set((running?.running || []).map((r) => r.id))
  const real = readCfg().projects.find((p) => runningIds.has(p.id))
  if (real) {
    const conflict = await call('start_project', { id: real.id })
    const msg = flat(conflict.text)
    const occupied = /端口冲突|外部进程占用|已在运行/.test(msg)
    check(`对运行中的真实项目「${real.name}」不误启动`, conflict.isError || occupied, msg.slice(0, 160))

    // ---- 负向：外部项目不能 stop ----
    const stopExt = await call('stop_project', { id: real.id })
    check('外部启动的项目拒绝 stop_project', stopExt.isError, flat(stopExt.text).slice(0, 140))
  } else {
    console.log('⏭ 当前没有运行中的真实项目，跳过「不误启动 / 拒绝 stop」两项负向验证')
  }

  // ---- 负向：不存在的项目 ----
  const ghost = await call('start_project', { name: '绝对不存在的项目名xyz' })
  check('不存在的项目名报错清晰', ghost.isError, flat(ghost.text).slice(0, 100))

  // ---- 负向：不允许 AI 结束 DevToolkit 自身 ----
  if (guiPid) {
    const selfKill = await call('kill_pid', { pid: guiPid })
    check('拒绝 kill DevToolkit 自身', parse(selfKill.text)?.ok === false, flat(selfKill.text).slice(0, 100))
  }

  // ---- 审计 ----
  const audit = readCfg().mcp_audit || []
  check(
    'AI 写操作已落审计',
    audit.length >= 2 && audit.some((a) => a.action === 'start') && audit.some((a) => a.action === 'stop'),
    audit
      .slice(0, 6)
      .map((a) => `${a.action}:${a.target}=${a.ok ? 'ok' : 'fail'}`)
      .join(', ')
  )
} catch (e) {
  fail++
  console.error(`✖ 自检中断：${e.message}`)
} finally {
  // 还原配置：恢复原始内容，仅保留 token/端口与本轮测试产生的审计
  try {
    const original = JSON.parse(readFileSync(backupPath, 'utf8'))
    const now = readCfg()
    original.mcp_token = now.mcp_token
    original.mcp_port = now.mcp_port
    original.mcp_audit = (now.mcp_audit || []).filter((a) => (a.target || '').includes(TEST_ID))
    writeCfg(original)
    console.log('↩ 配置已还原（保留 token/端口，仅留测试相关审计）')
  } catch (e) {
    console.error('✖ 还原配置失败: ' + e.message)
  }
  child.kill()
  console.log(`\n结果：${pass} 通过 / ${fail} 失败`)
  if (stderrTail.length) console.log('MCP stderr:\n  ' + stderrTail.join('\n  '))
  process.exit(fail ? 1 : 0)
}
