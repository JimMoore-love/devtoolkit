<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue'
import Icon from './Icon.vue'
import { api, fmtDate } from '../api'
import { toast, cleanErr } from '../store'

const info = ref(null) // mcp_info：控制口状态与统计
const clients = ref(null) // { mcp_exe, clients: [...] }
const selftest = ref(null) // 自检结果
const audit = ref(null) // { total, shown, items }
const busy = ref('') // 正在执行的动作 key，防连点
const portInput = ref('')
const pageErr = ref('')

/** AI 操作的动作名 → 中文展示（与仪表盘流水口径一致） */
const ACT_LABEL = { start: '启动', stop: '停止', takeover: '接管', kill_pid: '结束进程' }

/** 自检失败时，把"断在哪一层"翻译成下一步该怎么办。
 *  只说"失败了"是没用的 —— 用户要的是动作。 */
const STAGE_HINT = {
  binary: '构建产物里没有 devtoolkit-mcp 可执行文件。执行 cargo build --release 会与主程序一起产出。',
  spawn: '找到了文件但起不来，通常是被安全软件拦下或文件不完整，可尝试重新构建。',
  timeout: '进程起来了却不应答，可能是它并非 MCP 服务（拿错文件），或启动时卡住。',
  protocol: '进程有输出但不是合法 MCP 响应，检查是否混入了日志输出到标准输出。',
  tools: '握手成功但没有任何工具，说明工具注册表被清空了，服务端实现有问题。',
}

const snippet = computed(() => {
  const exe = clients.value?.mcp_exe || info.value?.mcp_exe || ''
  return JSON.stringify(
    { mcpServers: { devtoolkit: { command: exe || '<devtoolkit-mcp 可执行文件路径>', args: [] } } },
    null,
    2
  )
})

const enabled = computed(() => !!info.value?.enabled)
const exePath = computed(() => clients.value?.mcp_exe || info.value?.mcp_exe || '')
const exeReady = computed(() => !!exePath.value)

/** 端口是否发生顺延：用户期望的与实际绑定的不一致时要显式提示 */
const portShifted = computed(() => {
  const p = info.value?.preferred_port
  return !!p && p !== info.value?.port
})

async function copy(text, msg) {
  try {
    await navigator.clipboard.writeText(text)
    toast(msg, 'ok')
  } catch {
    // WebView 里 clipboard 可能被拒，退回选中提示（与其它页面一致的处理）
    toast('复制失败，请手动选中文本', 'err')
  }
}

async function loadInfo() {
  try {
    info.value = await api.mcpInfo()
    if (!portInput.value) portInput.value = String(info.value.preferred_port || info.value.port || '')
  } catch (e) {
    pageErr.value = cleanErr(e)
  }
}

async function loadClients() {
  try {
    clients.value = await api.mcpClientsDetect()
  } catch (e) {
    pageErr.value = cleanErr(e)
  }
}

async function loadAudit() {
  try {
    audit.value = await api.mcpAuditList(500)
  } catch (e) {
    pageErr.value = cleanErr(e)
  }
}

async function runSelftest() {
  if (busy.value) return
  busy.value = 'selftest'
  selftest.value = null
  try {
    selftest.value = await api.mcpSelftest()
    if (selftest.value.ok) {
      toast(`自检通过：${selftest.value.tool_count} 个工具可用`, 'ok')
    } else {
      toast(`自检失败于「${selftest.value.stage}」`, 'err')
    }
  } catch (e) {
    toast(cleanErr(e), 'err')
  } finally {
    busy.value = ''
  }
}

async function writeClient(c) {
  if (busy.value) return
  busy.value = 'write:' + c.id
  try {
    const r = await api.mcpClientWrite(c.id)
    const kept = r.preserved?.length ? `，保留了原有 ${r.preserved.join('、')}` : ''
    toast(`已写入 ${r.name}${kept}`, 'ok')
    await loadClients()
  } catch (e) {
    toast(cleanErr(e), 'err')
  } finally {
    busy.value = ''
  }
}

