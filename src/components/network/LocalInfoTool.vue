<script setup>
/**
 * 本机网络信息：打开即知本机网络全貌。
 *
 * 公网出口 IP 是本工具箱唯一的**外发请求**（访问 myip.ipip.net），
 * 所以不自动执行，必须由用户点按钮触发，界面上也写明这一点。
 */
import { computed, onMounted, ref } from 'vue'
import Icon from '../Icon.vue'
import ResultBox from './ResultBox.vue'
import { api } from '../../api'
import { toast, cleanErr } from '../../store'

const state = ref('idle')
const info = ref(null)
const err = ref('')

const pubState = ref('idle')
const pub = ref(null)
const pubErr = ref('')

async function load() {
  state.value = 'running'
  err.value = ''
  try {
    info.value = await api.netLocalInfo()
    state.value = 'ok'
  } catch (e) {
    err.value = cleanErr(e)
    state.value = 'err'
    toast('读取本机网络信息失败: ' + err.value, 'err')
  }
}

async function loadPublic() {
  if (pubState.value === 'running') return
  pubState.value = 'running'
  pubErr.value = ''
  try {
    pub.value = await api.netPublicIp()
    pubState.value = 'ok'
  } catch (e) {
    pubErr.value = cleanErr(e)
    pubState.value = 'err'
  }
}

onMounted(load)

const summary = computed(() => {
  const i = info.value
  if (!i) return ''
  const linked = i.interfaces.filter((x) => x.ipv4)
  const parts = [`${i.interfaces.length} 个网卡`]
  if (linked.length) parts.push(`${linked.length} 个已分配 IPv4`)
  if (i.primary_gateway) parts.push(`默认网关 ${i.primary_gateway}`)
  if (i.hostname) parts.push(`主机名 ${i.hostname}`)
  return parts.join(' · ')
})

const report = computed(() => {
  const i = info.value
  if (!i) return ''
  const L = ['本机网络信息', `时间：${new Date().toLocaleString('zh-CN')}`, '']
  if (i.hostname) L.push(`主机名：${i.hostname}`)
  if (i.primary_gateway) L.push(`默认网关：${i.primary_gateway}`)
  L.push('')
  for (const n of i.interfaces) {
    L.push(`[${n.kind || '适配器'}] ${n.name}`)
    if (n.desc) L.push(`  描述：${n.desc}`)
    if (n.ipv4) L.push(`  IPv4：${n.ipv4}${n.mask ? ' / ' + n.mask : ''}`)
    if (n.gateway) L.push(`  网关：${n.gateway}`)
    if (n.mac) L.push(`  MAC：${n.mac}`)
    if (n.dns.length) L.push(`  DNS：${n.dns.join(', ')}`)
    if (n.state) L.push(`  状态：${n.state}`)
    L.push('')
  }
  if (pub.value) {
    L.push(`公网出口 IP：${pub.value.ip}${pub.value.location ? '（' + pub.value.location + '）' : ''}`)
  }
  return L.join('\n')
})
</script>

