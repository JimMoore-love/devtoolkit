// 自检脚本公用：确保 DevToolkit 正在运行（控制口可连）
//
// 自动拉起默认是关闭的（见 devtoolkit-mcp.rs 里的说明），所以自检脚本自己负责把 GUI 拉起来，
// 这样测试才可重复，也不依赖用户先手动打开应用。
import { spawn } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import net from 'node:net'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const cfgPath = join(process.env.APPDATA, 'cn.devtoolkit.app', 'devtoolkit.json')

export function controlPort() {
  try {
    const c = JSON.parse(readFileSync(cfgPath, 'utf8'))
    if (c.mcp_port) return c.mcp_port
  } catch {}
  return 9527
}

function probe(port) {
  return new Promise((res) => {
    const s = net.connect({ host: '127.0.0.1', port })
    const done = (v) => {
      s.destroy()
      res(v)
    }
    s.setTimeout(800)
    s.on('connect', () => done(true))
    s.on('error', () => done(false))
    s.on('timeout', () => done(false))
  })
}

export async function ensureGui() {
  const port = controlPort()
  if (await probe(port)) return port

  const exe = resolve(root, 'src-tauri/target/release/devtoolkit.exe')
  if (!existsSync(exe)) throw new Error(`未找到 ${exe}，先执行 cargo build --release`)
  spawn(exe, [], { detached: true, stdio: 'ignore', cwd: dirname(exe) }).unref()

  for (let i = 0; i < 30; i++) {
    await new Promise((r) => setTimeout(r, 500))
    if (await probe(port)) return port
  }
  throw new Error(`DevToolkit 启动后 ${port} 端口一直没就绪，请手动打开 DevToolkit 后重试`)
}

export function configPath() {
  return cfgPath
}
