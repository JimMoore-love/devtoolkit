<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue'
import Icon from './Icon.vue'
import Modal from './Modal.vue'
import { api, fmtUptime, fmtBytes, fmtDate, isProtectedPid } from '../api'
import { store, toast, refreshConfig, syncTasks, takeoverProject, cleanErr } from '../store'

const ports = ref([])
const loading = ref(false)
const nowTs = ref(Math.floor(Date.now() / 1000))
const detail = ref(null) // { pid, name, cmdline, loading }
const killTarget = ref(null)
const killing = ref(false)
const mcp = ref(null)
const stopping = ref('') // 正在停止的任务 id，防止连点
const takeoverTarget = ref(null) // 待接管的运行项（外部启动的项目）
const taking = ref('') // 正在接管的运行项 key，防止连点
const portErr = ref('')

/** 给 AI 客户端用的 MCP 配置片段（复制即可粘贴） */
const mcpSnippet = computed(() => {
  const exe = mcp.value?.mcp_exe || ''
  return JSON.stringify(
    { mcpServers: { devtoolkit: { command: exe || '<devtoolkit-mcp 可执行文件路径>', args: [] } } },
    null,
    2
  )
})

const listenCount = computed(() => ports.value.filter((p) => p.state === 'LISTENING').length)

const totalProjects = computed(() => (store.config.projects || []).length)

/**
 * 在跑的项目清单。
 *
 * 运行态统一取自 store.runtime —— 它由全局轮询里的 syncScan（后端 scan_ports）产出，
 * 判定覆盖三种情况：本应用启动的任务 / 项目声明的端口正在监听 / 监听进程目录命中项目目录。
 *
 * 这里不能只数 store.tasks（本应用亲手拉起的进程）：在终端或 ServBay 里手工起的服务
 * 会让首页恒为 0，而项目管理页却显示"运行中"，两边口径不一致。
 */
const runningProjects = computed(() => {
  const out = []
  const seen = new Set()
  for (const p of store.config.projects || []) {
    const rt = store.runtime[p.id]
    if (!rt || !rt.running) continue
    seen.add(p.id)
    // task_id 非空 = 本应用托管（有日志、能停止）；为空 = 外部启动（只能接管）
    const taskId = rt.task_id || ''
    out.push({
      key: p.id,
      name: p.name,
      color: p.color,
      managed: !!taskId,
      taskId,
      task: taskId ? store.tasks[taskId] || null : null,
      ports: rt.ports || [],
      pids: rt.pids || [],
      processName: rt.process_name || '',
      project: p,
    })
  }
  // 项目被删掉之后残留的任务仍在跑：runtime 里已经没有它，但 store.tasks 还有。
  // 直接丢掉，用户就会在首页看不到一个确实在跑、且他能一键停掉的进程。
  for (const t of Object.values(store.tasks)) {
    if (t.project && seen.has(t.project.id)) continue
    out.push({
      key: 'task:' + t.id,
      name: t.project?.name || '（已删除的项目）',
      color: t.project?.color || '#4f8cff',
      managed: true,
      taskId: t.id,
      task: t,
      ports: [],
      pids: t.pid ? [t.pid] : [],
      processName: '',
      project: null,
    })
  }
  return out
})

const runningCount = computed(() => runningProjects.value.length)
/** 外部启动的个数：这些不能给「停止」（后端没托管它），只能走接管 */
const externalCount = computed(() => runningProjects.value.filter((x) => !x.managed).length)

const memPct = computed(() =>
  store.metrics.mem_total ? (store.metrics.mem_used / store.metrics.mem_total) * 100 : 0
)

const cpuPoints = computed(() => {
  const h = store.cpuHistory
  if (!h.length) return ''
  return h.map((v, i) => `${(i / (h.length - 1 || 1)) * 120},${36 - (v / 100) * 34}`).join(' ')
})
const memPoints = computed(() => {
  const h = store.memHistory
  if (!h.length) return ''
  return h.map((v, i) => `${(i / (h.length - 1 || 1)) * 120},${36 - (v / 100) * 34}`).join(' ')
})

/** AI 操作的动作名 → 中文展示 */
const ACT_LABEL = { start: '启动', stop: '停止', takeover: '接管', kill_pid: '结束进程' }

