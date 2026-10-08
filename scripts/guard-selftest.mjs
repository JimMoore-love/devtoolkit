// 自证：guard-checks 的每一条判断都真的会响。
//
// 一个安全检查最坏的形态不是"漏了"，而是**看起来在检查、其实什么都不判**
// —— 它给人虚假的安全感，比没有更糟。所以这里用临时目录伪造各种"被改坏"
// 的源码结构，逐条确认 checkGuards 报得出来；再用一份正常结构确认它不误报。
//
// 全程只碰临时目录，不动仓库里的任何源码。
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { execFileSync } from 'node:child_process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { checkGuards } from './guard-checks.mjs'

// Windows 上 ESM 的 import 只认 file:// URL，裸盘符路径会 ERR_UNSUPPORTED_ESM_URL_SCHEME
const GUARD_MODULE = pathToFileURL(path.resolve(path.dirname(fileURLToPath(import.meta.url)), 'guard-checks.mjs')).href

let pass = 0
let fail = 0

function check(name, cond, detail = '') {
  if (cond) {
    pass++
    console.log(`  ✔ ${name}`)
  } else {
    fail++
    console.log(`  ✖ ${name}${detail ? '  — ' + detail : ''}`)
  }
}

// ---------------------------------------------------------------- 伪造源码
// 一份"结构正确"的最小骨架：分级闸门在，裸入口传 Blind，放行口锁在 rt.pids
const GOOD_MAIN = String.raw`
fn kill_pid(app: AppHandle, pid: u32) -> Result<KillResult, String> {
    kill_pid_scoped(&app, pid, KillScope::Blind)
}

pub(crate) fn kill_pid_scoped(app: &AppHandle, pid: u32, scope: KillScope) -> Result<KillResult, String> {
    if scope == KillScope::Blind {
        match protected_hits(app, pid) {
            Ok(hit) if !hit.is_empty() => return Ok(KillResult { ok: false, message: "protected".into() }),
            _ => {}
        }
    }
    let (ok, message) = platform::kill_process(pid)?;
    Ok(KillResult { ok, message })
}

pub(crate) async fn takeover_project(app: AppHandle, project: Project) -> Result<TakeoverResult, String> {
    for pid in &rt.pids {
        kill_pid_scoped(&app, *pid, KillScope::Project)?;
    }
    let task = start_task(app, project).await?;
    Ok(TakeoverResult { killed_pids, task })
}

/// 关闭应用时保留受保护端口上的托管任务
fn keep_alive_tasks(tasks: &[(u32, Project)], ports: &[PortInfo], protected: &[u16]) -> Vec<u32> {
    if protected.is_empty() {
        return Vec::new();
    }
    let runtime = runtime_by_ports(&projects, ports);
    tasks
        .iter()
        .filter(|(pid, p)| {
            !match_protected(ports, *pid, protected).is_empty()
                || runtime.iter().any(|r| r.id == p.id)
        })
        .map(|(pid, _)| *pid)
        .collect()
}

fn wire_close_guard(app: &AppHandle) {
    app.on_window_event(|window, event| {
        if let tauri::WindowEvent::CloseRequested { .. } = event {
            let keep = keep_alive_tasks(&tasks, &ports, &protected);
            let _ = keep.contains(&1u32);
        }
    });
}
`

const GOOD_CONTROL = String.raw`
        Cmd::TakeoverProject => {
            match block(crate::takeover_project(app.clone(), project)) { }
        }
        Cmd::StopProject => { }
`

/**
 * 把伪造源码铺到临时目录，在**子进程**里跑 checkGuards。
 * 用子进程是因为 ROOT 在模块加载时就算好了，同进程改环境变量没用。
 * @returns {string[]} problems
 */
function probe(mainRs, controlRs, cfg) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'guard-selftest-'))
  try {
    fs.mkdirSync(path.join(root, 'src-tauri', 'src'), { recursive: true })
    fs.writeFileSync(path.join(root, 'src-tauri', 'src', 'main.rs'), mainRs)
    fs.writeFileSync(path.join(root, 'src-tauri', 'src', 'control.rs'), controlRs)

    const probeScript = path.join(root, '__probe.mjs')
    fs.writeFileSync(
      probeScript,
      `import { checkGuards } from ${JSON.stringify(GUARD_MODULE)}\n` +
        `process.stdout.write(JSON.stringify(checkGuards(${JSON.stringify(cfg)}).problems))\n`
    )

    const out = execFileSync(process.execPath, [probeScript], {
      env: { ...process.env, DEVTOOLKIT_GUARD_ROOT: root },
      encoding: 'utf8',
    })
    return JSON.parse(out)
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
}

