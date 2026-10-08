/**
 * 无头 UI 夹具：把真实构建产物 dist/ 跑在普通浏览器里，用来验证界面渲染与交互。
 *
 * 为什么需要它：
 *   本机是锁屏/无 DWM 合成的环境，WebView2 的内容既抓不到（PrintWindow 全白）、
 *   CDP 也不稳定（连上后很快断）。而这些都不影响"页面画得对不对"这个问题 ——
 *   那纯粹是前端的事，用无头浏览器验证反而更干净、更可复现。
 *
 * 它到底验证什么、不验证什么（别把结论说过头）：
 *   ✔ 真实 dist 产物能不能挂载、五个区块渲染成什么样
 *   ✔ 点击"运行自检"后工具清单能不能正确渲染
 *   ✔ invoke 的入参名、返回值字段名是否与后端对齐（对不上这里就露馅）
 *   ✔ 自检这一条是真的：真去 spawn devtoolkit-mcp.exe 跑协议握手
 *   ✔ 审计流水是真的：直接读真实 devtoolkit.json
 *   ✘ 不验证 Tauri IPC 本身（这里用 HTTP 转发替代）、不验证窗口/系统集成
 *
 * 用法：
 *   node scripts/ui-harness.mjs            # 起服务，打印访问地址
 *   node scripts/ui-harness.mjs --port=4400
 */

import http from 'node:http'
import net from 'node:net'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { spawn } from 'node:child_process'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const DIST = join(ROOT, 'dist')
const HOME = os.homedir()
const CFG = join(HOME, 'AppData/Roaming/cn.devtoolkit.app/devtoolkit.json')
const MCP_EXE = join(ROOT, 'src-tauri/target/release/devtoolkit-mcp.exe')

const PORT = (() => {
  const a = process.argv.find((x) => x.startsWith('--port='))
  return a ? Number(a.slice(7)) : 4321
})()

/* ---------- 真实数据来源 ---------- */

function readCfg() {
  try {
    return JSON.parse(fs.readFileSync(CFG, 'utf8'))
  } catch {
    return {}
  }
}

/** 与 Rust 的 ClientSpec 对齐：名字、路径、可写性、提示都要一致 */
function knownClients() {
  return [
    { id: 'workbuddy', name: 'WorkBuddy', path: join(HOME, '.workbuddy', 'mcp.json'), writable: true, hint: '' },
    { id: 'cursor', name: 'Cursor', path: join(HOME, '.cursor', 'mcp.json'), writable: true, hint: '' },
    {
      id: 'claude',
      name: 'Claude Code',
      path: join(HOME, '.claude.json'),
      writable: false,
      hint: 'claude mcp add devtoolkit -- "<下面的 exe 路径>"',
    },
  ]
}