/** 操作流水不同类型对应图标 */
const FEED_ICON = { kill: 'kill', cmd: 'terminal', task: 'play', mcp: 'bolt' }

const feed = computed(() => {
  const items = []
  // 老三样都要兜底：配置可能来自旧版本，字段缺失时 computed 里抛错会让整个首页白屏
  for (const k of (store.config.kills || []).slice(0, 12)) {
    items.push({ ts: k.ts, type: 'kill', label: `结束进程 ${k.name || '?'} (PID ${k.pid})` })
  }
  for (const h of (store.config.history || []).slice(0, 12)) {
    items.push({ ts: h.ts, type: 'cmd', label: `$ ${h.command}` })
  }
  for (const t of Object.values(store.tasks)) {
    // 项目被删掉之后，残留任务仍然可能在跑；这里不能直接 t.project.name
    items.push({ ts: t.started_at, type: 'task', label: `启动项目 ${t.project?.name || '（已删除的项目）'}` })
  }
  // AI 通过 MCP 做的写操作也要看得见，否则"谁动了我的服务"无从追溯
  for (const a of mcp.value?.recent_audit || []) {
    items.push({
      ts: a.ts,
      type: 'mcp',
      label: `AI ${ACT_LABEL[a.action] || a.action} ${a.target}${a.ok ? '' : '（失败）'}`,
    })
  }
  return items.sort((a, b) => b.ts - a.ts).slice(0, 12)
})

async function loadMcp() {
  try {
    mcp.value = await api.mcpInfo()
  } catch (e) {
    mcp.value = null
  }
}

async function copySnippet() {
  const text = mcpSnippet.value
  try {
    await navigator.clipboard.writeText(text)
    toast('MCP 配置已复制')
  } catch (e) {
    // WebView 里 clipboard API 偶尔不可用，退回选中+execCommand
    const ta = document.createElement('textarea')
    ta.value = text
    ta.style.position = 'fixed'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    let ok = false
    try {
      ok = document.execCommand('copy')
    } catch (_) {
      ok = false
    }
    document.body.removeChild(ta)
    toast(ok ? 'MCP 配置已复制' : '复制失败，请手动选中上方文本', ok ? 'ok' : 'err')
  }
}

async function loadPorts() {
  loading.value = true
  try {
    ports.value = await api.listPorts()
    portErr.value = ''
  } catch (e) {
    portErr.value = cleanErr(e)
    toast('端口扫描失败: ' + portErr.value, 'err')
  } finally {
    loading.value = false
  }
}

let loading_ = false
async function load() {
  if (loading_) return // 首次加载与轮询可能重叠，避免请求堆叠
  loading_ = true
  try {
    // 运行态不在这里取：store.runtime 由全局轮询维护（见 store.js 的 initStore），
    // 首页只负责读，避免每个页面各扫一遍。
    await Promise.all([loadPorts(), refreshConfig(), syncTasks(), loadMcp()])
  } finally {
    loading_ = false
  }
}

/** 停止托管任务：成功后必须给反馈，否则用户点了按钮像是没反应 */
async function stopTask(t) {
  if (stopping.value) return
  stopping.value = t.id
  try {
    const r = await api.stopTask(t.id)
    // 后端约定：stopped=false 表示这个任务不是本应用启动的（或已停）
    if (r && r.stopped === false) {
      toast(`「${t.project?.name || t.id}」不是本应用启动的，无法停止`, 'err')
    } else {
      toast(`已停止 ${t.project?.name || t.id}`)
    }
    await syncTasks()
  } catch (e) {
    toast('停止失败: ' + cleanErr(e), 'err')
  } finally {
    stopping.value = ''
  }
}

/** 托管任务走「停止」；外部启动的只能「接管」—— 后端对后者调 stop_task 会直接拒绝 */
function stopRunning(it) {
  // task 对象缺失时（runtime 与 tasks 短暂不同步）用最小字段兜底，仍能按 id 停
  return stopTask(it.task || { id: it.taskId, project: it.project, pid: it.pids[0] })
}

