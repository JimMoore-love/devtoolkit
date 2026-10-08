/**
 * MCP 协议层测试（负向为主）。
 *
 * 与 mcp-selftest / mcp-write-test 的分工：
 *   - mcp-selftest     ：工具能不能用（正向链路）
 *   - mcp-write-test   ：写操作有没有副作用之外的意外
 *   - 本脚本           ：**协议本身在收到坏输入时是否还站得住**
 *     （错 token、超长报文、未知命令、参数类型错、非法 JSON、stdout 纯净度）
 *
 * 这些场景平时不会遇到，但一旦出现就表现为"AI 那边毫无反应"或"客户端直接断开"，
 * 排查成本极高，所以必须用脚本钉死。
 *
 * 用法：node scripts/mcp-protocol-test.mjs
 */
import { spawn } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import net from 'node:net'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { ensureGui, controlPort, configPath } from './ensure-gui.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const mcpExe = resolve(root, 'src-tauri/target/release/devtoolkit-mcp.exe')

let pass = 0
let fail = 0
let skip = 0

function ok(name, detail = '') {
  pass++
  console.log(`  ✔ ${name}${detail ? '  — ' + detail : ''}`)
}
function bad(name, detail = '') {
  fail++
  console.log(`  ✖ ${name}${detail ? '  — ' + detail : ''}`)
}
function skipIt(name, why) {
  skip++
  console.log(`  ⊘ ${name}  — 跳过：${why}`)
}
function check(name, cond, detail = '') {
  cond ? ok(name, detail) : bad(name, detail)
}

const flat = (t) => String(t || '').replace(/\s+/g, ' ')

// ---------------- MCP stdio 驱动 ----------------

if (!existsSync(mcpExe)) {
  console.error(`✖ 未找到 ${mcpExe}\n  先执行: cargo build --release`)
  process.exit(1)
}

const child = spawn(mcpExe, [], { stdio: ['pipe', 'pipe', 'pipe'] })
const stderrTail = []
child.stderr.on('data', (d) => {
  for (const l of d.toString().split('\n')) {
    if (l.trim()) stderrTail.push(l.trim())
  }
  if (stderrTail.length > 20) stderrTail.shift()
})

let outBuf = ''
const rawLines = []
const badLines = []
const waiters = []

child.stdout.on('data', (chunk) => {
  outBuf += chunk.toString('utf8')
  let idx
  while ((idx = outBuf.indexOf('\n')) >= 0) {
    const raw = outBuf.slice(0, idx).trim()
    outBuf = outBuf.slice(idx + 1)
    if (!raw) continue
    rawLines.push(raw)
    let msg
    try {
      msg = JSON.parse(raw)
    } catch {
      // stdout 混入非 JSON 是致命的：MCP 客户端会直接判定服务器异常
      badLines.push(raw)
      continue
    }
    const w = waiters.shift()
    if (w) w(msg)
  }
})

let seq = 0
function rpc(obj, timeoutMs = 20000) {
  const id = obj.id !== undefined ? obj.id : ++seq
  const payload = { jsonrpc: '2.0', id, ...obj, id }
  return new Promise((res, rej) => {
    const t = setTimeout(() => rej(new Error('等待响应超时')), timeoutMs)
    waiters.push((m) => {
      clearTimeout(t)
      res(m)
    })
    child.stdin.write(JSON.stringify(payload) + '\n')
  })
}

function notify(obj) {
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', ...obj }) + '\n')
}

function textOf(resp) {
  const c = resp?.result?.content
  if (!Array.isArray(c)) return ''
  return c.map((x) => x.text || '').join('\n')
}

// ---------------- 控制口原始客户端 ----------------