/** 在伪造源码里做一处替换；替换目标必须存在，否则用例本身是坏的 */
function edit(src, from, to) {
  if (!src.includes(from)) throw new Error(`用例失效：伪造源码里找不到待替换片段 ${JSON.stringify(from)}`)
  return src.replace(from, to)
}

// ---------------------------------------------------------------- 用例
console.log('\n— 保护闸门：自证这些判断真的会响 —')

const has = (problems, re) => problems.some((p) => re.test(p))

// 1) 空名单：最隐蔽的坏法，命令还能跑，只是不再拦人
check(
  '保护名单为空会被抓出',
  has(probe(GOOD_MAIN, GOOD_CONTROL, { protected_ports: [] }), /protected_ports 为空/)
)

// 2) 正向：结构完好时不能误报（误报会让闸门被当成噪音而遭绕过）
{
  const r = probe(GOOD_MAIN, GOOD_CONTROL, { protected_ports: [8000, 9528] })
  check('结构正常时不误报', r.length === 0, JSON.stringify(r))
}

// 3) 裸入口被改成 Project —— 等于给"拿个裸 PID 就杀"开后门
check(
  'kill_pid 命令没走 Blind 会被抓出',
  has(
    probe(edit(GOOD_MAIN, 'kill_pid_scoped(&app, pid, KillScope::Blind)', 'kill_pid_scoped(&app, pid, KillScope::Project)'), GOOD_CONTROL, {
      protected_ports: [8000],
    }),
    /没有以 KillScope::Blind 委托/
  )
)

// 4) kill_pid_scoped 里的闸门被整段删掉
check(
  'kill_pid_scoped 缺 protected_hits 会被抓出',
  has(
    probe(edit(GOOD_MAIN, 'match protected_hits(app, pid) {', 'match Vec::<u16>::new() {'), GOOD_CONTROL, {
      protected_ports: [8000],
    }),
    /没有 protected_hits 闸门/
  )
)

// 5) 闸门还在，但不再绑定 Blind —— 最阴的一种：代码里还看得见"检查"两个字
check(
  '闸门没绑在 Blind 上会被抓出',
  has(
    probe(edit(GOOD_MAIN, 'if scope == KillScope::Blind {', 'if true {'), GOOD_CONTROL, {
      protected_ports: [8000],
    }),
    /没绑定在 KillScope::Blind/
  )
)

// 6) 闸门被挪到真正杀进程之后
check(
  'kill_pid_scoped 闸门排在杀进程之后会被抓出',
  has(
    probe(
      GOOD_MAIN.replace(
        /    if scope == KillScope::Blind \{[\s\S]*?\n    \}\n/,
        ''
      ).replace(
        '    let (ok, message) = platform::kill_process(pid)?;',
        '    let (ok, message) = platform::kill_process(pid)?;\n    if scope == KillScope::Blind { let _ = protected_hits(app, pid); }'
      ),
      GOOD_CONTROL,
      { protected_ports: [8000] }
    ),
    /之后/
  )
)

// 7) 放行口的范围被放宽 —— 不再锁在"本项目声明端口上的进程"
check(
  'takeover 不限定 rt.pids 会被抓出',
  has(
    probe(edit(GOOD_MAIN, 'for pid in &rt.pids {', 'for pid in &all_pids {'), GOOD_CONTROL, {
      protected_ports: [8000],
    }),
    /没把结束范围限定在 rt\.pids/
  )
)

// 8) 放行口忘了声明 Project —— 受保护端口上会卡死（这正是本次修的那个 bug）
check(
  'takeover 没以 Project 放行会被抓出',
  has(
    probe(edit(GOOD_MAIN, 'kill_pid_scoped(&app, *pid, KillScope::Project)?;', 'kill_pid_scoped(&app, *pid, KillScope::Blind)?;'), GOOD_CONTROL, {
      protected_ports: [8000],
    }),
    /没有以 KillScope::Project 放行/
  )
)

// 9) 放行口自己另起一套杀进程逻辑，绕过 kill_pid_scoped 的上下文
check(
  'takeover 直接调 platform::kill_process 会被抓出',
  has(
    probe(
      edit(GOOD_MAIN, '        kill_pid_scoped(&app, *pid, KillScope::Project)?;', '        platform::kill_process(*pid)?;'),
      GOOD_CONTROL,
      { protected_ports: [8000] }
    ),
    /绕过了 kill_pid_scoped/
  )
)

// 10) 控制口另起一套接管逻辑，界面与 AI 走出两套行为
check(
  '控制口接管分支不转发会被抓出',
  has(
    probe(
      GOOD_MAIN,
      String.raw`
        Cmd::TakeoverProject => {
            for pid in pids { let _ = kill_pid(app.clone(), pid); }
        }
        Cmd::StopProject => { }
`,
      { protected_ports: [8000] }
    ),
    /没有转发到 crate::takeover_project/
  )
)

