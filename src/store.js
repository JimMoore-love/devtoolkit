import { reactive } from 'vue'
import { api, onEvent } from './api'

/** 已结束任务在内存里保留多少个（供停止后回看日志） */
const RECENT_CAP = 20
/** 后端可用时的轮询间隔 */
const POLL_BASE_MS = 5000
/** 连续失败时的退避上限：后端没了还每 5 秒撞一次只会刷屏 */
const POLL_MAX_MS = 30000

export const store = reactive({
  page: localStorage.getItem('dk.page') || 'dashboard',
  metrics: { cpu: 0, mem_total: 0, mem_used: 0, uptime: 0 },
  cpuHistory: [],
  memHistory: [],
  sysinfo: null,
  config: { projects: [], history: [], kills: [], protected_ports: [] },
  tasks: {}, // id -> { id, project, pid, started_at, status, logs: [] }
  // 已结束的托管任务：任务一结束就从 tasks 移到这里，日志跟着走。
  // 以前直接丢掉，用户点完"停止"就再也看不到刚才的输出，排查问题恰恰需要那几行。
  recent: [],
  // 统一运行态：一次扫描同时得到「全量端口」与「项目运行态」，端口页 / 项目管理页共用
  ports: [],
  runtime: {}, // projectId -> { running, external, task_id, source, ports, pids, process_name }
  autoScan: true, // 端口页「自动刷新」开关
  // 与后端失联的原因。非空时界面必须显式提示"看到的数据可能是旧的"，
  // 而不是继续展示上一次的快照假装一切正常
  lastError: '',
  pollFails: 0,
  lastScanAt: 0,
  toasts: [],
})

let toastSeq = 0
export function toast(message, type = 'ok') {
  const id = ++toastSeq
  store.toasts.push({ id, message, type })
  setTimeout(() => {
    const i = store.toasts.findIndex((t) => t.id === id)
    if (i > -1) store.toasts.splice(i, 1)
  }, 3400)
}

/** 剥掉 Tauri invoke 抛出的 "Error: " 前缀，避免 toast 里出现两遍"失败" */
export function cleanErr(e) {
  const s = e && e.message ? e.message : String(e)
  return s.replace(/^(Error|错误)[:：]\s*/, '').trim() || '未知错误'
}

function markOk() {
  store.lastError = ''
  store.pollFails = 0
}

function markFail(e) {
  store.pollFails += 1
  store.lastError = cleanErr(e)
}

export function setPage(p) {
  store.page = p
  localStorage.setItem('dk.page', p)
}

function ensureTask(id) {
  if (!store.tasks[id]) {
    store.tasks[id] = {
      id,
      project: { id: '', name: '未知任务', color: '#4f8cff' },
      pid: 0,
      started_at: 0,
      status: 'running',
      logs: [],
    }
  }
  return store.tasks[id]
}

/** 把任务挪进"最近结束"列表，日志一并保留 */
function archiveTasks(list, status) {
  if (!list.length) return
  const next = list.map((t) => ({ ...t, status, ended_at: Math.floor(Date.now() / 1000) }))
  const ids = new Set(next.map((t) => t.id))
  store.recent = [...next, ...store.recent.filter((x) => !ids.has(x.id))].slice(0, RECENT_CAP)
}

export function pushLog(taskId, entry) {
  const t = ensureTask(taskId)
  t.logs.push(entry)
  if (t.logs.length > 3000) t.logs.splice(0, t.logs.length - 3000)
}

/** 读取任务信息：先看在跑的任务，再落到"最近结束"里 */
export function taskInfo(taskId) {
  if (!taskId) return null
  return store.tasks[taskId] || store.recent.find((x) => x.id === taskId) || null
}

/** 读取任务日志。任务结束后仍然读得到，这是日志抽屉能回看的前提。 */
export function logsOf(taskId) {
  const t = taskInfo(taskId)
  return t ? t.logs || [] : []
}

/** 某项目最近一次任务（优先在跑的），用于"日志"按钮 */
export function lastTaskOf(projectId) {
  const running = Object.values(store.tasks).find(
    (t) => t.project.id === projectId && t.status !== 'exited'
  )
  if (running) return running
  return store.recent.find((t) => t.project.id === projectId) || null
}

export async function syncTasks() {
  try {
    const tasks = await api.listTasks()
    const map = {}
    for (const t of tasks) {
      const old = store.tasks[t.id]
      map[t.id] = {
        ...t,
        // pid=0 是后端"已登记、进程还没起来"的占位态，界面上要区别对待
        status: t.pid ? 'running' : 'starting',
        logs: old ? old.logs : [],
      }
    }
    // 后端已不再持有的任务不要直接丢：它们身上挂着用户可能正在看的日志
    archiveTasks(
      Object.values(store.tasks).filter((t) => !map[t.id]),
      'exited'
    )
    store.tasks = map
    markOk()
  } catch (e) {
    markFail(e)
  }
}

/**
 * 扫描端口 + 项目运行态（唯一数据源）。
 * 项目「在跑」的判定不再只看本应用启动的任务，而是三种情况任一命中：
 *   1. 本应用启动的任务
 *   2. 项目声明的端口正在被监听
 *   3. 正在监听的进程，其工作目录 / 命令行命中项目目录
 * 这样在终端里手工启动的服务同样能被识别并关联到项目。
 */