function controlRaw(port, payload) {
  return new Promise((res, rej) => {
    const s = net.connect({ host: '127.0.0.1', port })
    let buf = ''
    let settled = false
    const finish = (v) => {
      if (settled) return
      settled = true
      s.destroy()
      res(v)
    }
    s.setTimeout(20000)
    s.on('connect', () => s.write(payload))
    s.on('data', (d) => {
      buf += d.toString('utf8')
    })
    s.on('error', (e) => rej(e))
    s.on('timeout', () => finish(buf))
    s.on('close', () => finish(buf))
  })
}

function parseResp(buf) {
  try {
    return JSON.parse(buf.trim().split('\n')[0])
  } catch {
    return null
  }
}

// ---------------- 主流程 ----------------

async function main() {
  console.log('=== MCP 协议层测试 ===\n')

  // ---- 1. 握手 ----
  console.log('— 握手与基础方法 —')
  const init = await rpc({
    method: 'initialize',
    params: { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'test', version: '1' } },
  })
  check('initialize 返回 protocolVersion', !!init.result?.protocolVersion, init.result?.protocolVersion)
  check('serverInfo.name = devtoolkit', init.result?.serverInfo?.name === 'devtoolkit')
  check('能力声明包含 tools', !!init.result?.capabilities?.tools)

  // 通知不该产生响应：紧接着 ping 一次，若通知有回包，ping 的 waiter 会拿到错的那个
  notify({ method: 'notifications/initialized' })
  const pong = await rpc({ method: 'ping' })
  check(
    'notifications 不产生响应（未污染后续请求）',
    pong.id !== undefined && !pong.error && pong.result !== undefined,
    `id=${pong.id}`
  )

  // ---- 2. tools/list ----
  console.log('\n— 工具清单 —')
  const listed = await rpc({ method: 'tools/list' })
  const tools = listed.result?.tools || []
  check('tools/list 返回非空数组', tools.length > 0, `${tools.length} 个工具`)

  const names = tools.map((t) => t.name)
  check('工具名唯一', new Set(names).size === names.length)
  const malformed = tools.filter(
    (t) => !t.name || !t.description || t.inputSchema?.type !== 'object'
  )
  check('每个工具都有 name/description/inputSchema(object)', malformed.length === 0, JSON.stringify(malformed.map((t) => t.name)))
  const badRequired = []
  for (const t of tools) {
    const props = t.inputSchema?.properties || {}
    for (const r of t.inputSchema?.required || []) {
      if (!(r in props)) badRequired.push(`${t.name}.${r}`)
    }
  }
  check('required 字段都在 properties 中定义', badRequired.length === 0, badRequired.join(', '))

  // ---- 3. 确认 GUI 可用（tools/call 都要经控制口，先确保它在） ----
  console.log('\n— DevToolkit 可用性 —')
  let port = null
  try {
    port = await ensureGui()
    ok('DevToolkit 控制口可用', `127.0.0.1:${port}`)
  } catch (e) {
    port = null
    console.log(`  ⊘ DevToolkit 不可用：${e.message}`)
    console.log('    依赖控制口的用例将跳过（这不算失败，但覆盖率会下降）')
  }

  // ---- 4. 坏输入 ----
  console.log('\n— 坏输入容错 —')
  const unknownMethod = await rpc({ method: 'definitely/not/a/method' })
  check('未知 method 返回 JSON-RPC 错误 -32601', unknownMethod.error?.code === -32601, JSON.stringify(unknownMethod.error))

  // 非法 JSON：服务器应当忽略该行并继续服务，而不是退出
  child.stdin.write('{ this is not json\n')
  const afterBadJson = await rpc({ method: 'ping' })
  check('收到非法 JSON 后仍能继续服务', afterBadJson.result !== undefined && !afterBadJson.error)

  const unknownTool = await rpc({ method: 'tools/call', params: { name: 'no_such_tool', arguments: {} } })
  check('未知工具 → isError 且列出可用工具', unknownTool.result?.isError === true && /可用工具/.test(textOf(unknownTool)), flat(textOf(unknownTool)).slice(0, 100))

  if (!port) {
    skipIt('缺必填参数 → 明确报"缺少参数"', 'DevToolkit 未运行')
    skipIt('参数类型错 → 报错而不是崩溃', 'DevToolkit 未运行')
    skipIt('pid 非数字 → 报错', 'DevToolkit 未运行')
  } else {
    const missingArg = await rpc({ method: 'tools/call', params: { name: 'check_ports', arguments: {} } })
    check('缺必填参数 → 明确报"缺少参数"', missingArg.result?.isError === true && /缺少参数/.test(textOf(missingArg)), flat(textOf(missingArg)).slice(0, 100))

    const wrongType = await rpc({ method: 'tools/call', params: { name: 'check_ports', arguments: { ports: 123 } } })
    check('参数类型错 → 报错而不是崩溃', wrongType.result?.isError === true, flat(textOf(wrongType)).slice(0, 100))

    const badPid = await rpc({ method: 'tools/call', params: { name: 'kill_pid', arguments: { pid: 'abc' } } })
    check('pid 非数字 → 报错', badPid.result?.isError === true, flat(textOf(badPid)).slice(0, 100))
  }

  // ---- 5. GUI 相关的守卫 ----
  console.log('\n— 危险操作守卫 —')
  if (!port) {
    skipIt('kill_pid(0) 被拒绝', 'DevToolkit 未运行')
    skipIt('拒绝结束自身进程', 'DevToolkit 未运行')
  } else {
    const zero = await rpc({ method: 'tools/call', params: { name: 'kill_pid', arguments: { pid: 0 } } })
    check(
      'kill_pid(0) 被拒绝（系统关键进程）',
      /系统关键进程/.test(textOf(zero)),
      flat(textOf(zero)).slice(0, 120)
    )

    const st = await rpc({ method: 'tools/call', params: { name: 'devtoolkit_status', arguments: {} } })
    const status = JSON.parse(textOf(st))
    const pid = status.pid
    const self = await rpc({ method: 'tools/call', params: { name: 'kill_pid', arguments: { pid } } })
    check(
      '拒绝结束 DevToolkit 自身进程',
      /自身进程/.test(textOf(self)),
      flat(textOf(self)).slice(0, 120)
    )
    check('devtoolkit_status 报 running=true', status.running === true, `pid=${pid} port=${status.control_port}`)
  }

  // ---- 5. 控制口线协议（直连，绕过 MCP） ----
  console.log('\n— 控制口线协议 —')
  /** 控制口实际接受的全部命令名，由"未知命令"的报错里带回来 */
  let controlCmds = []
  if (!port) {
    skipIt('控制口负向用例', 'DevToolkit 未运行')
  } else {
    let token = ''
    try {
      token = JSON.parse(readFileSync(configPath(), 'utf8')).mcp_token || ''
    } catch {}

    const badToken = parseResp(await controlRaw(port, JSON.stringify({ token: 'wrong', cmd: 'status', args: {} }) + '\n'))
    check('错 token 被拒绝', badToken && badToken.ok === false && /token/.test(badToken.error), badToken?.error)

    const noToken = parseResp(await controlRaw(port, JSON.stringify({ cmd: 'status' }) + '\n'))
    check('缺 token 被拒绝', noToken && noToken.ok === false, noToken?.error)

    const unknownCmd = parseResp(await controlRaw(port, JSON.stringify({ token, cmd: 'no_such_cmd', args: {} }) + '\n'))
    check(
      '未知命令附带候选列表',
      unknownCmd && unknownCmd.ok === false && /未知命令/.test(unknownCmd.error) && /可用命令/.test(unknownCmd.error),
      flat(unknownCmd?.error).slice(0, 110)
    )
    controlCmds = (/(?:可用命令：|available:)(.+)$/.exec(unknownCmd?.error || '')?.[1] || '')
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean)

    const badJson = parseResp(await controlRaw(port, 'not json at all\n'))
    check('非法 JSON 请求被拒绝且不断连接', badJson && badJson.ok === false && /解析失败/.test(badJson.error), badJson?.error)

    const badProjects = parseResp(
      await controlRaw(port, JSON.stringify({ token, cmd: 'scan_ports', args: { projects: 'oops' } }) + '\n')
    )
    check('projects 类型错有明确提示', badProjects && badProjects.ok === false && /项目数组/.test(badProjects.error), badProjects?.error)

    const okCall = parseResp(await controlRaw(port, JSON.stringify({ token, cmd: 'status', args: {} }) + '\n'))
    check('合法请求正常返回', okCall && okCall.ok === true && !!okCall.data?.version, okCall?.data?.version)

    // 超过 MAX_LINE_BYTES(1MB) 的报文应被挡下，而不是把内存吃满
    const huge = JSON.stringify({ token, cmd: 'status', args: { pad: 'x'.repeat(1_100_000) } }) + '\n'
    const hugeResp = parseResp(await controlRaw(port, huge))
    check('超长报文被拒绝', hugeResp && hugeResp.ok === false && /过大/.test(hugeResp.error), hugeResp?.error)

    // 超长报文之后控制口必须还能正常工作（不能被一次异常请求打坏）
    const afterHuge = parseResp(await controlRaw(port, JSON.stringify({ token, cmd: 'status', args: {} }) + '\n'))
    check('超长报文后控制口仍可用', afterHuge && afterHuge.ok === true)
  }

  // ---- 6. 权限边界：哪些能力不该让 AI 拿到 ----
  //
  // 控制口是给 AI 客户端用的，它拿到的能力必须小于人在这台机器上的能力。
  // 这条边界一旦被悄悄放开（比如后来有人为了图方便把某个命令加进 Cmd::ALL），
  // 下游所有"保护"都会失效：AI 只要能把端口从保护名单里摘掉，保护就等于不存在。
  console.log('\n— 权限边界 —')
  const LOCAL_ONLY = {
    set_port_protected: 'AI 能改保护名单，等于保护形同虚设',
    save_projects: '托管项目列表是人工维护的配置，应经界面显式保存',
    exec_command: '等于把整台机器交给 AI，远超"托管项目"的授权范围',
    open_in_explorer: '本机资源管理器的打开动作没有必要开放给 AI',
  }
  if (!port || !controlCmds.length) {
    skipIt('敏感命令未暴露给控制口', port ? '未能取到控制口命令列表' : 'DevToolkit 未运行')
  } else {
    const leaked = Object.keys(LOCAL_ONLY).filter((n) => controlCmds.includes(n))
    check(
      '敏感命令未暴露给控制口',
      leaked.length === 0,
      leaked.length
        ? `意外可达：${leaked.map((n) => `${n}（${LOCAL_ONLY[n]}）`).join('；')}`
        : `已核对 ${controlCmds.length} 个控制口命令，敏感项均不在其中`
    )
    // 反过来也要确认列表真的取到了：空列表会让上面那条断言假通过
    check('控制口命令列表非空', controlCmds.length > 0, `${controlCmds.length} 个：${controlCmds.join(', ')}`)
  }

  // ---- 7. stdout 纯净度 ----
  console.log('\n— 输出通道 —')
  check(
    'stdout 上没有混入非 JSON 内容',
    badLines.length === 0,
    badLines.length ? badLines[0].slice(0, 120) : `${rawLines.length} 行全部可解析`
  )

  child.kill()

  console.log(`\n结果：${pass} 通过 / ${fail} 失败${skip ? ` / ${skip} 跳过` : ''}`)
  if (stderrTail.length) console.log('MCP stderr（末尾）:\n  ' + stderrTail.slice(-6).join('\n  '))
  process.exit(fail ? 1 : 0)
}

main().catch((e) => {
  console.error('✖ 测试异常终止: ' + (e && e.message ? e.message : e))
  try {
    child.kill()
  } catch {}
  process.exit(1)
})