async function toggleControl() {
  if (busy.value) return
  const next = !enabled.value
  busy.value = 'toggle'
  try {
    const r = await api.mcpControlSet(next)
    toast(next ? `控制口已在 ${r.port} 开启` : '控制口已关闭，AI 侧将无法访问', 'ok')
    await loadInfo()
  } catch (e) {
    toast(cleanErr(e), 'err')
  } finally {
    busy.value = ''
  }
}

async function applyPort() {
  if (busy.value) return
  const n = Number(portInput.value)
  if (!Number.isInteger(n) || n < 1024 || n > 65535) {
    toast('端口需在 1024–65535 之间', 'err')
    return
  }
  busy.value = 'port'
  try {
    const r = await api.mcpPortSet(n)
    // 顺延必须说出来，否则用户会以为设置没生效
    toast(
      r.shifted
        ? `请求 ${r.requested} 已被占用，实际监听 ${r.port}`
        : `控制口端口已改为 ${r.port}`,
      'ok'
    )
    await loadInfo()
  } catch (e) {
    toast(cleanErr(e), 'err')
  } finally {
    busy.value = ''
  }
}

async function resetToken() {
  if (busy.value) return
  busy.value = 'token'
  try {
    const r = await api.mcpTokenReset()
    toast(`令牌已更换（${r.token_len} 位），当前存活的 MCP 进程需重开会话`, 'ok')
    await loadInfo()
  } catch (e) {
    toast(cleanErr(e), 'err')
  } finally {
    busy.value = ''
  }
}

async function clearAudit() {
  if (busy.value) return
  busy.value = 'audit'
  try {
    const n = await api.mcpAuditClear()
    toast(`已清空 ${n} 条审计记录`, 'ok')
    await loadAudit()
  } catch (e) {
    toast(cleanErr(e), 'err')
  } finally {
    busy.value = ''
  }
}

// 客户端状态可能被外部改动（客户端自己也会写这个文件），进页面时刷一次
onMounted(() => {
  loadInfo()
  loadClients()
  loadAudit()
})

/** AI 随时可能通过 MCP 操作，状态与审计独立轮询。
 *  带 in-flight guard：上轮没回来就跳过，避免慢请求堆积。 */
let timer = null
let polling = false
onMounted(() => {
  timer = setInterval(async () => {
    if (polling || busy.value) return
    polling = true
    try {
      await loadInfo()
      await loadAudit()
    } finally {
      polling = false
    }
  }, 5000)
})
onUnmounted(() => clearInterval(timer))
</script>