/** 接管：结束外部进程后以托管方式重启。动作序列在 store 里与项目页共用 */
async function doTakeover() {
  const it = takeoverTarget.value
  if (!it || !it.project) return
  taking.value = it.key
  try {
    const task = await takeoverProject(it.project)
    if (task && task.id) {
      toast(`「${it.name}」已接管并启动 (PID ${task.pid})`)
    } else {
      toast(`「${it.name}」外部进程已结束`)
    }
    await refreshConfig()
    await loadPorts()
  } catch (e) {
    toast('接管失败: ' + cleanErr(e), 'err')
  } finally {
    taking.value = ''
    takeoverTarget.value = null
  }
}

/** AI 可能随时通过 MCP 操作，独立刷新面板与操作流水 */
let mcpBusy = false
async function pollMcp() {
  if (mcpBusy) return
  mcpBusy = true
  try {
    await loadMcp()
  } finally {
    mcpBusy = false
  }
}

let timer = null
let mcpTimer = null
onMounted(() => {
  load()
  // 每秒只推进时钟（运行时长要跟着走）。运行态是 store.runtime 的响应式派生，不需要在这里重算
  timer = setInterval(() => {
    nowTs.value = Math.floor(Date.now() / 1000)
  }, 1000)
  mcpTimer = setInterval(pollMcp, 5000)
})
onUnmounted(() => {
  clearInterval(timer)
  clearInterval(mcpTimer)
})

async function showDetail(p) {
  detail.value = { pid: p.pid, name: p.process_name, cmdline: '', loading: true }
  try {
    const d = await api.processDetail(p.pid)
    detail.value.cmdline = d.cmdline || '（无法获取命令行，可能需要权限）'
  } catch (e) {
    detail.value.cmdline = '获取失败: ' + cleanErr(e)
  } finally {
    detail.value.loading = false
  }
}

function askKill(p) {
  if (isProtectedPid(p.pid)) {
    toast('PID ' + p.pid + ' 属于系统关键进程，应用不会处理', 'err')
    return
  }
  killTarget.value = p
}

async function doKill() {
  const t = killTarget.value
  if (!t) return
  killing.value = true
  try {
    const r = await api.killPid(t.pid)
    if (r.ok) {
      toast(`已结束 ${t.process_name || 'PID ' + t.pid}`)
      await refreshConfig()
      await loadPorts()
    } else {
      toast('结束失败: ' + r.message, 'err')
    }
  } catch (e) {
    toast('结束失败: ' + cleanErr(e), 'err')
  } finally {
    killing.value = false
    killTarget.value = null
  }
}
</script>

