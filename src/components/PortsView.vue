<script setup>
import { computed, onMounted, ref } from 'vue'
import Icon from './Icon.vue'
import Modal from './Modal.vue'
import { api, parsePortSpec, isProtectedPid } from '../api'
import {
  store,
  toast,
  refreshConfig,
  syncScan,
  isPortProtected,
  setPortProtected,
  cleanErr,
} from '../store'

// 端口数据与项目运行态统一由 store 全局轮询产出，本页只负责展示，避免两页各扫一遍导致口径不一致
const ports = computed(() => store.ports)
const loading = ref(false)
const q = ref('')
const proto = ref('all')
const auto = computed({
  get: () => store.autoScan,
  set: (v) => (store.autoScan = v),
})
const onlyListen = ref(true)
const sortKey = ref('local_port')
const sortAsc = ref(true)
const killTarget = ref(null)
const killing = ref(false)
const detail = ref(null)
const unprotectTarget = ref(null)
const toggling = ref(0)

let scanning = false

async function loadPorts() {
  if (scanning) return
  scanning = true
  loading.value = true
  try {
    // syncScan 内部已经把失败写进 store.lastError，顶部全局横幅会据此提示"数据可能是旧的"，
    // 这里再 toast 一次就成了同一件事说两遍
    await syncScan()
  } finally {
    loading.value = false
    scanning = false
  }
}

onMounted(loadPorts)

function setSort(k) {
  if (sortKey.value === k) sortAsc.value = !sortAsc.value
  else {
    sortKey.value = k
    sortAsc.value = true
  }
}

/** 进程名在真机上可能取不到（权限不足的系统进程），直接 toLowerCase 会让整页渲染失败 */
const procName = (p) => String(p.process_name || '')
const procAddr = (p) => String(p.local_addr || '')

const filtered = computed(() => {
  let list = ports.value
  if (proto.value !== 'all') list = list.filter((p) => p.proto === proto.value)
  if (onlyListen.value) list = list.filter((p) => p.state === 'LISTENING' || p.proto === 'UDP')
  const s = q.value.trim().toLowerCase()
  if (s) {
    list = list.filter(
      (p) =>
        String(p.local_port).includes(s) ||
        procName(p).toLowerCase().includes(s) ||
        String(p.pid).includes(s) ||
        procAddr(p).toLowerCase().includes(s)
    )
  }
  const dir = sortAsc.value ? 1 : -1
  const key = sortKey.value
  return [...list].sort((a, b) => {
    const va = key === 'process_name' ? procName(a).toLowerCase() : a[key]
    const vb = key === 'process_name' ? procName(b).toLowerCase() : b[key]
    if (va === vb) return (a.local_port || 0) - (b.local_port || 0)
    return va < vb ? -dir : dir
  })
})

/** 受保护端口数量：这个数字要一直看得见，否则用户不明白为什么"结束"按钮点不动 */
const protectedCount = computed(() => (store.config.protected_ports || []).length)

/**
 * 端口 → 托管项目（统一取自 store.runtime）：
 * 「声明端口正在监听」或「监听进程目录命中项目目录」都会建立关联，
 * 因此在终端里手工启动的服务同样能显示归属，不再只认本应用启动的任务。
 */
const projectByPort = computed(() => {
  const m = {}
  for (const p of store.config.projects) {
    const r = store.runtime[p.id]
    if (!r || !r.running) continue
    // 优先用实测监听的端口；刚启动尚未监听时回落到声明端口。
    // 解析统一走 parsePortSpec，与后端同一套语义（含 3000-3010 区间）
    const declared = parsePortSpec(p.ports)
    const nums = r.ports && r.ports.length ? r.ports.map(Number) : declared
    for (const n of nums) m[n] = p
  }
  return m
})

function projOf(p) {
  return projectByPort.value[p.local_port]
}

/** 该端口的归属项目是否为外部启动（非本应用） */
function projExternal(p) {
  const proj = projOf(p)
  return !!(proj && store.runtime[proj.id] && store.runtime[proj.id].external)
}