const norm = (p) => String(p || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase()

/** 与 Rust detect() 同语义：文件在不在 → 条目在不在 → 条目对不对 */
function detect(spec) {
  const out = {
    id: spec.id,
    name: spec.name,
    path: spec.path,
    writable: spec.writable,
    hint: spec.hint,
    exists: false,
    broken: false,
    registered: false,
    disabled: false,
    command: '',
    command_ok: false,
    others: [],
  }
  let raw
  try {
    raw = fs.readFileSync(spec.path, 'utf8')
  } catch {
    return out
  }
  out.exists = true

  let doc
  try {
    doc = JSON.parse(raw)
  } catch {
    out.broken = true
    return out
  }

  const servers = doc && typeof doc === 'object' ? doc.mcpServers : null
  if (servers && typeof servers === 'object') {
    out.others = Object.keys(servers).filter((k) => k !== 'devtoolkit')
    const entry = servers.devtoolkit
    if (entry && typeof entry === 'object') {
      out.registered = true
      out.disabled = entry.disabled === true
      out.command = typeof entry.command === 'string' ? entry.command : ''
      out.command_ok = norm(out.command) === norm(MCP_EXE)
    }
  } else if (doc && typeof doc === 'object' && doc.mcpServers && typeof doc.mcpServers !== 'object') {
    out.broken = true
  }

  // Claude Code 把 mcpServers 藏在按项目分身里，这里只做浅层探测，
  // 与 Rust 侧"结构复杂 → 只检测"的定位一致
  if (!out.registered && spec.id === 'claude') {
    const hit = JSON.stringify(doc).includes(norm(MCP_EXE)) || JSON.stringify(doc).includes('devtoolkit-mcp')
    if (hit) {
      out.registered = true
      out.command = '(嵌在项目级 mcpServers 中)'
      out.command_ok = true
    }
  }
  return out
}

/** 真的拉起 MCP 二进制跑一次协议握手（与 Rust selftest 同流程） */
function probe(timeoutSec = 15) {
  return new Promise((resolve) => {
    const t0 = Date.now()
    if (!fs.existsSync(MCP_EXE)) {
      resolve({
        ok: false,
        stage: 'binary',
        error: '没找到 devtoolkit-mcp 可执行文件，请先构建（cargo build --release 会一起产出）',
        elapsed_ms: 0,
        tools: [],
      })
      return
    }

    const child = spawn(MCP_EXE, [], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true })
    let buf = ''
    let server = ''
    let version = ''
    let protocol = ''
    let tools = []
    let done = false

    const finish = (v) => {
      if (done) return
      done = true
      clearTimeout(timer)
      try {
        child.kill()
      } catch {
        /* 已经退了 */
      }
      resolve({ ...v, elapsed_ms: Date.now() - t0, tools })
    }

    const timer = setTimeout(
      () => finish({ ok: false, stage: 'timeout', error: `自检超时（${timeoutSec}s 内没拿到完整响应）` }),
      timeoutSec * 1000
    )

    child.on('error', (e) => finish({ ok: false, stage: 'spawn', error: '无法启动进程: ' + e.message }))

    child.stdout.on('data', (d) => {
      buf += d.toString('utf8')
      let i
      while ((i = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, i).trim()
        buf = buf.slice(i + 1)
        if (!line) continue
        let m
        try {
          m = JSON.parse(line)
        } catch {
          continue
        }
        if (m.id === 1 && m.result) {
          server = m.result.serverInfo?.name || ''
          version = m.result.serverInfo?.version || ''
          protocol = m.result.protocolVersion || ''
          child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n')
          child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }) + '\n')
        } else if (m.id === 2) {
          if (m.error) {
            finish({ ok: false, stage: 'protocol', error: 'tools/list 返回错误：' + JSON.stringify(m.error), server, version, protocol })
          } else {
            tools = (m.result?.tools || []).map((t) => ({ name: t.name, description: t.description || '' }))
            if (!tools.length) {
              finish({ ok: false, stage: 'tools', error: '握手成功但工具清单为空 —— 服务端没有暴露任何工具', server, version, protocol })
            } else {
              finish({ ok: true, stage: 'ok', server, version, protocol, tool_count: tools.length })
            }
          }
        }
      }
    })

    child.stdin.write(
      JSON.stringify({
        jsonrpc: '2.0',
        id: 1,
        method: 'initialize',
        params: {
          protocolVersion: '2024-11-05',
          capabilities: {},
          clientInfo: { name: 'devtoolkit-gui', version: '1' },
        },
      }) + '\n'
    )
  })
}

/* ---------- 命令实现 ---------- */

/**
 * 调真实 GUI 控制口（回环 TCP + token），拿真实的运行态数据。
 *
 * 存在的意义：夹具验证的数据越假，"界面画对了"这个结论就越没意义 ——
 * 端口/运行态这类会决定分支渲染的数据必须是真的。GUI 没在跑时返回 null，
 * 由调用方退化成空数据（而不是抛错，否则整个页面挂不上）。
 */
function controlCall(cmd, args = {}, timeoutMs = 15000) {
  return new Promise((resolve) => {
    const cfg = readCfg()
    const sock = net.connect(cfg.mcp_port || 9527, '127.0.0.1')
    let buf = ''
    let settled = false
    const done = (v) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      try {
        sock.destroy()
      } catch (e) {
        /* 已经断了就算了 */
      }
      resolve(v)
    }
    const timer = setTimeout(() => done(null), timeoutMs)
    sock.on('connect', () => {
      sock.write(JSON.stringify({ token: cfg.mcp_token || '', cmd, args }) + '\n')
    })
    sock.on('data', (d) => {
      buf += d.toString()
      if (!buf.includes('\n')) return
      try {
        const r = JSON.parse(buf.split('\n')[0])
        done(r && r.ok ? r.data : null)
      } catch (e) {
        done(null)
      }
    })
    sock.on('error', () => done(null))
  })
}

