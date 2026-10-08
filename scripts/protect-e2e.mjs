// 端到端验证「受保护端口」这道闸门：该拦的仍然拦得住，该放的确实放得开。
//
// 后面几个单元测试只验证 protected_hits 的判定逻辑，控制口协议测试只验证命令边界；
// 真正要确认的是链路：配置里的保护名单 → 控制口入口 → 进程一个没死。
//
// 这个脚本故意选"用户正在跑的真实项目"作为靶子，所以它**只做不会造成伤害的断言**：
//   1. 裸 kill_pid 打受保护端口上的进程 → 必须被拒，且事后进程一个没少
//   2. 对「已托管」的项目调 takeover / start → 必须走"已在运行/无需接管"这条短路，
//      证明它根本没有走到结束进程那一步
//   3. 非保护端口上的 kill_pid 行为不受影响（保护没有扩大化成"什么都干不了"）
//
// 反面情形 —— "外部启动的项目在受保护端口上能不能被成功接管" —— **不在这里测**：
// 那需要真的杀掉用户的服务来验证成功路径。该路径由 guard-checks.mjs 做静态核实
// （takeover 的结束范围锁死 rt.pids、且以 Project 放行），不拿用户的线上服务做实验。
import fs from 'node:fs'
import net from 'node:net'
import { ensureGui, configPath } from './ensure-gui.mjs'
import { checkGuards, formatProblems } from './guard-checks.mjs'

let pass = 0
let fail = 0
let skip = 0

function check(name, cond, detail = '') {
  if (cond) {
    pass++
    console.log(`  ✔ ${name}${detail ? '  — ' + detail : ''}`)
  } else {
    fail++
    console.log(`  ✖ ${name}${detail ? '  — ' + detail : ''}`)
  }
}
function skipIt(name, why) {
  skip++
  console.log(`  ↩ ${name}（跳过：${why}）`)
}

const cfg = JSON.parse(fs.readFileSync(configPath(), 'utf8'))

// 熔断：本脚本会故意去调 kill_pid / takeover_project 打真实项目的靶子，
// 它的安全性**完全依赖**"保护闸门确实存在、且放行口只对项目自己的端口开"。
// 一旦闸门被改坏（函数名变了、挪到了杀进程之后、条件被放宽到所有调用、
// 或名单被清空），这里跑的就不是测试，而是"真杀用户的服务"。
// 所以先静态核实闸门，有任何一条对不上就立刻中止，绝不把破坏性调用发出去。
// （判断逻辑与 port-guard.mjs 共用 guard-checks.mjs，避免同一套规则写两遍）
const guard = checkGuards(cfg)
if (guard.problems.length) {
  console.log('\n— 受保护端口：端到端拦截 —')
  console.log('  ⛔ 熔断：保护闸门未通过静态核实，已中止（绝不执行破坏性调用）')
  console.log(formatProblems(guard.problems))
  console.log('\n  这个脚本拿真实项目当靶子，闸门不可信时跑它等于直接杀服务。')
  console.log('  请先修好 src-tauri/src/main.rs / control.rs 里的闸门逻辑，再重跑。\n')
  process.exit(1)
}

await ensureGui()

function call(cmd, args) {
  return new Promise((res, rej) => {
    const s = net.connect({ host: '127.0.0.1', port: cfg.mcp_port })
    let buf = ''
    s.setTimeout(15000)
    s.on('connect', () => s.write(JSON.stringify({ token: cfg.mcp_token, cmd, args }) + '\n'))
    s.on('data', (d) => {
      buf += d
      const i = buf.indexOf('\n')
      if (i >= 0) {
        s.destroy()
        try {
          res(JSON.parse(buf.slice(0, i)))
        } catch (e) {
          rej(e)
        }
      }
    })
    s.on('error', rej)
    s.on('timeout', () => {
      s.destroy()
      rej(new Error('超时'))
    })
  })
}

/** 受保护端口上的活动监听：pid → 端口 */
async function liveOnProtected(protectedPorts) {
  const r = await call('list_ports', {})
  const out = new Map()
  for (const p of r.data || []) {
    if (protectedPorts.includes(p.local_port) && p.state === 'LISTENING' && p.pid) {
      out.set(p.pid, p.local_port)
    }
  }
  return out
}

