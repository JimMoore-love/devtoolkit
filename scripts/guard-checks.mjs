// 「受保护端口」闸门的静态核实。
//
// 为什么要单独有这个：保护闸门是**唯一**阻止 DevToolkit（以及经控制口驱动的 AI）
// 杀掉用户常驻服务的东西。它一旦被改坏 —— 函数名换了、判断挪到了杀进程之后、
// 条件被放宽、或者配置里的名单被清空 —— 表面上看不出任何异常，界面照常、单测照常，
// 直到某次操作真的把用户的 8000 / 9528 杀掉。
//
// 所以这里不看行为、只读源码结构。**闸门的语义是"按上下文分级"**，对应的五条：
//   1. 名单非空
//   2. 裸入口 `kill_pid` 命令必须走 Blind —— 它只拿到一个 PID，不能替调用方放宽
//   3. `kill_pid_scoped` 里必须真的存在"Blind 才拦"的判断，且排在真正杀进程之前
//   4. 唯一的放行口 `takeover_project` 必须把结束范围限定在 `rt.pids`
//      （即"监听本项目声明端口"的那些进程），并显式用 Project 放行
//   5. 关闭窗口时，受保护端口上的托管任务必须留下 —— 且判定不能只看启动器 PID
//
// 第 4 条是"分级闸门"重构时新增的：原先放行与否看的是"入口有没有拦截"，
// 现在看的是"杀的到底是谁"。第 5 条是关闭路径的同类坑：任务 PID 是 `cmd /C`
// 启动器，它自己不监听端口，只看它会判空，而结束是连树杀的。
// 少了两头任何一条，保护都会悄悄失效。
//
// 纯函数，不碰网络、不碰进程，两个脚本共用（protect-e2e 拿它做熔断，
// port-guard 拿它做体检），避免同一套判断写两遍。
import fs from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const ROOT = path.resolve(
  process.env.DEVTOOLKIT_GUARD_ROOT || path.join(path.dirname(fileURLToPath(import.meta.url)), '..')
)

/**
 * 取一个函数的正文：从签名起，到下一个行首 `}` 为止。
 *
 * 上一版用的是固定 3000 字符窗口，函数一长就会把后面的代码一起圈进来 ——
 * 于是"闸门在不在这个函数里"变成了"闸门在不在这个窗口里"，检查会悄悄失真。
 *
 * @returns {string|null} 找不到签名时返回 null
 */
function fnBody(src, marker, maxLen = 4000) {
  const at = src.indexOf(marker)
  if (at < 0) return null
  const end = src.indexOf('\n}', at)
  const stop = end > 0 ? end + 2 : src.length
  return src.slice(at, Math.min(stop, at + maxLen))
}

/**
 * @param {{protected_ports?: number[]}} cfg 已解析的配置对象
 * @returns {{problems: string[], checks: string[], protectedPorts: number[]}}
 *   `checks` 是"确实核实过的不变量"清单 —— 让调用方照单打印即可。
 *   上一版是调用方自己写死三行 ✔ 文案，这次改不变量时文案就悄悄过期了
 *   （还印着"takeover 入口拦截"），所以把文案收回到判断这一侧，不会再漂移。
 */
