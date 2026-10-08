<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import Icon from './Icon.vue'
import Modal from './Modal.vue'
import { api, fmtUptime, fmtTime, parsePortSpec } from '../api'
import {
  store,
  toast,
  saveProjects,
  syncTasks,
  syncScan,
  takeoverProject,
  pushLog,
  logsOf,
  lastTaskOf,
  cleanErr,
} from '../store'

const editing = ref(null) // 项目编辑表单
const logTask = ref(null) // 日志抽屉的任务 id
const preview = ref(null) // 启动前确认（含端口预检 / 脚本来源确认）
const nowTs = ref(Math.floor(Date.now() / 1000))
const starting = ref(null)
const stopping = ref(null)
const resolving = ref(null)
const batchBusy = ref(false)
const batch = ref(null) // { label, total, done, name, failed: [], finished }
const q = ref('')
const groupFilter = ref('')
// 统一确认框：替换原生 confirm()，文案能排版、能列出影响范围
const ask = ref(null) // { title, lines: [], note, confirmText, danger, run }
const askBusy = ref(false)

const COLORS = ['#4f8cff', '#22d3ee', '#a78bfa', '#34d399', '#fbbf24', '#f87171', '#f472b6']
const TRUST_KEY = 'dk.trustedScripts'
const KIND_LABEL = { command: '命令行', script: '脚本' }

// ---------------- 派生数据 ----------------

const runningByProject = computed(() => {
  const m = {}
  for (const t of Object.values(store.tasks)) {
    // 已退出的任务不参与"运行中"判断，避免进程死了仍显示运行中
    if (t.status === 'exited') continue
    m[t.project.id] = t
  }
  return m
})

// ---- 运行态统一取自 store.runtime（后端 scan_ports 产出），
// ---- 判定覆盖「本应用任务 / 声明端口正在监听 / 监听进程目录命中项目目录」三种情况，
// ---- 因此终端里手工起的服务同样会被识别。

function rtOf(p) {
  return store.runtime[p.id] || null
}

/** 是否在跑（不论谁启动的） */
function isRunning(p) {
  return !!(rtOf(p) && rtOf(p).running)
}

/** 在跑但不是本应用启动的（例如在终端里手工起） */
function isExternal(p) {
  return !!(rtOf(p) && rtOf(p).running && rtOf(p).external)
}

/** 本应用启动的任务：只有它有日志、能走 stopTask */
function taskOf(p) {
  return runningByProject.value[p.id] || null
}

/** 最近一次任务（含已结束的），用于「日志」入口 */
function lastTask(p) {
  return lastTaskOf(p.id)
}

/** 实际正在监听的端口 */
function livePorts(p) {
  return (rtOf(p)?.ports || []).map(Number)
}

const runningCount = computed(() => store.config.projects.filter((p) => isRunning(p)).length)
const externalCount = computed(() => store.config.projects.filter((p) => isExternal(p)).length)

const groups = computed(() => {
  const s = new Set()
  for (const p of store.config.projects) if (p.group) s.add(p.group)
  return [...s]
})

const list = computed(() => {
  const kw = q.value.trim().toLowerCase()
  return store.config.projects.filter((p) => {
    if (groupFilter.value && p.group !== groupFilter.value) return false
    if (!kw) return true
    return [p.name, p.command, p.script_path, p.cwd, p.note, p.group]
      .filter(Boolean)
      .some((v) => String(v).toLowerCase().includes(kw))
  })
})

let timer = null
onMounted(() => {
  syncTasks()
  timer = setInterval(() => (nowTs.value = Math.floor(Date.now() / 1000)), 1000)
})
onUnmounted(() => clearInterval(timer))

// ---------------- 展示辅助 ----------------

function kindOf(p) {
  return p.kind || 'command'
}

/** 卡片上的简短命令描述 */
function commandText(p) {
  if (kindOf(p) === 'script') {
    const name = String(p.script_path || '').split(/[\\/]/).pop() || p.script_path
    return p.interpreter ? `${p.interpreter} ${name}` : name
  }
  return p.command
}