// 11) 源码读不到（被清空、被移走、路径写错）必须报错，绝不能静默放行
{
  const r = probe('', '', { protected_ports: [8000] })
  check('源码读不到时报错而不是放行', r.length >= 2, JSON.stringify(r))
}

// 12) 关闭窗口的保留判定退化成"只看任务进程自己的端口" —— 而任务 PID 是 cmd 启动器，
//     它不监听任何端口，判定必然为空，于是关窗口就把用户的 8000/9528 一起带走
check(
  '关窗口只看任务自身端口会被抓出',
  has(
    probe(edit(GOOD_MAIN, 'runtime_by_ports(&projects, ports)', 'Vec::new()'), GOOD_CONTROL, {
      protected_ports: [8000],
    }),
    /只看任务进程自己的端口/
  )
)

// 13) 关闭路径干脆不调用保留判定
check(
  'CloseRequested 不调用 keep_alive_tasks 会被抓出',
  has(
    probe(edit(GOOD_MAIN, 'let keep = keep_alive_tasks(&tasks, &ports, &protected);', 'let keep = Vec::<u32>::new();'), GOOD_CONTROL, {
      protected_ports: [8000],
    }),
    /没有调用 keep_alive_tasks/
  )
)

// 14) 丢了"任务进程自己就在受保护端口上"这一层
check(
  '保留判定丢了任务自身端口会被抓出',
  has(
    probe(edit(GOOD_MAIN, '!match_protected(ports, *pid, protected).is_empty()', 'false'), GOOD_CONTROL, {
      protected_ports: [8000],
    }),
    /丢了"任务进程自己就在受保护端口上"这一层/
  )
)

// 15) 空名单分支被去掉 —— 保护名单清空后关窗口会一个任务都不结束
check(
  '保留判定不处理空名单会被抓出',
  has(
    probe(edit(GOOD_MAIN, 'if protected.is_empty() {', 'if false {'), GOOD_CONTROL, {
      protected_ports: [8000],
    }),
    /没处理"保护名单为空"/
  )
)

// ---------------------------------------------------------------- 端口解析：单一真相
//
// 背景：端口声明的解析规则曾在仓库里有 **5 份副本**（Rust 3 份 + JS 2 份），
// 分隔符、跨度上限、是否接受端口 0 各写各的，直接导致用户可见的矛盾 ——
// 「端口扫描里写 1-2000 能跑，项目管理里写 3000-4000 报跨度过大」。
//
// 下面这几条断言做两件事：
//   1. 防止第 6 份副本出现（扫描全仓，白名单之外不许再出现解析特征）
//   2. 防止两侧规则再次漂移：直接把 ports.rs 里那份声明式用例表抠出来，
//      逐条喂给前端实现。两侧对着**同一张契约**校验，不需要第三份副本。

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url))
const REPO_ROOT = path.resolve(SCRIPT_DIR, '..')

const { parsePortSpec, MAX_PORT_RANGE_SPAN } = await import(
  pathToFileURL(path.join(REPO_ROOT, 'src', 'portSpec.js')).href
)

function collectFiles(dir, exts, out = []) {
  let entries = []
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true })
  } catch {
    return out
  }
  for (const e of entries) {
    const p = path.join(dir, e.name)
    if (e.isDirectory()) {
      if (e.name === 'node_modules' || e.name === '.git' || e.name === 'target') continue
      collectFiles(p, exts, out)
    } else if (exts.some((x) => e.name.endsWith(x))) {
      out.push(p)
    }
  }
  return out
}

const rel = (p) => path.relative(REPO_ROOT, p).split(path.sep).join('/')

// ---- 1) 用例表：从 ports.rs 抠出来，逐条喂给前端实现 ----
const PORTS_RS = path.join(REPO_ROOT, 'src-tauri', 'src', 'ports.rs')
const NETWORK_RS = path.join(REPO_ROOT, 'src-tauri', 'src', 'network.rs')
const API_JS = path.join(REPO_ROOT, 'src', 'api.js')
const PORT_SPEC_JS = path.join(REPO_ROOT, 'src', 'portSpec.js')

let portsSrc = ''
try {
  portsSrc = fs.readFileSync(PORTS_RS, 'utf8')
} catch {}

// 匹配 `("3000-3003", &[3000, 3001, 3002, 3003]),`
const CASES = [...portsSrc.matchAll(/\("([^"]*)",\s*&\[([0-9,\s]*)\]\s*\)/g)].map((m) => ({
  spec: m[1],
  expect: m[2].split(',').map((s) => s.trim()).filter(Boolean).map(Number),
}))

// 先确认解析本身是有效的 —— 否则"0 条用例全过"是最坏的假安全
check('ports.rs 的共享用例表被成功解析', CASES.length >= 15, `解析到 ${CASES.length} 条`)