function stateTag(p) {
  if (p.proto === 'UDP') return 'udp'
  if (p.state === 'LISTENING') return 'listen'
  if (p.state === 'ESTABLISHED') return 'established'
  return 'gray'
}

/** 受保护端口的进程，结束按钮要置灰：后端一定会拒，提前说明比点了才报错好 */
function killBlocked(p) {
  if (isPortProtected(p.local_port)) return '该端口已设为受保护，后端会拒绝结束它。先解除保护再操作。'
  if (isProtectedPid(p.pid)) return 'PID ' + p.pid + ' 属于系统关键进程，应用不会处理'
  return ''
}

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
  const why = killBlocked(p)
  if (why) {
    toast(why, 'err')
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

/** 加上保护是收紧，直接生效；解除保护是松开一道闸门，先确认 */
async function toggleProtect(p) {
  const port = p.local_port
  if (toggling.value) return
  if (!isPortProtected(port)) {
    toggling.value = port
    const ok = await setPortProtected(port, true)
    toggling.value = 0
    if (ok) toast(`端口 ${port} 已设为受保护，结束与接管都会被拒绝`)
    return
  }
  unprotectTarget.value = p
}

async function confirmUnprotect() {
  const p = unprotectTarget.value
  unprotectTarget.value = null
  if (!p) return
  toggling.value = p.local_port
  const ok = await setPortProtected(p.local_port, false)
  toggling.value = 0
  if (ok) toast(`端口 ${p.local_port} 已解除保护`)
}
</script>

<template>
  <div>
    <div class="toolbar">
      <div class="search">
        <Icon name="search" :size="15" />
        <input class="input" v-model="q" placeholder="搜索端口 / 进程名 / PID / 地址" />
      </div>
      <div class="seg">
        <button :class="{ on: proto === 'all' }" @click="proto = 'all'">全部</button>
        <button :class="{ on: proto === 'TCP' }" @click="proto = 'TCP'">TCP</button>
        <button :class="{ on: proto === 'UDP' }" @click="proto = 'UDP'">UDP</button>
      </div>
      <div class="switch" :class="{ on: onlyListen }" @click="onlyListen = !onlyListen">
        <div class="track"></div>仅监听
      </div>
      <div class="switch" :class="{ on: auto }" @click="auto = !auto">
        <div class="track"></div>自动刷新
      </div>
      <div class="chip" title="受保护端口上的进程不会被结束或接管">
        <Icon name="lock" :size="12" /> 受保护 <b>{{ protectedCount }}</b>
      </div>
      <button class="btn" style="margin-left: auto" :disabled="loading" @click="loadPorts()">
        <Icon name="refresh" :size="13" :class="{ spin: loading }" /> 刷新
      </button>
    </div>

    <div class="card" style="padding: 0; overflow: hidden">
      <div style="max-height: calc(100vh - 230px); overflow: auto">
        <table class="tbl">
          <thead>
            <tr>
              <th style="cursor: pointer" @click="setSort('local_port')">端口 ⇅</th>
              <th>协议</th>
              <th>本地地址</th>
              <th>远程地址</th>
              <th>状态</th>
              <th style="cursor: pointer" @click="setSort('pid')">PID ⇅</th>
              <th style="cursor: pointer" @click="setSort('process_name')">进程 ⇅</th>
              <th>托管项目</th>
              <th style="text-align: right">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(p, i) in filtered" :key="i">
              <td class="num" style="font-weight: 600; color: var(--cyan)">
                {{ p.local_port }}
                <Icon
                  v-if="isPortProtected(p.local_port)"
                  name="lock"
                  :size="11"
                  class="port-lock"
                  title="该端口受保护，其上的进程不会被结束或接管"
                />
              </td>
              <td><span class="tag" :class="p.proto.toLowerCase()"><span class="dot"></span>{{ p.proto }}</span></td>
              <td class="num">{{ p.local_addr }}</td>
              <td class="num" style="color: var(--text-faint)">{{ p.remote_addr }}</td>
              <td>
                <span v-if="p.proto === 'TCP'" class="tag" :class="stateTag(p)"><span class="dot"></span>{{ p.state }}</span>
                <span v-else class="tag" :class="stateTag(p)"><span class="dot"></span>绑定</span>
              </td>
              <td class="num">{{ p.pid }}</td>
              <td style="max-width: 220px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap" :title="p.process_name">
                {{ p.process_name || '-' }}
              </td>
              <td style="max-width: 160px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap">
                <span
                  v-if="projOf(p)"
                  class="tag proj"
                  :class="{ ext: projExternal(p) }"
                  :title="projOf(p).name + (projExternal(p) ? '（外部启动）' : '')"
                  :style="{ color: projOf(p).color, borderColor: projOf(p).color }"
                >
                  <span class="dot" :style="{ background: projOf(p).color }"></span>{{ projOf(p).name }}
                  <span v-if="projExternal(p)" class="ext-mark">外部</span>
                </span>
                <span v-else style="color: var(--text-faint)">-</span>
              </td>
              <td>
                <div class="actions">
                  <button class="btn sm ghost" @click="showDetail(p)" title="查看命令行"><Icon name="doc" :size="13" /></button>
                  <button
                    class="btn sm ghost"
                    :disabled="toggling === p.local_port"
                    :title="isPortProtected(p.local_port) ? '解除保护，解除后该端口上的进程可以被结束' : '设为受保护，保护后不会被误杀'"
                    @click="toggleProtect(p)"
                  >
                    <Icon :name="isPortProtected(p.local_port) ? 'unlock' : 'lock'" :size="12" />
                  </button>
                  <button
                    class="btn sm danger"
                    :disabled="!!killBlocked(p)"
                    :title="killBlocked(p) || '结束该进程'"
                    @click="askKill(p)"
                  >
                    <Icon name="kill" :size="12" /> 结束
                  </button>
                </div>
              </td>
            </tr>
            <tr v-if="!filtered.length && !loading">
              <td colspan="9">
                <div class="empty">
                  <div class="icon">⌕</div>
                  <p>没有匹配的端口记录</p>
                  <p class="hint" v-if="q">尝试更换关键字或关闭「仅监听」</p>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <Modal :open="!!killTarget" @close="killTarget = null" title="结束进程" width="440">
      <div style="font-size: 13.5px; line-height: 1.8" v-if="killTarget">
        确认结束进程 <b style="color: var(--danger)">{{ killTarget.process_name || '未知进程' }}</b>
        （PID: <span style="font-family: var(--mono)">{{ killTarget.pid }}</span>，端口
        <span style="font-family: var(--mono); color: var(--cyan)">{{ killTarget.local_port }}</span>）？
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

    <Modal :open="!!unprotectTarget" @close="unprotectTarget = null" title="解除端口保护" width="480">
      <div v-if="unprotectTarget" style="font-size: 13.5px; line-height: 1.8">
        确认解除端口
        <b style="font-family: var(--mono); color: var(--cyan)">{{ unprotectTarget.local_port }}</b>
        的保护？
        <div style="color: var(--text-faint); font-size: 12px; margin-top: 8px">
          解除后，该端口上的进程可以被「结束进程」或「接管」直接终止。
          如果这个端口上跑着正在用的服务，结束它会立刻中断服务。
        </div>
      </div>
      <template #foot>
        <button class="btn" @click="unprotectTarget = null">取消</button>
        <button class="btn danger" @click="confirmUnprotect">
          <Icon name="unlock" :size="13" /> 解除保护
        </button>
      </template>
    </Modal>

    <Modal :open="!!detail" @close="detail = null" title="进程详情" width="560">
      <div v-if="detail">
        <div class="sysinfo-list">
          <div class="row"><span>进程名</span><span>{{ detail.name || '-' }}</span></div>
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