/** 启动确认里的完整命令（解释器 + 脚本路径 + 参数） */
function fullCommand(p) {
  if (kindOf(p) !== 'script') return p.command
  const interp = String(p.interpreter || '').trim() || '（按扩展名自动识别）'
  const args = String(p.args || '').trim()
  return `${interp} "${p.script_path}"${args ? ' ' + args : ''}`
}

/** 端口列表：与后端同一套解析语义（含区间），避免前后端各认一套 */
function portList(p) {
  return parsePortSpec(p.ports)
}

function envOf(p) {
  return Array.isArray(p.env) ? p.env.filter((e) => e && e.key) : []
}

/** 卡片上"最近一次任务"的日志入口是否可用 */
function hasLogs(p) {
  return !!lastTask(p)
}

// ---------------- 统一确认框 ----------------

function confirmAction(opts) {
  ask.value = opts
}

async function runAsk() {
  const a = ask.value
  if (!a) return
  askBusy.value = true
  try {
    await a.run()
  } catch (e) {
    toast(cleanErr(e), 'err')
  } finally {
    askBusy.value = false
    ask.value = null
  }
}

// ---------------- 表单 ----------------

function blankProject() {
  return {
    id: 'p' + Date.now().toString(36),
    name: '',
    kind: 'command',
    command: '',
    script_path: '',
    interpreter: '',
    args: '',
    cwd: '',
    ports: '',
    group: '',
    env: [],
    color: COLORS[Math.floor(Math.random() * COLORS.length)],
    note: '',
  }
}

function newProject() {
  editing.value = blankProject()
}

function editProject(p) {
  // 兼容 v1 旧数据：用默认值补齐 v2 字段
  editing.value = { ...blankProject(), ...JSON.parse(JSON.stringify(p)) }
  if (!Array.isArray(editing.value.env)) editing.value.env = []
}

function addEnv() {
  editing.value.env.push({ key: '', value: '' })
}

function removeEnv(i) {
  editing.value.env.splice(i, 1)
}

/**
 * 保存项目。
 * 顺序很重要：先体检 → 再落库 → 只有落库成功才改本地状态。
 * 之前是"先改 store 再保存"，保存失败时界面上看起来已经改好了，下次刷新又变回去。
 */
async function saveEdit() {
  const p = editing.value
  if (!p.name.trim()) return toast('请填写项目名称', 'err')
  if (!p.cwd.trim()) return toast('请填写工作目录', 'err')

  p.env = (p.env || []).filter((e) => e.key && e.key.trim())
  p.kind = kindOf(p)

  // 后端体检：把"一启动就会失败"的原因挡在保存之前
  let check = { errors: [], warnings: [] }
  try {
    check = await api.checkProject(p)
  } catch (e) {
    // 体检本身失败不该阻断保存，后端的 save_projects 还会再校验一次
    check = { errors: [], warnings: [] }
  }
  if (check.errors && check.errors.length) {
    return toast(check.errors[0], 'err')
  }

  const warnings = check.warnings || []
  if (warnings.length) {
    return confirmAction({
      title: '保存前请确认',
      lines: warnings,
      note: '这些项目不影响保存，但启动时可能失败。',
      confirmText: '仍然保存',
      run: () => persist(p),
    })
  }
  await persist(p)
}

async function persist(p) {
  const next = [...store.config.projects]
  const i = next.findIndex((x) => x.id === p.id)
  if (i > -1) next[i] = p
  else next.push(p)

  if (await saveProjects(next)) {
    toast('项目已保存')
    editing.value = null
  }
  // 保存失败时不动 store，弹窗保持打开，用户可以直接改
}

function removeProject(p) {
  if (isRunning(p)) {
    return toast(isExternal(p) ? '该项目在外部运行中，请先结束外部进程' : '请先停止该项目再删除', 'err')
  }
  confirmAction({
    title: '删除项目',
    lines: [`「${p.name}」的托管配置将被删除。`, `工作目录 ${p.cwd} 不会被删除。`],
    confirmText: '删除',
    danger: true,
    run: async () => {
      const next = store.config.projects.filter((x) => x.id !== p.id)
      if (await saveProjects(next)) toast('项目已删除')
    },
  })
}

// ---------------- 脚本来源信任（记录已确认的脚本路径） ----------------