const sameNums = (a, b) => a.length === b.length && a.every((v, i) => v === b[i])
const mismatches = CASES.filter(({ spec, expect }) => !sameNums(parsePortSpec(spec), expect))
check(
  '前端实现满足 ports.rs 声明的每一条用例',
  CASES.length > 0 && mismatches.length === 0,
  mismatches
    .slice(0, 3)
    .map(({ spec, expect }) => `${JSON.stringify(spec)} 期望 ${JSON.stringify(expect)} 实得 ${JSON.stringify(parsePortSpec(spec))}`)
    .join('; ')
)

// ---- 2) 两侧的跨度上限与分隔符必须一致 ----
const rustSpan = /MAX_PORT_RANGE_SPAN\s*:\s*u16\s*=\s*(\d+)/.exec(portsSrc)
check(
  '跨度上限两侧一致',
  !!rustSpan && Number(rustSpan[1]) === MAX_PORT_RANGE_SPAN,
  `Rust=${rustSpan ? rustSpan[1] : '未找到'} JS=${MAX_PORT_RANGE_SPAN}`
)

const SEPARATORS = [',', ';', '，', '；', '|']
// 注意按「行」取：`const SEPARATORS: [char; 5] = [...]` 里那个分号会让
// 非贪婪的 `[\s\S]*?;` 提前收尾，检查就变成永远失败的噪音。
const sepLine = /const SEPARATORS[^\n]*/.exec(portsSrc)?.[0] ?? ''
check(
  'ports.rs 的分隔符集合完整',
  SEPARATORS.every((c) => sepLine.includes(`'${c}'`)),
  sepLine.replace(/\s+/g, ' ').slice(0, 80)
)

let portSpecSrc = ''
try {
  portSpecSrc = fs.readFileSync(PORT_SPEC_JS, 'utf8')
} catch {}
const jsSepLine = /const SEPARATORS[\s\S]*?\n/.exec(portSpecSrc)?.[0] ?? ''
check(
  'src/portSpec.js 的分隔符集合与 Rust 相同',
  SEPARATORS.every((c) => jsSepLine.includes(c)),
  jsSepLine.trim()
)

// ---- 3) 不许再出现第 6 份副本 ----
let networkSrc = ''
try {
  networkSrc = fs.readFileSync(NETWORK_RS, 'utf8')
} catch {}
const parsePortsBody = /fn parse_ports\([\s\S]*?\n\}/.exec(networkSrc)?.[0] ?? ''
check(
  'network.rs 的 parse_ports 已委托给 ports 模块',
  parsePortsBody.includes('crate::ports::parse_lenient'),
  parsePortsBody.replace(/\s+/g, ' ').slice(0, 90)
)
check(
  'network.rs 不再自带分隔符表',
  parsePortsBody.length > 0 && !/['"]，['"]/.test(parsePortsBody),
  '仍然出现全角逗号字面量'
)

let apiSrc = ''
try {
  apiSrc = fs.readFileSync(API_JS, 'utf8')
} catch {}
check(
  'api.js 从 portSpec.js re-export（保持对外接口不变）',
  /export\s*\{[^}]*parsePortSpec[^}]*\}\s*from\s*['"]\.\/portSpec\.js['"]/.test(apiSrc)
)
check(
  'api.js 里没有第二份实现',
  !/function\s+parsePortSpec\s*\(/.test(apiSrc) && !/PORT_RANGE_SPAN\s*=\s*\d/.test(apiSrc)
)

// 全仓扫描：除白名单外，不该有人再定义端口解析
const CANDIDATES = [
  ...collectFiles(path.join(REPO_ROOT, 'src'), ['.js', '.vue']),
  ...collectFiles(path.join(REPO_ROOT, 'scripts'), ['.mjs']),
]
const WHITELIST = new Set([rel(PORT_SPEC_JS), rel(API_JS)])
const offenders = []
for (const f of CANDIDATES) {
  if (WHITELIST.has(rel(f))) continue
  let src = ''
  try {
    src = fs.readFileSync(f, 'utf8')
  } catch {
    continue
  }
  // 特征：同时出现「自建端口区间变量」与「端口号边界 65535」才算一份解析副本
  const looksLikeParser =
    /MAX_PORT_RANGE_SPAN\s*=\s*\d/.test(src) ||
    (/parsePortSpec\s*\([^)]*\)\s*\{/.test(src) && /65535/.test(src))
  if (looksLikeParser) offenders.push(rel(f))
}
check('除 portSpec.js / api.js 外没有其它 JS 侧副本', offenders.length === 0, offenders.join(', '))

console.log(`\n结果：${pass} 通过 / ${fail} 失败`)
process.exit(fail ? 1 : 0)