<template>
  <div>
    <div class="stat-grid">
      <div class="card stat-card">
        <div class="glow" style="background: #4f8cff"></div>
        <div class="k"><Icon name="bolt" :size="14" /> CPU 使用率</div>
        <div class="v">{{ (store.metrics.cpu || 0).toFixed(0) }}<small>%</small></div>
        <svg viewBox="0 0 120 36" style="width: 100%; height: 36px; margin-top: 6px" preserveAspectRatio="none">
          <defs>
            <linearGradient id="gc" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stop-color="#4f8cff" stop-opacity="0.9" />
              <stop offset="100%" stop-color="#4f8cff" stop-opacity="0.1" />
            </linearGradient>
          </defs>
          <polyline v-if="cpuPoints" :points="cpuPoints" fill="none" stroke="#4f8cff" stroke-width="1.6" />
        </svg>
      </div>
      <div class="card stat-card">
        <div class="glow" style="background: #22d3ee"></div>
        <div class="k"><Icon name="dashboard" :size="14" /> 内存占用</div>
        <div class="v">{{ memPct.toFixed(0) }}<small>%</small></div>
        <div class="bar"><i :style="{ width: memPct + '%', background: 'linear-gradient(90deg,#22d3ee,#4f8cff)' }"></i></div>
        <div class="foot">{{ fmtBytes(store.metrics.mem_used) }} / {{ fmtBytes(store.metrics.mem_total) }}</div>
      </div>
      <div class="card stat-card">
        <div class="glow" style="background: #34d399"></div>
        <div class="k"><Icon name="ports" :size="14" /> 监听端口</div>
        <div class="v">{{ listenCount }}<small>个</small></div>
        <div class="foot">共 {{ ports.length }} 条网络端点记录</div>
      </div>
      <div class="card stat-card">
        <div class="glow" style="background: #a78bfa"></div>
        <div class="k"><Icon name="projects" :size="14" /> 运行中项目</div>
        <div class="v">{{ runningCount }}<small>个</small></div>
        <div class="foot">
          {{ externalCount ? externalCount + ' 个外部启动 · ' : '' }}共 {{ totalProjects }} 个项目已托管
        </div>
      </div>
    </div>

    <div v-if="portErr" class="warn-banner">
      <Icon name="bolt" :size="13" />
      <span>端口扫描失败：{{ portErr }}。下面的端口数据可能是上一次的快照。</span>
    </div>

    <div class="dash-cols">
      <div class="card" style="margin-bottom: 14px">
        <div class="card-title">
          <Icon name="play" :size="14" /> 运行中的项目
          <span class="more" v-if="!runningProjects.length">暂无项目在跑，去「项目管理」启动一个吧</span>
        </div>
        <div v-if="runningProjects.length">
          <div v-for="it in runningProjects" :key="it.key" class="task-row">
            <div class="dot-c" :style="{ background: it.color }"></div>
            <div class="meta">
              <div class="n">
                {{ it.name }}
                <span class="tag" :class="it.managed ? 'run' : 'ext'">
                  <span class="dot"></span>{{ it.managed ? '本应用托管' : '外部启动' }}
                </span>
              </div>
              <div class="s">
                <template v-if="it.task">
                  PID {{ it.task.pid }} · 已运行 {{ fmtUptime(nowTs - it.task.started_at) }}
                </template>
                <template v-else-if="it.managed">由本应用托管，日志见「项目管理」</template>
                <template v-else>
                  <template v-if="it.ports.length">端口 {{ it.ports.join(' / ') }}</template>
                  <template v-if="it.processName">{{ it.ports.length ? ' · ' : '' }}{{ it.processName }}</template>
                  <template v-if="it.pids.length"> · PID {{ it.pids.join(' / ') }}</template>
                </template>
              </div>
            </div>
            <button
              v-if="it.managed"
              class="btn sm danger"
              :disabled="stopping === it.taskId"
              @click="stopRunning(it)"
            >
              <Icon name="stop" :size="12" /> {{ stopping === it.taskId ? '停止中' : '停止' }}
            </button>
            <button
              v-else
              class="btn sm"
              :disabled="taking === it.key"
              @click="takeoverTarget = it"
            >
              <Icon name="refresh" :size="12" /> {{ taking === it.key ? '接管中' : '接管' }}
            </button>
          </div>
        </div>
        <div v-else class="empty" style="padding: 30px 10px">
          <p>当前没有运行中的项目</p>
        </div>
      </div>

      <div style="display: flex; flex-direction: column; gap: 14px">
        <div class="card">
          <div class="card-title"><Icon name="doc" :size="14" /> 最近操作</div>
          <div class="feed">
            <div v-if="!feed.length" class="empty" style="padding: 20px 10px"><p>暂无操作记录</p></div>
            <div v-for="(f, i) in feed" :key="i" class="feed-item">
              <div class="fi" :class="f.type">
                <Icon :name="FEED_ICON[f.type] || 'play'" :size="13" />
              </div>
              <div class="body">
                <div class="l1">{{ f.label }}</div>
                <div class="l2">{{ fmtDate(f.ts) }}</div>
              </div>
            </div>
          </div>
        </div>

        <div class="card" v-if="store.sysinfo">
          <div class="card-title"><Icon name="info" :size="14" /> 系统信息</div>
          <div class="sysinfo-list">
            <div class="row"><span>主机名</span><span>{{ store.sysinfo.hostname || '-' }}</span></div>
            <div class="row"><span>系统</span><span>{{ store.sysinfo.os || '-' }}</span></div>
            <div class="row"><span>逻辑核心</span><span>{{ store.sysinfo.cores }}</span></div>
            <div class="row"><span>运行时长</span><span>{{ fmtUptime(store.metrics.uptime) }}</span></div>
            <div class="row" style="grid-column: 1 / -1"><span>CPU</span><span>{{ store.sysinfo.cpu_brand }}</span></div>
          </div>
        </div>

        <div class="card" v-if="mcp">
          <div class="card-title">
            <Icon name="bolt" :size="14" /> AI 接入（MCP）
            <span class="tag" :class="mcp.enabled ? 'run' : 'stop'">
              <span class="dot"></span>{{ mcp.enabled ? '控制口已开启' : '控制口未开启' }}
            </span>
          </div>
          <div class="sysinfo-list">
            <div class="row">
              <span>控制口</span>
              <span>{{ mcp.enabled ? '127.0.0.1:' + mcp.port : '-' }}</span>
            </div>
            <div class="row">
              <span>MCP 服务</span>
              <span>{{ mcp.mcp_exe_found ? '已就绪' : '未构建' }}</span>
            </div>
            <div class="row">
              <span>AI 调用</span>
              <span>{{ mcp.requests }} 次 / 失败 {{ mcp.errors }}</span>
            </div>
            <div class="row">
              <span>审计记录</span>
              <span>{{ mcp.audit_count }} 条</span>
            </div>
          </div>
          <div style="font-size: 12px; color: var(--text-dim); margin: 10px 0 6px">
            把下面的配置加到 AI 客户端的 MCP 设置里，AI 就能直接查端口、看运行态、启停项目。
            使用期间请保持本应用运行（AI 的所有操作都经由这里，关闭即断开）。
          </div>
          <div class="term-panel">
            <div class="out" style="font-size: 11.5px; white-space: pre-wrap">{{ mcpSnippet }}</div>
          </div>
          <div style="display: flex; gap: 8px; margin-top: 10px">
            <button class="btn sm" @click="copySnippet"><Icon name="doc" :size="12" /> 复制 MCP 配置</button>
            <button class="btn sm" @click="loadMcp"><Icon name="refresh" :size="12" /> 刷新</button>
          </div>
        </div>
      </div>
    </div>

    <Modal :open="!!killTarget" @close="killTarget = null" :title="'结束进程'" width="440">
      <div style="font-size: 13.5px; line-height: 1.8" v-if="killTarget">
        确认结束进程 <b style="color: var(--danger)">{{ killTarget.process_name || '未知进程' }}</b>
        （PID: <span class="num" style="font-family: var(--mono)">{{ killTarget.pid }}</span>）？
        <div style="color: var(--text-faint); font-size: 12px; margin-top: 8px">
          该进程及其子进程树将被强制终止，运行中的服务会立即停止。
        </div>
      </div>
      <template #foot>
        <button class="btn" @click="killTarget = null">取消</button>
        <button class="btn danger" :disabled="killing" @click="doKill">
          <Icon name="kill" :size="13" /> {{ killing ? '结束中...' : '强制结束' }}
        </button>
      </template>
    </Modal>

    <Modal :open="!!takeoverTarget" @close="takeoverTarget = null" :title="'结束并接管'" width="460">
      <div style="font-size: 13.5px; line-height: 1.8" v-if="takeoverTarget">
        将结束「<b>{{ takeoverTarget.name }}</b>」当前的外部进程
        （PID <span class="num" style="font-family: var(--mono)">{{ takeoverTarget.pids.join(' / ') || '-' }}</span>），
        然后立即以 DevToolkit 托管的方式重新启动它。
        <div style="color: var(--text-faint); font-size: 12px; margin-top: 8px">
          服务会中断数秒（重启），期间接口不可用。之后可在本应用看日志、一键停止。
        </div>
      </div>
      <template #foot>
        <button class="btn" @click="takeoverTarget = null">取消</button>
        <button class="btn danger" :disabled="!!taking" @click="doTakeover">
          <Icon name="refresh" :size="13" /> {{ taking ? '接管中...' : '重启并接管' }}
        </button>
      </template>
    </Modal>

    <Modal :open="!!detail" @close="detail = null" :title="'进程详情'" width="560">
      <div v-if="detail">
        <div class="sysinfo-list">
          <div class="row"><span>进程名</span><span>{{ detail.name }}</span></div>
          <div class="row"><span>PID</span><span>{{ detail.pid }}</span></div>
        </div>
        <div style="margin-top: 14px">
          <div style="font-size: 12px; color: var(--text-dim); margin-bottom: 6px">完整命令行</div>
          <div class="term-panel">
            <div v-if="detail.loading" style="color: var(--text-faint)">读取中...</div>
            <div v-else class="out" style="word-break: break-all">{{ detail.cmdline }}</div>
          </div>
        </div>
      </div>
      <template #foot>
        <button class="btn" @click="detail = null">关闭</button>
      </template>
    </Modal>
  </div>
</template>