function trustedScripts() {
  try {
    const raw = JSON.parse(localStorage.getItem(TRUST_KEY) || '[]')
    return Array.isArray(raw) ? raw : []
  } catch (e) {
    return []
  }
}

function rememberedTrust(path) {
  return !!path && trustedScripts().includes(path)
}

function rememberTrust(path) {
  if (!path) return
  const all = trustedScripts()
  if (!all.includes(path)) {
    all.push(path)
    localStorage.setItem(TRUST_KEY, JSON.stringify(all.slice(-200)))
  }
}

// ---------------- 启动流程 ----------------

async function openPreview(p) {
  if (isRunning(p)) {
    return toast(isExternal(p) ? '该项目在外部运行中，请先结束外部进程' : '该项目已在运行中', 'err')
  }
  preview.value = {
    project: p,
    conflicts: [],
    checking: true,
    trust: false,
    needTrust: kindOf(p) === 'script' && !rememberedTrust(p.script_path),
  }
  try {
    const conflicts = await api.checkPorts(p.ports || '')
    if (preview.value) preview.value.conflicts = conflicts || []
  } catch (e) {
    toast('端口预检失败: ' + cleanErr(e), 'err')
  } finally {
    if (preview.value) preview.value.checking = false
  }
}

async function confirmStart() {
  const pv = preview.value
  if (!pv) return
  if (pv.conflicts.length) return toast('请先释放冲突端口', 'err')
  if (pv.needTrust && !pv.trust) return toast('请先确认脚本来源可信', 'err')
  if (pv.needTrust) rememberTrust(pv.project.script_path)
  const project = pv.project
  preview.value = null
  await doStart(project)
}

async function doStart(p) {
  starting.value = p.id
  try {
    const task = await api.startTask(p)
    if (task && task.id) {
      store.tasks[task.id] = {
        ...task,
        status: task.pid ? 'running' : 'starting',
        logs: [],
      }
      pushLog(task.id, { ts: task.started_at, stream: 'sys', text: `▶ 启动: ${fullCommand(p)}` })
      toast(`「${p.name}」已启动 (PID ${task.pid})`)
    }
    // 极短命的命令可能已经退出，回读一次让界面与后端对齐
    syncTasks()
  } catch (e) {
    toast(cleanErr(e), 'err')
  } finally {
    starting.value = null
  }
}

/** 释放被占用的端口（结束占用进程） */
async function releasePort(c) {
  resolving.value = c.pid
  try {
    const r = await api.killPid(c.pid)
    if (r.ok) toast(`已释放端口 ${c.port}`)
    else toast('释放失败: ' + r.message, 'err')
    const conflicts = await api.checkPorts(preview.value?.project?.ports || '')
    if (preview.value) preview.value.conflicts = conflicts || []
  } catch (e) {
    toast('释放失败: ' + cleanErr(e), 'err')
  } finally {
    resolving.value = null
  }
}

async function stop(p) {
  const t = taskOf(p)
  if (!t) return
  stopping.value = p.id
  try {
    await api.stopTask(t.id)
    toast(`「${p.name}」已停止`)
    await syncTasks()
  } catch (e) {
    // 停止失败也要回读：进程可能早就自己退了，后端会把任务摘掉
    toast(cleanErr(e), 'err')
    await syncTasks()
  } finally {
    stopping.value = null
  }
}

// ---------------- 外部进程处理 ----------------

/** 结束外部进程（非本应用启动，只能按 PID / 进程树结束） */
async function killExternal(p) {
  const r = rtOf(p)
  if (!r || !r.pids.length) return toast('未找到占用进程', 'err')
  confirmAction({
    title: '结束外部进程',
    lines: [
      `项目「${p.name}」的进程不是 DevToolkit 启动的，只能强制结束。`,
      `将结束 PID：${r.pids.join(' / ')}`,
      r.ports.length ? `占用端口：${r.ports.join(' / ')}` : '',
    ].filter(Boolean),
    note: '服务会立即中断，且不会自动重启。',
    confirmText: '结束进程',
    danger: true,
    run: async () => {
      stopping.value = p.id
      try {
        let ok = 0
        for (const pid of r.pids) {
          try {
            const res = await api.killPid(pid)
            if (res.ok) ok++
          } catch (e) {
            /* 单个失败不阻断 */
          }
        }
        if (ok) toast(`已结束 ${ok} 个外部进程`)
        else toast('结束失败，可能需要管理员权限', 'err')
        await syncScan()
      } finally {
        stopping.value = null
      }
    },
  })
}