export function checkGuards(cfg) {
  const problems = []
  const checks = []
  const protectedPorts = Array.isArray(cfg?.protected_ports) ? cfg.protected_ports : []
  const ok = (label) => checks.push(label)
  const bad = (msg) => problems.push(msg)

  // 1) 空名单 = 保护彻底失效。这是最隐蔽的一种坏法：命令还能跑，只是不再拦人。
  if (!protectedPorts.length) {
    bad('配置里的 protected_ports 为空，保护名单形同不存在')
  } else {
    ok(`名单非空 [${protectedPorts.join(', ')}]`)
  }

  const main = read('src-tauri/src/main.rs')
  if (main == null) {
    bad('读不到 src-tauri/src/main.rs，无法核实 kill_pid 闸门')
  } else {
    // 2) 裸入口：命令体必须把 Blind 传下去。谁把它改成 Project，就等于给
    //    「拿一个裸 PID 就杀」开了后门，而界面和 AI 走的都是这条。
    const entry = fnBody(main, 'fn kill_pid(')
    if (entry == null) bad('main.rs 里找不到 kill_pid')
    else if (!entry.includes('KillScope::Blind')) {
      bad('kill_pid 命令没有以 KillScope::Blind 委托，裸结束可能绕开保护')
    } else {
      ok('kill_pid 裸入口以 KillScope::Blind 委托')
    }

    // 3) 闸门本体：既要存在，又要绑在 Blind 上，还要排在真正动手之前
    const scoped = fnBody(main, 'fn kill_pid_scoped(')
    if (scoped == null) {
      bad('main.rs 里找不到 kill_pid_scoped，无法核实闸门')
    } else {
      const gate = scoped.search(/protected_hits\s*\(/)
      const blind = scoped.search(/KillScope::Blind/)
      const realKill = scoped.search(/platform::kill_process\s*\(/)
      if (gate < 0) bad('kill_pid_scoped 里没有 protected_hits 闸门')
      if (blind < 0) {
        bad('kill_pid_scoped 里的闸门没绑定在 KillScope::Blind 上，等于对所有调用都放行')
      }
      if (gate >= 0 && realKill >= 0 && gate > realKill) {
        bad('kill_pid_scoped 的受保护闸门排在 platform::kill_process 之后，拦不住')
      }
      if (gate >= 0 && blind >= 0 && (realKill < 0 || gate < realKill)) {
        ok('kill_pid_scoped 的闸门绑定 Blind，且排在真正杀进程之前')
      }
    }

    // 4) 唯一的放行口：结束范围必须锁死在 rt.pids，且显式声明 Project 上下文
    const takeover = fnBody(main, 'fn takeover_project(')
    if (takeover == null) {
      bad('main.rs 里找不到 takeover_project，无法核实放行范围')
    } else {
      const scopedPids = /rt\.pids/.test(takeover)
      const projectScope = takeover.includes('KillScope::Project')
      const noRawKill = !/platform::kill_process\s*\(/.test(takeover)
      if (!scopedPids) {
        bad('takeover_project 没把结束范围限定在 rt.pids（本项目声明端口上的进程）')
      }
      if (!projectScope) {
        bad('takeover_project 没有以 KillScope::Project 放行，受保护端口上会卡死')
      }
      if (!noRawKill) {
        bad('takeover_project 直接调用了 platform::kill_process，绕过了 kill_pid_scoped 的上下文')
      }
      if (scopedPids && projectScope && noRawKill) {
        ok('takeover_project 结束范围锁定 rt.pids，并以 Project 放行')
      }
    }

    // 5) 关闭窗口不得带走受保护端口上的项目。
    //    这里的坑极隐蔽：任务的 PID 是 `cmd /C` 启动器，它自己不监听任何端口，
    //    所以"只看任务进程自己的端口"必然判空；而结束是 `taskkill /F /T` 连树杀，
    //    于是关个工具就把用户 8000/9528 上的服务一起带走。判定必须同时看项目声明端口。
    const keepFn = fnBody(main, 'fn keep_alive_tasks(')
    if (keepFn == null) {
      bad('main.rs 里找不到 keep_alive_tasks，关闭窗口的保留判定无处核实')
    } else {
      const declared = /runtime_by_ports\s*\(/.test(keepFn)
      const ownPorts = /match_protected\s*\(/.test(keepFn)
      const emptySafe = /protected\.is_empty\(\)/.test(keepFn)
      if (!declared) {
        bad('keep_alive_tasks 只看任务进程自己的端口，认不出 cmd /C 启动器托管的服务（关窗口会带走用户项目）')
      }
      if (!ownPorts) {
        bad('keep_alive_tasks 丢了"任务进程自己就在受保护端口上"这一层')
      }
      if (!emptySafe) {
        bad('keep_alive_tasks 没处理"保护名单为空"的情况，关窗口可能一个任务都不结束')
      }
      if (declared && ownPorts && emptySafe) {
        ok('关闭窗口的保留判定同时看任务自身端口与项目声明端口')
      }
    }

    const closeAt = main.indexOf('CloseRequested')
    if (closeAt < 0) {
      bad('main.rs 里找不到 CloseRequested，无法核实关闭行为')
    } else {
      const body = main.slice(closeAt, closeAt + 2500)
      if (!/keep_alive_tasks\s*\(/.test(body)) {
        bad('CloseRequested 处理没有调用 keep_alive_tasks，可能又退回只按启动器 PID 判定')
      } else {
        ok('CloseRequested 用 keep_alive_tasks 决定保留哪些任务')
      }
    }
  }

  // 6) 控制口的接管分支只能转发，不能自己另起一套杀进程逻辑
  const control = read('src-tauri/src/control.rs')
  if (control == null) {
    bad('读不到 src-tauri/src/control.rs，无法核实 takeover 转发')
  } else {
    const at = control.indexOf('Cmd::TakeoverProject =>')
    if (at < 0) {
      bad('control.rs 里找不到 Cmd::TakeoverProject 分支')
    } else {
      // 截到下一个分支臂为止，够覆盖整个接管流程
      const tail = control.slice(at)
      const nextArm = tail.indexOf('\n        Cmd::')
      const body = nextArm > 0 ? tail.slice(0, nextArm) : tail.slice(0, 2000)
      const forwards = /crate::takeover_project\s*\(/.test(body)
      const noBlindKill = !/kill_pid\s*\(/.test(body)
      if (!forwards) {
        bad('control.rs 的接管分支没有转发到 crate::takeover_project，界面与 AI 会走出两套行为')
      }
      if (!noBlindKill) {
        bad('control.rs 的接管分支里出现了裸 kill_pid 调用，绕过了项目上下文')
      }
      if (forwards && noBlindKill) {
        ok('控制口接管分支转发 crate::takeover_project，未出现裸 kill')
      }
    }
  }

  return { problems, checks, protectedPorts }
}

function read(rel) {
  try {
    return fs.readFileSync(path.join(ROOT, rel), 'utf8')
  } catch {
    return null
  }
}

/** 把 problems 打成带缩进的人话，给脚本直接复用 */
export function formatProblems(problems) {
  return problems.map((p) => `  · ${p}`).join('\n')
}