<template>
  <div class="mcp-page">
    <div v-if="pageErr" class="mcp-banner">
      <Icon name="alert" :size="13" /> {{ pageErr }}
    </div>

    <div class="mcp-stats">
      <div class="card stat-card">
        <div class="card-title"><Icon name="bolt" :size="14" /> 控制口</div>
        <div class="stat-val">
          <span class="tag" :class="enabled ? 'run' : 'stop'">
            <span class="dot"></span>{{ enabled ? '已开启' : '已关闭' }}
          </span>
        </div>
      </div>

      <div class="card stat-card">
        <div class="card-title"><Icon name="route" :size="14" /> 监听地址</div>
        <div class="stat-val mono">{{ enabled ? '127.0.0.1:' + info.port : '—' }}</div>
        <div v-if="portShifted" class="stat-note warn">
          你期望 {{ info.preferred_port }}，被占用后顺延
        </div>
      </div>

      <div class="card stat-card">
        <div class="card-title"><Icon name="ping" :size="14" /> AI 调用</div>
        <div class="stat-val mono">{{ info ? info.requests : '—' }} 次</div>
        <div class="stat-note" :class="info && info.errors ? 'bad' : ''">
          失败 {{ info ? info.errors : '—' }} 次
        </div>
      </div>

      <div class="card stat-card">
        <div class="card-title"><Icon name="clock" :size="14" /> 审计记录</div>
        <div class="stat-val mono">{{ info ? info.audit_count : '—' }} 条</div>
        <div class="stat-note">写操作全部留痕</div>
      </div>
    </div>

    <div class="card">
      <div class="card-title">
        <Icon name="power" :size="14" /> 控制口管控
        <span class="more">关掉它 AI 就完全无法操作本机服务，界面手工操作不受影响；重启应用会恢复开启</span>
      </div>

      <div class="mcp-actions">
        <button class="btn" :disabled="!!busy" @click="toggleControl">
          <Icon name="power" :size="12" /> {{ enabled ? '关闭控制口' : '开启控制口' }}
        </button>

        <div class="mcp-inline">
          <span class="lbl">起始端口</span>
          <input
            class="mcp-input"
            v-model="portInput"
            placeholder="9527"
            inputmode="numeric"
          />
          <button class="btn sm" :disabled="!!busy" @click="applyPort">应用</button>
        </div>

        <button class="btn danger" :disabled="!!busy" @click="resetToken">
          <Icon name="refresh" :size="12" /> 重置访问令牌
        </button>
      </div>

      <div class="mcp-exe">
        <span class="lbl">MCP 服务</span>
        <code v-if="exeReady">{{ exePath }}</code>
        <span v-else class="bad">未找到 —— 请执行 cargo build --release，它与主程序一起产出</span>
      </div>
      <div class="mcp-note">
        控制口只监听回环地址并要求令牌，本机其它程序无法随意驱动本应用。
        改端口或重置令牌都会短暂断开已连接的 AI 会话，稍后自动重连。
      </div>
    </div>

    <div class="card">
      <div class="card-title">
        <Icon name="doc" :size="14" /> 客户端接入
        <span class="more">写入只做合并，不动你的其它 server</span>
      </div>

      <div v-if="!clients" class="mcp-note">检测中…</div>
      <div v-else class="client-list">
        <div v-for="c in clients.clients" :key="c.id" class="client">
          <div class="client-head">
            <strong>{{ c.name }}</strong>
            <span v-if="c.broken" class="tag bad-tag"><span class="dot"></span>配置损坏</span>
            <span v-else-if="!c.exists" class="tag gray"><span class="dot"></span>未使用</span>
            <span v-else-if="!c.registered" class="tag stop"><span class="dot"></span>未注册</span>
            <span v-else-if="c.disabled" class="tag stop"><span class="dot"></span>已禁用</span>
            <span v-else-if="!c.command_ok" class="tag warn-tag"><span class="dot"></span>路径失效</span>
            <span v-else class="tag run"><span class="dot"></span>已就绪</span>
          </div>

          <div class="client-path">{{ c.path }}</div>

          <div v-if="c.registered && c.command" class="client-cmd">
            指向：<code>{{ c.command }}</code>
          </div>
          <div v-if="c.registered && !c.command_ok" class="client-warn">
            配置里指向的可执行文件与当前构建不一致。客户端会静默启动失败 —— 这通常就是"看不到工具"的原因，重新写入即可。
          </div>
          <div v-if="c.registered && c.disabled" class="client-warn">
            条目被显式标记为 disabled，客户端会跳过它。
          </div>
          <div v-if="c.broken" class="client-warn">
            文件存在但不是合法 JSON，需要你手工修复 —— 自动改写会覆盖掉里面其它内容。
          </div>
          <div v-if="c.others && c.others.length" class="client-others">
            写入时会保留：{{ c.others.join('、') }}
          </div>

          <div class="client-actions">
            <button
              v-if="c.writable"
              class="btn sm"
              :disabled="!!busy"
              @click="writeClient(c)"
            >
              <Icon name="doc" :size="12" /> {{ c.registered ? '重新写入' : '写入配置' }}
            </button>
            <template v-else>
              <span class="client-manual">手工执行：<code>{{ c.hint }}</code></span>
              <button class="btn sm ghost" :disabled="!!busy" @click="copy(c.hint, '命令已复制')">
                <Icon name="copy" :size="12" /> 复制
              </button>
            </template>
          </div>
        </div>
      </div>

      <div class="mcp-note">
        写入只代表配置到位。<strong>自定义 MCP 需要在客户端里显式「信任」才会加载</strong> ——
        在连接器管理页的自定义连接器入口里点信任，然后新开一个会话。这一步无法由本应用代劳。
      </div>

      <details class="mcp-details">
        <summary>手动接入用的配置片段</summary>
        <div class="term-panel">
          <div class="out">{{ snippet }}</div>
        </div>
        <div class="mcp-details-actions">
          <button class="btn sm" @click="copy(snippet, '配置已复制')">
            <Icon name="copy" :size="12" /> 复制
          </button>
        </div>
      </details>
    </div>

    <div class="card">
      <div class="card-title">
        <Icon name="search" :size="14" /> 连通自检
        <span class="more">另起服务进程跑一次真实协议握手，不依赖客户端</span>
      </div>

      <div class="mcp-actions">
        <button class="btn primary" :disabled="!!busy" @click="runSelftest">
          <Icon name="search" :size="12" /> {{ busy === 'selftest' ? '自检中…' : '运行自检' }}
        </button>
        <span v-if="selftest" class="mcp-note inline">耗时 {{ selftest.elapsed_ms }} ms</span>
      </div>

      <div v-if="!selftest" class="mcp-note">
        这一步验证了三件事：可执行文件能启动、协议实现正确、工具清单能取回。
        自检通过说明问题一定在客户端侧（注册或信任），不必再怀疑服务端。
      </div>

      <template v-else-if="selftest.ok">
        <div class="st-ok">
          <Icon name="check" :size="13" /> 服务端正常 ·
          {{ selftest.server }} {{ selftest.version }} · 协议 {{ selftest.protocol }} ·
          共 {{ selftest.tool_count }} 个工具
        </div>
        <table class="tbl tool-tbl">
          <thead>
            <tr><th style="width: 210px">工具</th><th>说明</th></tr>
          </thead>
          <tbody>
            <tr v-for="t in selftest.tools" :key="t.name">
              <td class="mono">{{ t.name }}</td>
              <td class="dim">{{ t.description }}</td>
            </tr>
          </tbody>
        </table>
      </template>

      <template v-else>
        <div class="st-bad">
          <Icon name="alert" :size="13" /> 卡在「{{ selftest.stage }}」层：{{ selftest.error }}
        </div>
        <div class="mcp-note">{{ STAGE_HINT[selftest.stage] || '' }}</div>
      </template>
    </div>

    <div class="card">
      <div class="card-title">
        <Icon name="table" :size="14" /> AI 操作审计
        <span v-if="audit" class="more">共 {{ audit.total }} 条，显示最近 {{ audit.shown }} 条</span>
        <button class="btn sm ghost" style="margin-left: auto" :disabled="!!busy" @click="clearAudit">
          <Icon name="kill" :size="12" /> 清空
        </button>
      </div>

      <div v-if="!audit || !audit.items.length" class="empty">
        <p>还没有 AI 操作记录</p>
        <div class="hint">AI 通过 MCP 启动/停止/结束进程时会自动留痕</div>
      </div>
      <table v-else class="tbl">
        <thead>
          <tr>
            <th style="width: 130px">时间</th>
            <th style="width: 170px">工具</th>
            <th style="width: 100px">动作</th>
            <th>目标</th>
            <th style="width: 80px">结果</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(it, i) in audit.items" :key="i">
            <td class="num dim">{{ fmtDate(it.ts) }}</td>
            <td class="mono">{{ it.tool }}</td>
            <td>{{ ACT_LABEL[it.action] || it.action }}</td>
            <td class="mono dim">{{ it.target || '—' }}</td>
            <td :class="it.ok ? 'ok' : 'bad'">{{ it.ok ? '成功' : '失败' }}</td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>