/** 结束外部进程后立即以托管方式启动（一键接管） */
async function takeOver(p) {
  const r = rtOf(p)
  if (!r) return
  confirmAction({
    title: '结束并接管',
    lines: [
      `将结束「${p.name}」当前的外部进程（PID ${(r.pids || []).join(' / ') || '-'}），`,
      '然后立即以 DevToolkit 托管的方式重新启动它。',
    ],
    note: '服务会中断数秒（重启），期间接口不可用。之后可在本应用看日志、一键停止。',
    confirmText: '重启并接管',
    danger: true,
    run: async () => {
      stopping.value = p.id
      try {
        const task = await takeoverProject(p)
        if (task && task.id) {
          pushLog(task.id, { ts: task.started_at, stream: 'sys', text: `▶ 启动: ${fullCommand(p)}` })
          toast(`「${p.name}」已接管并启动 (PID ${task.pid})`)
        }
      } catch (e) {
        toast(cleanErr(e), 'err')
      } finally {
        stopping.value = null
      }
    },
  })
}

// ---------------- 批量启停（作用于当前筛选结果） ----------------

function finishBatch(label, total, failed) {
  if (!batch.value) return
  batch.value.done = total
  batch.value.finished = true
  if (failed.length) {
    batch.value.failed = failed
    // 有失败时面板留着，让用户看清是哪几个、为什么
    return
  }
  const ok = total
  toast(`已${label} ${ok} 个项目`)
  setTimeout(() => (batch.value = null), 1200)
}

async function startAll() {
  if (batchBusy.value) return
  // 已在运行的（含外部启动）一律跳过，避免抢占端口
  const targets = list.value.filter((p) => !isRunning(p))
  if (!targets.length) return toast('当前没有可启动的项目')
  // 批量不弹确认框，但未确认来源的脚本必须逐个走确认，避免绕过信任检查
  const untrusted = targets.filter((p) => kindOf(p) === 'script' && !rememberedTrust(p.script_path))
  if (untrusted.length) {
    return toast(`有 ${untrusted.length} 个脚本项目尚未确认来源，请单独启动完成确认`, 'err')
  }

  batchBusy.value = true
  batch.value = { label: '启动', total: targets.length, done: 0, name: '', failed: [], finished: false }
  const failed = []
  for (const p of targets) {
    batch.value.name = p.name
    try {
      const task = await api.startTask(p)
      if (task && task.id) {
        store.tasks[task.id] = {
          ...task,
          status: task.pid ? 'running' : 'starting',
          logs: [],
        }
        pushLog(task.id, { ts: task.started_at, stream: 'sys', text: `▶ 启动: ${fullCommand(p)}` })
      }
    } catch (e) {
      failed.push(`${p.name}：${cleanErr(e)}`)
    }
    batch.value.done++
  }
  finishBatch('启动', targets.length, failed)
  batchBusy.value = false
  await syncTasks()
}

async function stopAll() {
  if (batchBusy.value) return
  // 只能停本应用启动的任务；外部进程需在卡片上单独处理
  const targets = list.value.filter((p) => taskOf(p))
  if (!targets.length) {
    const ext = list.value.filter((p) => isExternal(p)).length
    return toast(ext ? `当前 ${ext} 个项目为外部启动，需逐个结束外部进程` : '当前没有运行中的项目', ext ? 'err' : 'ok')
  }

  batchBusy.value = true
  batch.value = { label: '停止', total: targets.length, done: 0, name: '', failed: [], finished: false }
  const failed = []
  for (const p of targets) {
    batch.value.name = p.name
    try {
      await api.stopTask(taskOf(p).id)
    } catch (e) {
      failed.push(`${p.name}：${cleanErr(e)}`)
    }
    batch.value.done++
  }
  finishBatch('停止', targets.length, failed)
  batchBusy.value = false
  await syncTasks()
}

// ---------------- 日志抽屉 ----------------

