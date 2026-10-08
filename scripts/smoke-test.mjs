/**
 * 一键验证：把"改完之后到底还行不行"收敛成一条命令。
 *
 *   node scripts/smoke-test.mjs            # 全量（含会改配置的写链路测试）
 *   node scripts/smoke-test.mjs --quick    # 快速（跳过写链路测试与前端构建）
 *   node scripts/smoke-test.mjs --no-build # 跳过前端构建
 *
 * 每一步独立执行、独立判定，最后汇总。任何一步失败都以非 0 退出，
 * 便于挂到 CI 或 pre-push。
 */
import { spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const isWin = process.platform === 'win32'
const args = new Set(process.argv.slice(2))
const quick = args.has('--quick')
const noBuild = args.has('--no-build') || quick

/** 有显式路径就不用依赖 PATH */
function pickBin(candidates, fallback) {
  for (const c of candidates) {
    if (c && existsSync(c)) return c
  }
  return fallback
}

/**
 * cargo 自己会去 PATH 里找 rustc，只给它绝对路径是不够的。
 * 这里把工具链目录补进 PATH，否则 cargo 会在 0.1s 内以"找不到 rustc"退出，
 * 看起来像测试失败，实际是环境没接上。
 */
const extraPath = [
  isWin ? 'D:\\ServBay\\packages\\rust\\1\\cargo\\bin' : null,
  isWin ? 'D:\\ServBay\\packages\\rust\\1\\rustc\\bin' : null,
  isWin ? 'D:\\ServBay\\packages\\node\\current' : null,
  isWin ? 'C:\\Users\\lenovo\\.cargo\\bin' : null,
].filter((p) => p && existsSync(p))

/** PATH 变量在 Windows 上是 'Path'，全量覆盖键名才对，这里按不区分大小写替换 */
function withToolchainPath(env) {
  if (!extraPath.length) return { ...env }
  const out = { ...env }
  const key = Object.keys(out).find((k) => k.toLowerCase() === 'path') || 'PATH'
  const sep = isWin ? ';' : ':'
  const current = out[key] || ''
  out[key] = [...extraPath, current].filter(Boolean).join(sep)
  return out
}

const CHILD_ENV = withToolchainPath(process.env)

const CARGO = pickBin(
  [
    process.env.CARGO,
    isWin ? 'D:\\ServBay\\packages\\rust\\1\\cargo\\bin\\cargo.exe' : null,
    isWin ? 'C:\\Users\\lenovo\\.cargo\\bin\\cargo.exe' : null,
    '/usr/local/bin/cargo',
  ],
  'cargo'
)

const NPM = pickBin(
  [
    process.env.NPM,
    isWin ? 'D:\\ServBay\\packages\\node\\current\\npm.cmd' : null,
  ],
  isWin ? 'npm.cmd' : 'npm'
)

const steps = [
  {
    name: '静态自检（模板引用 / 图标名 / 前后端命令对齐）',
    cmd: process.execPath,
    args: ['scripts/check-template-refs.mjs'],
  },
  {
    // 保护闸门是唯一阻止 DevToolkit（及经控制口驱动的 AI）杀掉用户常驻服务的东西，
    // 它坏掉时表面毫无异常，所以每次验证都静态核实一遍：名单非空 + 闸门排在动手之前。
    // 这一步跑的是 guard-selftest：用伪造源码逐条证明检查真的会响，而不是空转
    name: '保护闸门自证（坏法能被抓出、好结构不误报）',
    cmd: process.execPath,
    args: ['scripts/guard-selftest.mjs'],
  },
  {
    name: 'Rust 单元测试',
    cmd: CARGO,
    args: ['test', '--release'],
    cwd: resolve(root, 'src-tauri'),
  },
  {
    name: 'MCP 协议层测试（负向）',
    cmd: process.execPath,
    args: ['scripts/mcp-protocol-test.mjs'],
  },
  {
    name: '受保护端口端到端拦截（会在真实项目上验证"拒了"）',
    cmd: process.execPath,
    args: ['scripts/protect-e2e.mjs'],
  },
  {
    name: 'MCP 读链路自检',
    cmd: process.execPath,
    args: ['scripts/mcp-selftest.mjs'],
  },
]

if (!quick) {
  steps.splice(3, 0, {
    name: 'MCP 写链路自检（会临时改配置，结束自动还原）',
    cmd: process.execPath,
    args: ['scripts/mcp-write-test.mjs'],
  })
}

if (!noBuild) {
  steps.push({
    name: '前端构建',
    cmd: NPM,
    args: ['run', 'build'],
    shell: isWin,
  })
}

const results = []
const started = Date.now()

for (const [i, s] of steps.entries()) {
  const label = `[${i + 1}/${steps.length}] ${s.name}`
  console.log(`\n\x1b[36m${label}\x1b[0m`)
  const t0 = Date.now()
  const r = spawnSync(s.cmd, s.args, {
    cwd: s.cwd || root,
    encoding: 'utf8',
    shell: !!s.shell,
    env: CHILD_ENV,
  })
  const ms = Date.now() - t0
  const out = `${r.stdout || ''}${r.stderr || ''}`.trimEnd()
  // 只回显末尾若干行：成功的步骤日志很长，看头没意义
  const lines = out ? out.split('\n') : []
  const tail = lines.slice(-24)
  if (tail.length) {
    console.log(tail.map((l) => '  │ ' + l).join('\n'))
    if (lines.length > tail.length) console.log(`  │ …（省略前 ${lines.length - tail.length} 行）`)
  }
  const code = r.status === null ? 1 : r.status
  // 失败且毫无输出是最难查的情况（通常是没启动成功），这里把原因显式打出来
  if (code !== 0 && !out) {
    console.log(`  │ ⚠ 该步骤没有任何输出，通常是可执行文件没找到或无法启动`)
    console.log(`  │   命令：${s.cmd} ${s.args.join(' ')}`)
    if (r.error) console.log(`  │   原因：${r.error.message || r.error}`)
    if (r.signal) console.log(`  │   信号：${r.signal}`)
  }
  // cargo 写不进去 ≠ 测试失败：应用正跑在同一个 target 目录上时，Windows 会锁住
  // deps 下的中间产物，报的是 "permission denied"。这条提示省掉一次"是不是代码坏了"的误判。
  if (code !== 0 && /permission denied|being used by another process|拒绝访问/i.test(out)) {
    console.log('  │ ⚠ 这是输出目录被占用，不是测试失败：目标目录正被运行中的应用锁着。')
    console.log('  │   换个输出目录重跑即可（该环境变量会透传给 cargo）：')
    console.log(`  │   CARGO_TARGET_DIR=${resolve(root, 'target-ctx').replace(/\\/g, '/')} npm run verify:quick`)
  }
  results.push({
    name: s.name,
    code,
    ms,
    error: r.error ? String(r.error.message || r.error) : code !== 0 && !out ? '无输出，疑似启动失败' : '',
  })
  console.log(code === 0 ? `  \x1b[32m✔ 通过\x1b[0m （${(ms / 1000).toFixed(1)}s）` : `  \x1b[31m✖ 失败（退出码 ${code}）\x1b[0m`)
}

console.log('\n================ 汇总 ================')
for (const r of results) {
  const mark = r.code === 0 ? '\x1b[32m✔\x1b[0m' : '\x1b[31m✖\x1b[0m'
  console.log(`${mark} ${r.name}  ${(r.ms / 1000).toFixed(1)}s${r.error ? '  ' + r.error : ''}`)
}
const failed = results.filter((r) => r.code !== 0)
console.log(`\n${results.length - failed.length}/${results.length} 通过，共耗时 ${((Date.now() - started) / 1000).toFixed(1)}s`)
if (quick) console.log('（--quick 模式：已跳过写链路测试与前端构建）')

// 写链路测试依赖控制口；应用没开时它自己会说明，这里只做提示
const needGui = failed.some((r) => /MCP/.test(r.name))
if (needGui) {
  console.log('\n提示：MCP 相关步骤需要 DevToolkit 正在运行（控制口 127.0.0.1:9527）。')
}

process.exit(failed.length ? 1 : 0)