<style scoped>
.mcp-page {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.mcp-banner {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 14px;
  border-radius: 10px;
  font-size: 12.5px;
  background: rgba(248, 113, 113, 0.12);
  border: 1px solid rgba(248, 113, 113, 0.35);
  color: var(--danger);
}

.mcp-stats {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 16px;
}

.stat-card {
  padding: 14px 16px;
}

.stat-val {
  font-size: 15px;
  color: var(--text);
  margin-top: 2px;
}

.stat-note {
  font-size: 11.5px;
  color: var(--text-faint);
  margin-top: 6px;
}

.stat-note.warn {
  color: var(--warn);
}

.stat-note.bad {
  color: var(--danger);
}

.mono {
  font-family: var(--mono);
}

.mcp-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 10px;
}

.mcp-inline {
  display: flex;
  align-items: center;
  gap: 6px;
}

.lbl {
  font-size: 12px;
  color: var(--text-faint);
}

.mcp-input {
  width: 84px;
  padding: 5px 9px;
  border-radius: 6px;
  font-size: 12.5px;
  font-family: var(--mono);
  background: var(--panel-3);
  border: 1px solid var(--border-2);
  color: var(--text);
  outline: none;
}

.mcp-input:focus {
  border-color: var(--accent);
}

.mcp-exe {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
  font-size: 12px;
  flex-wrap: wrap;
}