const logLines = computed(() => logsOf(logTask.value))
const logMeta = computed(() => (logTask.value ? store.tasks[logTask.value] || null : null))
/** 抽屉标题：任务已结束时也要说清是哪个项目的哪一次运行 */
const logTitle = computed(() => {
  const t = logTask.value ? store.tasks[logTask.value] || store.recent.find((x) => x.id === logTask.value) : null
  return t?.project?.name || '任务日志'
})

/**
 * 智能滚动：只有用户本来就在底部时才跟着滚。
 * 之前无条件滚到底，用户往上翻查错误日志时会被每一行新输出拽回底部。
 */
const stickBottom = ref(true)
const logEl = ref(null)

function onLogScroll(e) {
  const el = e.target
  if (!el) return
  stickBottom.value = el.scrollHeight - el.scrollTop - el.clientHeight < 40
}

function bindLogEl(el) {
  logEl.value = el
  if (el) {
    el.scrollTop = el.scrollHeight
    stickBottom.value = true
  }
}

// 新日志到达时，仅在用户"本来就在底部"时才跟随滚动
watch(
  () => logLines.value.length,
  () => {
    const el = logEl.value
    if (el && stickBottom.value) el.scrollTop = el.scrollHeight
  }
)

function openLog(taskId) {
  if (!taskId) return
  logTask.value = taskId
  stickBottom.value = true
}
</script>