export async function syncScan() {
  try {
    const r = await api.scanPorts(store.config.projects)
    store.ports = (r && r.ports) || []
    const m = {}
    for (const x of (r && r.runtime) || []) m[x.id] = x
    store.runtime = m
    store.lastScanAt = Date.now()
    markOk()
  } catch (e) {
    markFail(e)
  }
}

/**
 * 结束项目当前的外部占用进程，并立即以托管方式重启（一键接管）。
 *
 * 只做动作、不做确认：接管会中断服务，而首页与项目页要给用户看的措辞不同，
 * 确认框留给调用方。
 *
 * 「结束 → 等端口释放 → 重启」整条序列在后端的 `takeover_project` 里执行 ——
 * 只有在后端才看得到"这些 PID 占的正是本项目声明的端口"这个上下文，受保护端口的
 * 闸门才不会把接管一起拦掉。早先这里是前端自己逐个 `killPid`：受保护端口上后端会
 * 拒绝，而拒绝又被 `catch` 静默吞掉，用户最后只看到一句莫名其妙的"端口冲突"。
 * 失败现在直接抛出，由调用方展示。
 *
 * 返回新起的托管任务。
 */
export async function takeoverProject(p) {
  try {
    const r = await api.takeoverTask(p)
    const task = (r && r.task) || null
    if (task && task.id) {
      store.tasks[task.id] = { ...task, status: task.pid ? 'running' : 'starting', logs: [] }
    }
    return task
  } finally {
    // 极短命的命令可能已经退出，回读一次让界面与后端对齐
    await syncScan()
    syncTasks()
  }
}

export async function refreshConfig() {
  try {
    store.config = await api.getConfig()
    markOk()
    return true
  } catch (e) {
    markFail(e)
    return false
  }
}

/**
 * 保存项目列表。
 * 传入 `list` 而不是直接改 store：保存失败时必须原样保留旧配置，
 * 否则"存不上"会在界面上表现成"改好了"，下次刷新又变回去。
 */
export async function saveProjects(list) {
  const next = list || store.config.projects
  try {
    await api.saveProjects(next)
    store.config.projects = next
    return true
  } catch (e) {
    toast('保存失败：' + cleanErr(e), 'err')
    return false
  }
}

/**
 * 端口是否受保护。名单的真相来源只有 config 一份（后端 `protected_ports`），
 * 前后端各存一份是这类"开关"最容易走偏的地方。
 */
export function isPortProtected(port) {
  return (store.config.protected_ports || []).includes(Number(port))
}

/**
 * 切换端口保护状态。
 * 成功后用后端返回的名单整体覆盖，不做本地先改再由后端兜底——
 * 那样保存失败会在界面上表现成"改好了"，下次刷新又变回去。
 */
export async function setPortProtected(port, enabled) {
  try {
    const list = await api.setPortProtected(Number(port), !!enabled)
    store.config.protected_ports = list || []
    return true
  } catch (e) {
    toast('保护设置失败：' + cleanErr(e), 'err')
    return false
  }
}

let inited = false
let pollTimer = null
export async function initStore() {
  if (inited) return
  inited = true

  try {
    store.sysinfo = await api.sysInfo()
  } catch (e) {
    /* 首屏拿不到系统信息不影响使用 */
  }
  await refreshConfig()
  await syncTasks()
  await syncScan()

  // 全局唯一的运行态轮询：端口页与项目管理页共用，避免各页各扫一遍。
  // 失败时指数退避——后端挂掉后仍每 5 秒重试只是无意义的刷屏。
  const tick = async () => {
    if (store.autoScan) {
      await Promise.all([syncScan(), syncTasks()])
    }
    const backoff = Math.min(POLL_BASE_MS * 2 ** Math.min(store.pollFails, 3), POLL_MAX_MS)
    pollTimer = setTimeout(tick, store.pollFails ? backoff : POLL_BASE_MS)
  }
  pollTimer = setTimeout(tick, POLL_BASE_MS)

  onEvent('sys-metrics', (m) => {
    store.metrics = m
    store.cpuHistory.push(m.cpu)
    if (store.cpuHistory.length > 60) store.cpuHistory.shift()
    const memPct = m.mem_total ? (m.mem_used / m.mem_total) * 100 : 0
    store.memHistory.push(memPct)
    if (store.memHistory.length > 60) store.memHistory.shift()
  })

  onEvent('task-log', (e) => {
    pushLog(e.taskId, { ts: e.ts, stream: e.stream, text: e.text })
  })

  onEvent('task-status', (e) => {
    const t = store.tasks[e.taskId]
    if (!t) return
    t.status = 'exited'
    pushLog(e.taskId, {
      ts: e.ts,
      stream: 'sys',
      text: e.code === 0 ? '✔ 进程已退出（代码 0）' : `✖ 进程已退出（代码 ${e.code}）`,
    })
    // 立刻归档而不是等下一次轮询：否则这段时间里"日志"按钮会短暂消失
    archiveTasks([t], 'exited')
    delete store.tasks[e.taskId]
  })
}

/** 组件卸载时用于清理（目前只有轮询，留一个出口避免以后忘记） */
export function stopPolling() {
  if (pollTimer) clearTimeout(pollTimer)
  pollTimer = null
}