.mcp-exe code {
  font-family: var(--mono);
  font-size: 11.5px;
  color: var(--text-dim);
  background: var(--panel-3);
  padding: 3px 8px;
  border-radius: 5px;
  word-break: break-all;
}

.mcp-exe .bad,
.bad {
  color: var(--danger);
}

.mcp-note {
  font-size: 12px;
  color: var(--text-faint);
  line-height: 1.7;
  margin-top: 10px;
}

.mcp-note.inline {
  margin-top: 0;
}

.client-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.client {
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 13px 15px;
}

.client-head {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 13px;
}

.client-path {
  font-family: var(--mono);
  font-size: 11.5px;
  color: var(--text-faint);
  margin-top: 6px;
  word-break: break-all;
}

.client-cmd {
  font-size: 12px;
  color: var(--text-dim);
  margin-top: 6px;
  word-break: break-all;
}

.client-cmd code,
.client-manual code {
  font-family: var(--mono);
  font-size: 11.5px;
}

.client-warn {
  font-size: 12px;
  color: var(--warn);
  line-height: 1.65;
  margin-top: 7px;
}

.client-others {
  font-size: 11.5px;
  color: var(--text-faint);
  margin-top: 6px;
}

.client-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 11px;
  flex-wrap: wrap;
}

.client-manual {
  font-size: 12px;
  color: var(--text-dim);
}

.tag.bad-tag {
  background: rgba(248, 113, 113, 0.13);
  color: var(--danger);
}

.tag.bad-tag .dot {
  background: var(--danger);
}

.tag.warn-tag {
  background: rgba(251, 191, 36, 0.14);
  color: var(--warn);
}

.tag.warn-tag .dot {
  background: var(--warn);
}

.mcp-details {
  margin-top: 14px;
}

.mcp-details summary {
  font-size: 12.5px;
  color: var(--text-dim);
  cursor: pointer;
  padding: 4px 0;
}

.mcp-details .term-panel {
  margin-top: 10px;
}

.mcp-details-actions {
  margin-top: 10px;
}

.st-ok,
.st-bad {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  padding: 10px 14px;
  border-radius: 10px;
  margin-top: 12px;
  line-height: 1.6;
}

.st-ok {
  background: rgba(52, 211, 153, 0.12);
  border: 1px solid rgba(52, 211, 153, 0.3);
  color: var(--ok);
}

.st-bad {
  background: rgba(248, 113, 113, 0.1);
  border: 1px solid rgba(248, 113, 113, 0.3);
  color: var(--danger);
}

.tool-tbl {
  margin-top: 12px;
}

.tool-tbl td {
  font-size: 12px;
  line-height: 1.55;
}
</style>
