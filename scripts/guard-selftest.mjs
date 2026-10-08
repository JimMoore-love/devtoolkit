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

console.log(`\n结果：${pass} 通过 / ${fail} 失败`)
process.exit(fail ? 1 : 0)