<template>
  <div class="net-tool">
    <div class="net-form">
      <button class="btn primary" :disabled="state === 'running'" @click="load">
        <Icon name="refresh" :size="12" :class="{ 'is-spin': state === 'running' }" />
        {{ state === 'running' ? '读取中…' : '刷新' }}
      </button>
      <span class="net-inline-note">读取 ipconfig /all，不发起任何外发请求</span>
    </div>

    <ResultBox
      :state="state"
      :title="info ? `本机网络 · ${info.interfaces.length} 个网卡` : '正在读取…'"
      :summary="summary"
      :error="err"
      :raw="info?.raw || ''"
      :report="report"
      name="localnet-report"
      empty="点击刷新读取本机网络配置"
    >
      <div v-if="info" class="nic-list">
        <div v-for="(n, i) in info.interfaces" :key="i" class="nic-card" :class="{ off: !n.ipv4 }">
          <div class="nic-head">
            <span class="nic-kind">{{ n.kind || '适配器' }}</span>
            <span class="nic-name">{{ n.name }}</span>
            <span v-if="n.ipv4" class="tag ok">已连接</span>
            <span v-else class="tag">无 IPv4</span>
          </div>
          <div v-if="n.desc" class="nic-desc">{{ n.desc }}</div>
          <div class="nic-grid">
            <div v-if="n.ipv4" class="nic-row">
              <span class="k">IPv4</span>
              <span class="v mono strong">{{ n.ipv4 }}</span>
            </div>
            <div v-if="n.mask" class="nic-row">
              <span class="k">子网掩码</span>
              <span class="v mono">{{ n.mask }}</span>
            </div>
            <div v-if="n.gateway" class="nic-row">
              <span class="k">默认网关</span>
              <span class="v mono">{{ n.gateway }}</span>
            </div>
            <div v-if="n.mac" class="nic-row">
              <span class="k">MAC</span>
              <span class="v mono">{{ n.mac }}</span>
            </div>
            <div v-if="n.dns.length" class="nic-row">
              <span class="k">DNS</span>
              <span class="v mono">{{ n.dns.join(' , ') }}</span>
            </div>
            <div v-if="n.state" class="nic-row">
              <span class="k">媒体状态</span>
              <span class="v">{{ n.state }}</span>
            </div>
          </div>
        </div>
      </div>
    </ResultBox>

    <div class="card pub-card">
      <div class="pub-head">
        <div>
          <div class="pub-title"><Icon name="globe" :size="14" /> 公网出口 IP</div>
          <div class="pub-sub">
            这是本工具箱唯一会访问外部服务的功能（数据源 myip.ipip.net），需手动触发
          </div>
        </div>
        <button class="btn" :disabled="pubState === 'running'" @click="loadPublic">
          {{ pubState === 'running' ? '查询中…' : '查询' }}
        </button>
      </div>
      <div v-if="pubState === 'ok' && pub" class="pub-out">
        <span class="mono pub-ip">{{ pub.ip }}</span>
        <span v-if="pub.location" class="pub-loc">{{ pub.location }}</span>
        <span class="pub-src">来源 {{ pub.source }}</span>
      </div>
      <div v-else-if="pubState === 'err'" class="net-msg err">
        {{ pubErr }}<template v-if="/不可达|超时/.test(pubErr)"> —— 这本身也是一条信息：本机可能不允许访问该服务。</template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.nic-list { display: flex; flex-direction: column; gap: 10px; }
.nic-card {
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 11px 13px;
  background: rgba(255, 255, 255, 0.02);
}
.nic-card.off { opacity: 0.6; }
.nic-head { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.nic-kind {
  font-size: 10.5px;
  color: var(--text-faint);
  border: 1px solid var(--border-2);
  border-radius: 5px;
  padding: 1px 6px;
}
.nic-name { font-weight: 600; font-size: 13px; }
.nic-desc { font-size: 11.5px; color: var(--text-faint); margin: 5px 0 8px; }
.nic-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(230px, 1fr)); gap: 5px 16px; }
.nic-row { display: flex; gap: 8px; font-size: 12px; }
.nic-row .k { color: var(--text-faint); flex-shrink: 0; min-width: 58px; }
.nic-row .v { color: var(--text-dim); user-select: text; }
.nic-row .v.strong { color: var(--cyan); font-weight: 600; }
.is-spin { animation: nic-rot 0.8s linear infinite; }
@keyframes nic-rot { to { transform: rotate(360deg); } }

.pub-card { margin-top: 12px; }
.pub-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 14px; }
.pub-title { display: flex; align-items: center; gap: 7px; font-weight: 600; font-size: 13px; }
.pub-sub { font-size: 11.5px; color: var(--text-faint); margin-top: 4px; }
.pub-out { display: flex; align-items: baseline; gap: 12px; margin-top: 12px; flex-wrap: wrap; }
.pub-ip { font-size: 20px; font-weight: 700; color: var(--cyan); user-select: text; }
.pub-loc { font-size: 12.5px; color: var(--text-dim); }
.pub-src { font-size: 11px; color: var(--text-faint); }
</style>
