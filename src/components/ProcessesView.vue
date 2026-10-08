<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue'
import Icon from './Icon.vue'
import Modal from './Modal.vue'
import { api, fmtBytes, isProtectedPid } from '../api'
import { store, toast, refreshConfig, cleanErr } from '../store'

const list = ref([])
const q = ref('')
const loading = ref(false)
const killTarget = ref(null)
const killing = ref(false)
const detail = ref(null)
const loadErr = ref('')
const LIMIT = 300
const POLL_MS = 10000

let timer = null
let inFlight = false

async function load() {
  // 进程多时一次枚举可能要好几秒；固定间隔重复触发会把请求堆起来，
  // 后回来的旧结果还会盖掉新结果。这里直接跳过重叠的那一次。
  if (inFlight) return
  inFlight = true
  loading.value = true
  try {
    list.value = await api.listProcesses()
    loadErr.value = ''
  } catch (e) {
    loadErr.value = cleanErr(e)
    toast('进程列表获取失败: ' + loadErr.value, 'err')
  } finally {
    loading.value = false
    inFlight = false
  }
}

function schedule() {
  clearTimeout(timer)
  timer = setTimeout(async () => {
    await load()
    schedule()
  }, POLL_MS)
}

onMounted(() => {
  load()
  schedule()
})
onUnmounted(() => clearTimeout(timer))

const matched = computed(() => {
  const s = q.value.trim().toLowerCase()
  if (!s) return list.value
  // name 理论上一定有，但真机上见过取不到名字的进程，
  // 直接 toLowerCase() 会让整个列表渲染炸掉
  return list.value.filter(
    (p) => String(p.name || '').toLowerCase().includes(s) || String(p.pid).includes(s)
  )
})

const shown = computed(() => matched.value.slice(0, LIMIT))
const shownMem = computed(() => shown.value.reduce((a, p) => a + (p.mem_kb || 0), 0))
const isTrimmed = computed(() => matched.value.length > shown.value.length)

async function showDetail(p) {
  detail.value = { pid: p.pid, name: p.name, cmdline: '', loading: true }
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
      toast(`已结束 ${t.name || 'PID ' + t.pid}`)
      // 先从本地列表摘掉：整表刷新要等下一次轮询，这中间用户会以为没生效
      list.value = list.value.filter((p) => p.pid !== t.pid)
      await refreshConfig()
      await load()
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
    <div class="toolbar">
      <div class="search">
        <Icon name="search" :size="15" />
        <input class="input" v-model="q" placeholder="搜索进程名 / PID" />
      </div>
      <div class="chip">
        进程总数 <b>{{ list.length }}</b>
      </div>
      <div class="chip">
        匹配 <b>{{ matched.length }}</b>
        <template v-if="isTrimmed">（仅显示前 <b>{{ LIMIT }}</b>）</template>
        · 已显示合计内存 <b>{{ fmtBytes(shownMem * 1024) }}</b>
      </div>
      <button class="btn" style="margin-left: auto" :disabled="loading" @click="load()">
        <Icon name="refresh" :size="13" :class="{ spin: loading }" /> 刷新
      </button>
    </div>

    <div v-if="loadErr" class="warn-banner">
      <Icon name="bolt" :size="13" />
      <span>进程列表获取失败：{{ loadErr }}。下方内容可能是上一次的快照。</span>
    </div>

    <div class="card" style="padding: 0; overflow: hidden">
      <div style="max-height: calc(100vh - 230px); overflow: auto">
        <table class="tbl">
          <thead>
            <tr>
              <th>进程名</th>
              <th>PID</th>
              <th>内存</th>
              <th style="text-align: right">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="p in shown" :key="p.pid">
              <td style="max-width: 420px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap" :title="p.name">
                {{ p.name || '-' }}
              </td>
              <td class="num">{{ p.pid }}</td>
              <td class="num">{{ ((p.mem_kb || 0) / 1024).toFixed(1) }} MB</td>
              <td>
                <div class="actions">
                  <button class="btn sm ghost" @click="showDetail(p)" title="查看命令行"><Icon name="doc" :size="13" /></button>
                  <button
                    class="btn sm danger"
                    :disabled="isProtectedPid(p.pid)"
                    :title="isProtectedPid(p.pid) ? '系统关键进程，不可结束' : '结束该进程'"
                    @click="askKill(p)"
                  >
                    <Icon name="kill" :size="12" /> 结束
                  </button>
                </div>
              </td>
            </tr>
            <tr v-if="!shown.length && !loading">
              <td colspan="4">
                <div class="empty"><div class="icon">⌕</div><p>没有匹配的进程</p></div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <Modal :open="!!killTarget" @close="killTarget = null" title="结束进程" width="440">
      <div style="font-size: 13.5px; line-height: 1.8" v-if="killTarget">
        确认结束进程 <b style="color: var(--danger)">{{ killTarget.name || '未知进程' }}</b>
        （PID: <span style="font-family: var(--mono)">{{ killTarget.pid }}</span>）？
        <div style="color: var(--text-faint); font-size: 12px; margin-top: 8px">
          该进程及其子进程树将被强制终止。
        </div>
      </div>
      <template #foot>
        <button class="btn" @click="killTarget = null">取消</button>
        <button class="btn danger" :disabled="killing" @click="doKill">
          <Icon name="kill" :size="13" /> {{ killing ? '结束中...' : '强制结束' }}
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
