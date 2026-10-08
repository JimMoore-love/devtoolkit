<script setup>
import { computed, onMounted } from 'vue'
import Icon from './components/Icon.vue'
import Dashboard from './components/Dashboard.vue'
import PortsView from './components/PortsView.vue'
import ProjectsView from './components/ProjectsView.vue'
import QuickCmd from './components/QuickCmd.vue'
import ProcessesView from './components/ProcessesView.vue'
import NetworkView from './components/NetworkView.vue'
import McpView from './components/McpView.vue'
import { store, initStore, setPage, toast } from './store'
import { fmtBytes } from './api'

const views = {
  dashboard: Dashboard,
  ports: PortsView,
  projects: ProjectsView,
  quickcmd: QuickCmd,
  processes: ProcessesView,
  network: NetworkView,
  mcp: McpView,
}

const meta = {
  dashboard: { title: '仪表盘', sub: '系统资源与运行概览' },
  ports: { title: '端口 / 服务发现', sub: '本机监听端口与连接状态' },
  projects: { title: '项目管理', sub: '命令行启动的项目托管' },
  quickcmd: { title: '快速命令', sub: '即时执行命令并留存历史' },
  processes: { title: '进程管理', sub: '按内存排序的进程与终止' },
  network: { title: '网络工具箱', sub: '连通性诊断 · 端口与服务发现 · DNS 查询 · 本机网络信息' },
  mcp: { title: 'MCP 管理', sub: 'AI 接入 · 客户端配置 · 连通自检 · 操作审计' },
}

const navs = [
  { key: 'dashboard', label: '仪表盘', icon: 'dashboard' },
  { key: 'ports', label: '端口 / 服务', icon: 'ports' },
  { key: 'network', label: '网络工具箱', icon: 'bolt' },
  { key: 'mcp', label: 'MCP 管理', icon: 'power' },
  { key: 'projects', label: '项目管理', icon: 'projects' },
  { key: 'quickcmd', label: '快速命令', icon: 'terminal' },
  { key: 'processes', label: '进程管理', icon: 'process' },
]

const title = computed(() => meta[store.page]?.title || '')
const sub = computed(() => meta[store.page]?.sub || '')
const runningCount = computed(() => Object.keys(store.tasks).length)
const memPct = computed(() =>
  store.metrics.mem_total ? (store.metrics.mem_used / store.metrics.mem_total) * 100 : 0
)

/** 与后端失联时把"数据可能是旧的"摆到明面上，而不是继续展示旧快照 */
const staleText = computed(() => {
  if (!store.lastError) return ''
  const secs = store.lastScanAt ? Math.round((Date.now() - store.lastScanAt) / 1000) : 0
  const ago = secs > 0 ? `数据停留在 ${secs} 秒前` : '尚未取到数据'
  return `${store.lastError} · ${ago}`
})

onMounted(() => {
  initStore()
})
</script>

<template>
  <div class="app">
    <aside class="sidebar">
      <div class="logo">
        <div class="logo-mark">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
            <path d="M5 7l5 5-5 5" /><path d="M13 17h6" />
          </svg>
        </div>
        <div class="logo-text">
          <div class="t1">DevToolkit</div>
          <div class="t2">开发工具助手</div>
        </div>
      </div>

      <div class="nav">
        <div class="nav-label">导航</div>
        <div
          v-for="n in navs"
          :key="n.key"
          class="nav-item"
          :class="{ active: store.page === n.key }"
          @click="setPage(n.key)"
        >
          <Icon :name="n.icon" :size="17" />
          <span>{{ n.label }}</span>
          <span v-if="n.key === 'projects' && runningCount" class="badge">{{ runningCount }}</span>
        </div>
      </div>

      <div class="sidebar-foot">
        <span class="dot"></span>本机服务运行中 · v1.5.0
      </div>
    </aside>

    <div class="main">
      <header class="topbar">
        <div>
          <h1>{{ title }}</h1>
          <div class="sub">{{ sub }}</div>
        </div>
        <div class="top-status">
          <div class="chip">
            CPU <b>{{ store.metrics.cpu.toFixed(0) }}%</b>
            <div class="mini-bar"><i :style="{ width: store.metrics.cpu + '%' }"></i></div>
          </div>
          <div class="chip">
            内存 <b>{{ memPct.toFixed(0) }}%</b>
            <div class="mini-bar"><i :style="{ width: memPct + '%' }"></i></div>
          </div>
          <div class="chip" v-if="store.metrics.mem_total">
            <b>{{ fmtBytes(store.metrics.mem_used) }}</b> / {{ fmtBytes(store.metrics.mem_total) }}
          </div>
        </div>
      </header>

      <main class="content">
        <div v-if="store.lastError" class="stale-banner">
          <Icon name="info" :size="14" />
          <span>
            后端暂时没响应，<b>下面显示的是最后一次取到的数据</b>，可能不是当前状态。
            <span class="stale-detail">{{ staleText }}</span>
          </span>
        </div>
        <component :is="views[store.page]" />
      </main>
    </div>

    <div class="toasts">
      <div v-for="t in store.toasts" :key="t.id" class="toast" :class="t.type">
        <Icon :name="t.type === 'err' ? 'info' : 'bolt'" :size="15" />
        <span>{{ t.message }}</span>
      </div>
    </div>
  </div>
</template>