<template>
  <div>
    <div class="toolbar">
      <div class="search">
        <Icon name="search" :size="15" />
        <input class="input" v-model="q" placeholder="搜索项目 / 命令 / 目录 / 分组" />
      </div>
      <div class="seg" v-if="groups.length">
        <button :class="{ on: groupFilter === '' }" @click="groupFilter = ''">全部</button>
        <button v-for="g in groups" :key="g" :class="{ on: groupFilter === g }" @click="groupFilter = g">
          {{ g }}
        </button>
      </div>
      <div class="chip">运行中 <b>{{ runningCount }}</b> / {{ store.config.projects.length }}</div>
      <div v-if="externalCount" class="chip ext-chip" title="进程不是本应用启动的">
        外部 <b>{{ externalCount }}</b>
      </div>
      <div style="margin-left: auto; display: flex; gap: 8px">
        <button class="btn sm" :disabled="batchBusy" @click="startAll">
          <Icon name="play" :size="12" /> 全部启动
        </button>
        <button class="btn sm" :disabled="batchBusy" @click="stopAll">
          <Icon name="stop" :size="12" /> 全部停止
        </button>
        <button class="btn primary" @click="newProject">
          <Icon name="plus" :size="14" /> 新建项目
        </button>
      </div>
    </div>

    <!-- 批量操作进度：一次启停几十个项目时，"卡住了还是在跑"必须看得见 -->
    <div v-if="batch" class="batch-panel" :class="{ done: batch.finished }">
      <div class="batch-head">
        <span class="batch-title">
          {{ batch.finished ? `已完成${batch.label}` : `正在${batch.label}` }}
          {{ batch.done }} / {{ batch.total }}
          <span v-if="!batch.finished && batch.name" class="batch-now">· {{ batch.name }}</span>
        </span>
        <button v-if="batch.finished" class="btn sm ghost" @click="batch = null">关闭</button>
      </div>
      <div class="batch-bar">
        <i :style="{ width: (batch.total ? (batch.done / batch.total) * 100 : 0) + '%' }"></i>
      </div>
      <div v-if="batch.failed.length" class="batch-fail">
        <div class="batch-fail-title">{{ batch.failed.length }} 个失败：</div>
        <div v-for="(f, i) in batch.failed" :key="i" class="batch-fail-line">{{ f }}</div>
      </div>
    </div>

    <div v-if="list.length" class="proj-grid">
      <div v-for="p in list" :key="p.id" class="card proj-card">
        <div class="proj-head">
          <div class="dot-c" :style="{ background: p.color }"></div>
          <div class="name" :title="p.name">{{ p.name }}</div>
          <span v-if="p.group" class="tag gray" style="margin-right: 6px"><span class="dot"></span>{{ p.group }}</span>
          <span v-if="isExternal(p)" class="tag ext" title="进程不是本应用启动的（例如在终端里手工起）">
            <span class="dot"></span>运行中 · 外部
          </span>
          <span v-else-if="isRunning(p)" class="tag run"><span class="dot"></span>运行中</span>
          <span v-else class="tag stop"><span class="dot"></span>未运行</span>
        </div>

        <div class="proj-note" v-if="p.note" :title="p.note">{{ p.note }}</div>
        <div class="proj-cmd" :title="fullCommand(p)">
          <span class="kind-tag">{{ KIND_LABEL[kindOf(p)] }}</span>
          {{ commandText(p) }}
        </div>
        <div class="proj-cwd" :title="p.cwd">
          <Icon name="folder" :size="12" style="vertical-align: -2px" /> {{ p.cwd }}
        </div>
        <div v-if="portList(p).length" class="proj-ports">
          <span
            v-for="n in portList(p)"
            :key="n"
            class="port-pill"
            :class="{ on: livePorts(p).includes(Number(n)) }"
            :title="livePorts(p).includes(Number(n)) ? `端口 ${n} 正在监听` : `端口 ${n} 未监听`"
          >
            {{ n }}
          </span>
        </div>
        <div v-if="taskOf(p)" class="proj-cwd" style="color: var(--ok)">
          PID {{ taskOf(p).pid }} · 已运行 {{ fmtUptime(nowTs - taskOf(p).started_at) }}
        </div>
        <div v-else-if="isExternal(p)" class="proj-cwd ext-line">
          <Icon name="terminal" :size="12" style="vertical-align: -2px" />
          外部进程 PID {{ (rtOf(p).pids || []).join(' / ') }}
          <span v-if="rtOf(p).process_name"> · {{ rtOf(p).process_name }}</span>
          <span class="src-hint">{{ rtOf(p).source === 'path' ? '按目录匹配' : '按端口匹配' }}</span>
        </div>

        <div class="proj-foot">
          <div class="status"></div>
          <button
            v-if="!isRunning(p)"
            class="btn primary sm"
            :disabled="starting === p.id"
            @click="openPreview(p)"
          >
            <Icon name="play" :size="12" /> {{ starting === p.id ? '启动中...' : '启动' }}
          </button>
          <template v-else-if="isExternal(p)">
            <button class="btn primary sm" :disabled="stopping === p.id" @click="takeOver(p)">
              <Icon name="refresh" :size="12" /> {{ stopping === p.id ? '处理中...' : '结束并接管' }}
            </button>
            <button class="btn danger sm" :disabled="stopping === p.id" @click="killExternal(p)">
              <Icon name="stop" :size="12" /> 结束
            </button>
          </template>
          <template v-else>
            <button class="btn danger sm" :disabled="stopping === p.id" @click="stop(p)">
              <Icon name="stop" :size="12" /> {{ stopping === p.id ? '停止中...' : '停止' }}
            </button>
          </template>
          <button
            v-if="hasLogs(p)"
            class="btn sm"
            :title="taskOf(p) ? '查看实时日志' : '查看最近一次运行的日志'"
            @click="openLog(lastTask(p).id)"
          >
            <Icon name="terminal" :size="12" /> 日志
            <span v-if="!taskOf(p)" class="log-ended-tag">已结束</span>
          </button>
          <button class="btn sm ghost" title="在资源管理器中打开" @click="api.openInExplorer(p.cwd).catch(() => toast('目录不存在', 'err'))">
            <Icon name="folder" :size="13" />
          </button>
          <button class="btn sm ghost" @click="editProject(p)"><Icon name="edit" :size="13" /></button>
          <button class="btn sm ghost" @click="removeProject(p)"><Icon name="trash" :size="13" /></button>
        </div>
      </div>
    </div>

    <div v-else class="card">
      <div class="empty">
        <div class="icon">📁</div>
        <p v-if="store.config.projects.length">没有匹配的项目</p>
        <p v-else>还没有托管任何项目</p>
        <p class="hint">
          {{ store.config.projects.length ? '尝试更换关键字或分组筛选' : '新建项目后，即可在这里一键启动 / 停止 / 查看实时日志' }}
        </p>
        <button class="btn primary" style="margin-top: 16px" @click="newProject">
          <Icon name="plus" :size="14" /> 新建项目
        </button>
      </div>
    </div>

    <!-- 启动确认：执行预览 + 端口预检 + 脚本来源确认 -->
    <Modal :open="!!preview" @close="preview = null" title="启动确认" width="640">
      <div v-if="preview">
        <div class="pv-label">将要执行的命令</div>
        <div class="term-panel" style="margin-bottom: 14px">
          <div class="out" style="word-break: break-all">{{ fullCommand(preview.project) }}</div>
        </div>

        <div class="pv-row">
          <span>启动方式</span>
          <b>{{ KIND_LABEL[kindOf(preview.project)] }}</b>
        </div>
        <div class="pv-row">
          <span>工作目录</span>
          <code>{{ preview.project.cwd }}</code>
        </div>
        <div class="pv-row" v-if="envOf(preview.project).length">
          <span>环境变量</span>
          <div>
            <div v-for="(e, i) in envOf(preview.project)" :key="i">
              <code>{{ e.key }}={{ e.value }}</code>
            </div>
          </div>
        </div>

        <template v-if="portList(preview.project).length">
          <div class="pv-label" style="margin-top: 16px">端口预检</div>
          <div v-if="preview.checking" class="pv-muted">检测中...</div>
          <div v-else-if="!preview.conflicts.length" class="pv-ok">
            端口 {{ portList(preview.project).join(' / ') }} 均空闲，可以启动
          </div>
          <div v-else>
            <div v-for="c in preview.conflicts" :key="c.port" class="pv-conflict">
              <span>
                端口 <b>{{ c.port }}</b> 被 <b>{{ c.process_name }}</b>（PID {{ c.pid }}）占用
              </span>
              <button class="btn sm danger" :disabled="resolving === c.pid" @click="releasePort(c)">
                {{ resolving === c.pid ? '释放中...' : '释放该端口' }}
              </button>
            </div>
          </div>
        </template>

        <label v-if="preview.needTrust" class="pv-trust">
          <input type="checkbox" v-model="preview.trust" />
          <span>我确认该脚本来源可信，首次执行后不再询问（{{ preview.project.script_path }}）</span>
        </label>

        <div class="pv-note">
          将以当前用户身份运行，不申请管理员权限；停止时会结束整个进程树。
        </div>
      </div>
      <template #foot>
        <button class="btn" @click="preview = null">取消</button>
        <button
          class="btn primary"
          :disabled="preview && (preview.checking || preview.conflicts.length || starting === preview.project.id)"
          @click="confirmStart"
        >
          <Icon name="play" :size="13" />
          {{ preview && starting === preview.project.id ? '启动中...' : '确认启动' }}
        </button>
      </template>
    </Modal>

    <!-- 项目编辑 -->
    <Modal
      :open="!!editing"
      @close="editing = null"
      :title="editing && store.config.projects.some((p) => p.id === editing.id) ? '编辑项目' : '新建项目'"
      width="620"
    >
      <div v-if="editing">
        <div class="field">
          <label>启动方式</label>
          <div class="seg">
            <button :class="{ on: kindOf(editing) === 'command' }" @click="editing.kind = 'command'">命令行</button>
            <button :class="{ on: kindOf(editing) === 'script' }" @click="editing.kind = 'script'">脚本文件</button>
          </div>
        </div>

        <div class="field">
          <label>项目名称 <span class="req">*</span></label>
          <input class="input" v-model="editing.name" placeholder="例如：ai-tools 前端" />
        </div>

        <template v-if="kindOf(editing) === 'command'">
          <div class="field">
            <label>启动命令 <span class="req">*</span></label>
            <input class="input mono" v-model="editing.command" placeholder="例如：npm run dev" />
          </div>
        </template>

        <template v-else>
          <div class="field">
            <label>脚本文件路径 <span class="req">*</span></label>
            <input class="input mono" v-model="editing.script_path" placeholder="例如：E:\scripts\build.ps1" />
          </div>
          <div class="field">
            <label>解释器</label>
            <input class="input mono" v-model="editing.interpreter" placeholder="留空自动识别：.bat→cmd，.ps1→powershell，.py→python，.sh→bash，.js→node" />
          </div>
          <div class="field">
            <label>附加参数</label>
            <input class="input mono" v-model="editing.args" placeholder="可选，以空格分隔" />
          </div>
        </template>

        <div class="field">
          <label>工作目录 <span class="req">*</span></label>
          <input class="input mono" v-model="editing.cwd" placeholder="例如：E:\ai_tools\my-project" />
        </div>

        <div class="field">
          <label>检测端口</label>
          <input class="input mono" v-model="editing.ports" placeholder="可选，如 3000,8080；启动前会检测占用，也会用于端口页关联" />
        </div>

        <div class="field">
          <label>分组</label>
          <input class="input" v-model="editing.group" placeholder="可选，如同一业务线的项目归为一组" list="dk-groups" />
          <datalist id="dk-groups">
            <option v-for="g in groups" :key="g" :value="g"></option>
          </datalist>
        </div>

        <div class="field">
          <label>环境变量</label>
          <div v-for="(e, i) in editing.env" :key="i" class="env-row">
            <input class="input mono" v-model="e.key" placeholder="KEY" />
            <input class="input mono" v-model="e.value" placeholder="VALUE" />
            <button class="btn sm ghost" @click="removeEnv(i)"><Icon name="trash" :size="13" /></button>
          </div>
          <button class="btn sm" style="margin-top: 8px" @click="addEnv">
            <Icon name="plus" :size="12" /> 添加变量
          </button>
        </div>

        <div class="field">
          <label>备注</label>
          <input class="input" v-model="editing.note" placeholder="可选" />
        </div>

        <div class="field">
          <label>标识颜色</label>
          <div class="color-pick">
            <div
              v-for="c in COLORS"
              :key="c"
              class="c"
              :class="{ on: editing.color === c }"
              :style="{ background: c }"
              @click="editing.color = c"
            ></div>
          </div>
        </div>
      </div>
      <template #foot>
        <button class="btn" @click="editing = null">取消</button>
        <button class="btn primary" @click="saveEdit">保存</button>
      </template>
    </Modal>

    <!-- 日志抽屉 -->
    <template v-if="logTask">
      <div class="drawer-mask" @click="logTask = null"></div>
      <div class="drawer">
        <div class="modal-head">
          <h3 style="display: flex; align-items: center; gap: 9px">
            <Icon name="terminal" :size="16" />
            {{ logTitle }}
            <span
              class="tag"
              :class="logMeta && logMeta.status === 'running' ? 'run' : 'stop'"
              style="margin-left: 4px"
            >
              <span class="dot"></span>{{ logMeta && logMeta.status === 'running' ? '运行中' : '已结束' }}
            </span>
          </h3>
          <button class="icon-btn" @click="logTask = null">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <path d="M6 6l12 12M18 6L6 18" />
            </svg>
          </button>
        </div>
        <div class="log-view" :ref="bindLogEl" @scroll="onLogScroll">
          <div v-if="!logLines.length" style="color: var(--text-faint); padding: 20px; text-align: center">
            等待输出...
          </div>
          <div
            v-for="(l, i) in logLines"
            :key="i"
            class="log-line"
            :class="l.stream === 'stderr' ? 'err' : l.stream === 'sys' ? 'sys' : ''"
          >
            <span class="ts">{{ fmtTime(l.ts) }}</span>
            <span class="txt">{{ l.text }}</span>
          </div>
        </div>
        <div v-if="!stickBottom" class="log-jump">
          <button class="btn sm" @click="bindLogEl(logEl)">跳到最新</button>
        </div>
      </div>
    </template>

    <!-- 统一确认框：删除项目 / 结束外部进程 / 结束并接管都要先说清后果 -->
    <Modal :open="!!ask" @close="ask = null" :title="ask?.title || '请确认'" width="560">
      <div v-if="ask">
        <div v-for="(l, i) in ask.lines" :key="i" class="ask-line">{{ l }}</div>
        <div v-if="ask.note" class="ask-note" :class="{ danger: ask.danger }">{{ ask.note }}</div>
      </div>
      <template #foot>
        <button class="btn" :disabled="askBusy" @click="ask = null">取消</button>
        <button class="btn" :class="ask?.danger ? 'danger' : 'primary'" :disabled="askBusy" @click="runAsk">
          {{ askBusy ? '处理中...' : ask?.confirmText || '确认' }}
        </button>
      </template>
    </Modal>
  </div>
</template>
