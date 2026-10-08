// 一次性诊断脚本：问控制口要 list_tasks / status，确认当前服务是不是本应用托管的
import fs from 'node:fs'
import net from 'node:net'
import { configPath, controlPort } from './ensure-gui.mjs'

// 配置路径不写死用户名（原先硬编码为 C:/Users/lenovo/...），统一走 ensure-gui 的推导
const CFG = configPath()
const cfg = JSON.parse(fs.readFileSync(CFG, 'utf8'))

// 端口同样不写死：原先这里硬编码 9527，用户在管理页把控制口改到别的端口后，
// 配置里的 mcp_port 变了而这个脚本还往 9527 连 —— 端口上根本没有监听，
// 报的却是"连不上"，很容易被误判成应用没启动。controlPort() 就是读同一个配置。
const PORT = controlPort()

function call(cmd, args = {}) {
  return new Promise((resolve) => {
    const s = net.connect(PORT, '127.0.0.1', () => {
      s.write(JSON.stringify({ token: cfg.mcp_token, cmd, args }) + '\n')
    })
    let buf = ''
    s.on('data', (d) => {
      buf += d
      s.end()
    })
    s.on('end', () => resolve(buf.trim()))
    s.on('error', (e) => resolve('ERR ' + e.message))
  })
}

for (const c of ['status', 'list_tasks']) {
  console.log(`=== ${c} ===`)
  console.log(await call(c))
}