/** 当前被本应用托管的任务：project_id 集合 */
async function managedProjectIds() {
  const r = await call('list_tasks', {})
  return new Set((r.data?.tasks || []).map((t) => t.project_id))
}

console.log('\n— 受保护端口：端到端拦截 —')
const protectedPorts = cfg.protected_ports || []
console.log(`  配置中的保护名单: [${protectedPorts.join(', ')}]`)

const before = await liveOnProtected(protectedPorts)
if (!before.size) {
  skipIt('受保护端口拦截', '当前没有进程监听受保护端口，测不出"该拒的拒了"')
} else {
  console.log(`  靶子: ${[...before].map(([pid, port]) => `pid ${pid} @ :${port}`).join(', ')}`)

  // 1) 结束受保护端口上的进程 → 必须被拒。
  //    这条是保护的核心承诺：**不管谁调、带不带项目上下文，裸 PID 都不许杀**。
  for (const [pid, port] of before) {
    const r = await call('kill_pid', { pid })
    check(
      `kill_pid(${pid}) 被拒（该进程占着受保护端口 :${port}）`,
      r.ok === true && r.data && r.data.ok === false && /受保护/.test(r.data.message || ''),
      (r.data && r.data.message) || JSON.stringify(r.error || r.data)
    )
  }

  // 2) 已托管的项目：接管与启动都必须走短路，不能碰进程。
  //    这两条同时说明"放行口"没有失守成"什么都能杀"——它对已经在管的项目直接拒绝。
  const projects = cfg.projects || []
  const managed = await managedProjectIds()
  for (const [pid, port] of before) {
    const proj = projects.find((p) => String(p.ports || '').includes(String(port)))
    if (!proj) continue
    if (!managed.has(proj.id)) {
      skipIt(
        `接管「${proj.name}」`,
        '该项目当前由外部进程启动 —— 测成功路径会真杀掉你的服务，故不在此处验证'
      )
      continue
    }

    const t = await call('takeover_project', { name: proj.name })
    check(
      `接管「${proj.name}」走短路（已托管，无需接管）`,
      t.ok === false && /无需接管/.test(t.error || '') && !/受保护/.test(t.error || ''),
      t.error || JSON.stringify(t.data)
    )

    const s = await call('start_project', { name: proj.name })
    check(
      `start「${proj.name}」报「已在运行中」而非重新拉起`,
      s.ok === true && s.data?.started === false && s.data?.reason === 'already_running',
      JSON.stringify(s.data || s.error)
    )
  }

  // 3) 最要紧的一条：被拒之后，那些进程必须一个都没少
  const after = await liveOnProtected(protectedPorts)
  const lost = [...before.keys()].filter((pid) => !after.has(pid))
  check(
    '被拒之后原有进程一个都没被误杀',
    lost.length === 0,
    lost.length ? `被误杀: ${lost.join(', ')}` : `${before.size} 个进程全部存活`
  )

  if (lost.length) {
    console.log('\n  ⛔ 严重：受保护端口上的进程被误杀了，保护闸门存在漏洞。')
    for (const pid of lost) {
      console.log(`     · pid ${pid} @ :${before.get(pid)} 已消失`)
    }
    console.log('  请立刻恢复服务，并在修复闸门之前**不要再跑本脚本**（它会继续杀）。')
    console.log('  可用的排查线索：')
    console.log('     - 确认 kill_pid 命令仍以 KillScope::Blind 委托')
    console.log('     - 确认 kill_pid_scoped 的 Blind 判断还在，且排在 platform::kill_process 之前')
    console.log('     - 确认 takeover_project 的结束范围仍锁在 rt.pids')
    console.log('     - 确认配置里的 protected_ports 未被清空\n')
  }

  // 4) 反向确认：保护名单**不影响**非保护端口上的操作，
  //    否则"保护"就成了"什么都干不了"
  const r = await call('kill_pid', { pid: 999999 })
  check(
    '非保护端口的 kill_pid 仍可正常执行（保护没有扩大化）',
    r.ok === true && r.data && r.data.ok === false && !/受保护/.test(r.data.message || ''),
    (r.data && r.data.message) || JSON.stringify(r.error || r.data)
  )
}

console.log(`\n结果：${pass} 通过 / ${fail} 失败${skip ? ` / ${skip} 跳过` : ''}`)
process.exit(fail ? 1 : 0)
