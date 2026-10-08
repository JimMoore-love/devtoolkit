// 一次性诊断脚本：问控制口要 list_tasks / status，确认当前服务是不是本应用托管的
import fs from 'node:fs'
import net from 'node:net'
import { configPath } from './ensure-gui.mjs'

// 配置路径不写死用户名（原先硬编码为 C:/Users/lenovo/...），统一走 ensure-gui 的推导
const CFG = configPath()
const cfg = JSON.parse(fs.readFileSync(CFG, 'utf8'))

function call(cmd, args = {}) {
  return new Promise((resolve) => {
    const s = net.connect(9527, '127.0.0.1', () => {
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