async function invoke(cmd, args) {
  const cfg = readCfg()

  switch (cmd) {
    case 'sys_info':
      return {
        hostname: os.hostname(),
        os: `${os.type()} ${os.release()}`,
        cores: os.cpus().length,
        cpu_brand: os.cpus()[0]?.model || '',
      }
    case 'get_config':
      return cfg
    // 下面三条走真实控制口：首页的「运行中的项目」完全由 scan_ports.runtime 决定，
    // 喂假数据就只能验证"没崩"，验证不出"画对了"。
    case 'list_tasks': {
      const real = await controlCall('list_tasks')
      return (real && real.tasks) || []
    }
    case 'list_ports': {
      const real = await controlCall('list_ports')
      if (Array.isArray(real)) return real
      return (real && real.ports) || []
    }
    case 'scan_ports': {
      const real = await controlCall('scan_ports', { projects: cfg.projects || [] })
      return real || { ports: [], runtime: [] }
    }
    case 'check_ports':
      return []
    case 'list_projects':
      return cfg.projects || []

    case 'mcp_info':
      return {
        enabled: true,
        port: cfg.mcp_port || 9527,
        requests: 2,
        errors: 0,
        started_at: Math.floor(Date.now() / 1000) - 3600,
        token: cfg.mcp_token || '',
        config_path: CFG,
        mcp_exe: MCP_EXE,
        mcp_exe_found: fs.existsSync(MCP_EXE),
        audit_count: (cfg.mcp_audit || []).length,
        recent_audit: (cfg.mcp_audit || []).slice(-8),
        preferred_port: cfg.mcp_preferred_port || 0,
      }

    case 'mcp_clients_detect':
      return { mcp_exe: MCP_EXE, clients: knownClients().map(detect) }

    case 'mcp_audit_list': {
      const all = cfg.mcp_audit || []
      const n = Math.min(args?.limit ?? 500, 5000)
      const items = all.slice().reverse().slice(0, n)
      return { total: all.length, shown: items.length, items }
    }

    case 'mcp_selftest':
      return await probe()

    case 'mcp_audit_clear':
      return 0
    case 'mcp_control_set':
      return { enabled: !!args?.enabled, port: args?.enabled ? cfg.mcp_port || 9527 : 0 }
    case 'mcp_token_reset':
      return {
        token_len: 32,
        enabled: true,
        port: cfg.mcp_port || 9527,
        note: '令牌已更换。MCP 进程每次启动都从配置文件读取最新令牌，客户端配置里只存 exe 路径，无需改动。',
      }
    case 'mcp_port_set':
      return { requested: args?.port ?? 0, port: args?.port || 9527, shifted: false }
    case 'mcp_client_write':
      return { id: args?.id, name: args?.id, path: '', preserved: [] }

    default:
      // 未实现的命令返回空对象而不是报错：界面大多能容错，
      // 报错会让跟被验证对象无关的区块一起红掉，干扰判断
      return {}
  }
}

/* ---------- HTTP 服务 ---------- */

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
  '.woff2': 'font/woff2',
}

const STUB = `(() => {
  let seq = 0
  const cbs = {}
  window.__TAURI_INTERNALS__ = {
    transformCallback(cb, once) {
      const id = ++seq
      cbs[id] = { cb, once }
      window['_' + id] = (payload) => { cb(payload); if (once) delete cbs[id] }
      return id
    },
    unregisterCallback(id) { delete cbs[id] },
    convertFileSrc(p) { return p },
    async invoke(cmd, args) {
      const r = await fetch('/__invoke/' + encodeURIComponent(cmd), {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(args || {}),
      })
      const j = await r.json()
      if (j.ok) return j.value
      throw new Error(j.error || ('invoke 失败: ' + cmd))
    },
  }
  // 让页面知道自己跑在夹具里，便于脚本判定
  window.__HARNESS__ = true
})()
`

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://127.0.0.1:${PORT}`)

  if (url.pathname === '/favicon.ico') {
    // dist 里本来就没有图标；不拦会以 404 出现在错误日志里，容易和真问题混淆
    res.writeHead(204)
    res.end()
    return
  }

  if (url.pathname === '/__stub.js') {
    res.writeHead(200, MIME['.js'] ? { 'content-type': MIME['.js'] } : {})
    res.end(STUB)
    return
  }

  if (url.pathname.startsWith('/__invoke/')) {
    const cmd = decodeURIComponent(url.pathname.slice('/__invoke/'.length))
    let body = ''
    req.on('data', (c) => (body += c))
    await new Promise((r) => req.on('end', r))
    let args = {}
    try {
      args = body ? JSON.parse(body) : {}
    } catch {
      /* 空 body 正常 */
    }
    try {
      const value = await invoke(cmd, args)
      res.writeHead(200, { 'content-type': 'application/json; charset=utf-8' })
      res.end(JSON.stringify({ ok: true, value }))
    } catch (e) {
      res.writeHead(200, { 'content-type': 'application/json; charset=utf-8' })
      res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }))
    }
    return
  }

  let rel = url.pathname === '/' ? '/index.html' : url.pathname
  let file = join(DIST, decodeURIComponent(rel))

  if (!file.startsWith(DIST) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
    res.writeHead(404)
    res.end('not found: ' + rel)
    return
  }

  if (rel === '/index.html') {
    // 桩必须排在模块脚本之前：模块加载失败不会走 window.onerror，
    // 先装好桥才能让真实产物正常拿到 invoke
    let html = fs.readFileSync(file, 'utf8')
    html = html.replace('<head>', '<head>\n    <script src="/__stub.js"></script>')
    res.writeHead(200, { 'content-type': MIME['.html'] })
    res.end(html)
    return
  }

  res.writeHead(200, { 'content-type': MIME[path.extname(file)] || 'application/octet-stream' })
  res.end(fs.readFileSync(file))
})

server.listen(PORT, '127.0.0.1', () => {
  console.log(`UI 夹具已启动：http://127.0.0.1:${PORT}/`)
  console.log(`  dist      : ${DIST}`)
  console.log(`  真实配置  : ${CFG}`)
  console.log(`  MCP 二进制: ${MCP_EXE} ${fs.existsSync(MCP_EXE) ? '(存在)' : '(缺失)'}`)
  console.log('按 Ctrl+C 结束')
})
