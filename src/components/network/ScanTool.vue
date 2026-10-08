<script setup>
/**
 * 端口扫描：探测目标主机开放了哪些端口。
 *
 * 补了端口预设（含本机常用服务，覆盖本机 ServBay / FRP / Ollama 等在用的端口），
 * 省去每次手打一长串端口号；结果表也标明服务名，不用自己去查。
 */
import { computed, onUnmounted, reactive, ref, watch } from 'vue'
import Icon from '../Icon.vue'
import ResultBox from './ResultBox.vue'
import { api } from '../../api'
import { toast, cleanErr } from '../../store'
import { loadPref, savePref } from '../../netPrefs'

/** 端口预设：本机一组特意带上 ServBay / FRP / Ollama 等本机在跑的服务 */
const PRESETS = [
  { label: '常用服务', ports: '21,22,23,25,53,80,110,143,443,445,3306,3389,5432,6379,8080,9200,27017' },
  { label: 'Web 服务', ports: '80,443,3000,5000,8000,8080,8081,8443,8888,9000,9090' },
  { label: '数据库', ports: '1433,1521,3306,5432,6379,9200,11211,27017' },
  { label: '本机服务', ports: '80,443,3306,6379,8000,8080,8890,9527,9528,11434' },
]

const form = reactive({
  host: loadPref('scan.host', '127.0.0.1'),
  ports: loadPref('scan.ports', PRESETS[3].ports),
})
const state = ref('idle')
const res = ref(null)
const err = ref('')
const elapsed = ref(0)
const waited = ref(0)

let timer = null

watch(() => form.host, (v) => savePref('scan.host', v))
watch(() => form.ports, (v) => savePref('scan.ports', v))
onUnmounted(() => {
  if (timer) clearInterval(timer)
})

/** 端口条数，用于提示扫描规模（大范围会明显变慢） */
const portCount = computed(() => {
  let n = 0
  for (const part of String(form.ports).split(/[,，\s]+/)) {
    const p = part.trim()
    if (!p) continue
    const m = p.match(/^(\d+)\s*-\s*(\d+)$/)
    if (m) {
      const a = Math.min(+m[1], +m[2])
      const b = Math.max(+m[1], +m[2])
      n += Math.max(0, Math.min(b, 65535) - Math.max(a, 1) + 1)
    } else if (/^\d+$/.test(p)) {
      n += 1
    }
  }
  return n
})

async function run() {
  if (state.value === 'running') return
  const host = form.host.trim()
  if (!host) {
    toast('请填写目标主机', 'err')
    return
  }
  if (!portCount.value) {
    toast('请填写端口（如 80,443,8000-8010）', 'err')
    return
  }
  state.value = 'running'
  res.value = null
  err.value = ''
  waited.value = 0
  const t0 = Date.now()
  timer = setInterval(() => {
    waited.value = Math.round((Date.now() - t0) / 1000)
  }, 200)
  try {
    res.value = await api.netPortScan(host, form.ports)
    elapsed.value = Date.now() - t0
    state.value = 'ok'
  } catch (e) {
    err.value = cleanErr(e)
    state.value = 'err'
    toast('扫描失败: ' + err.value, 'err')
  } finally {
    clearInterval(timer)
    timer = null
  }
}

const summary = computed(() => {
  const r = res.value
  if (!r) return ''
  if (!r.open.length) return `扫描 ${r.scanned} 个端口，没有发现开放端口`
  return `扫描 ${r.scanned} 个端口，开放 ${r.open.length} 个：${r.open.map((p) => p.port).join(', ')}`
})

const report = computed(() => {
  const r = res.value
  if (!r) return ''
  const L = [
    `端口扫描 ${form.host}`,
    `时间：${new Date().toLocaleString('zh-CN')}`,
    `端口：${form.ports}`,
    '',
    `扫描 ${r.scanned} 个 · 开放 ${r.open.length} 个 · 关闭 ${r.closed} 个`,
    '',
    '端口\t服务',
    ...r.open.map((p) => `${p.port}\t${p.service || '未知'}`),
  ]
  return L.join('\n')
})

function usePreset(p) {
  form.ports = p.ports
}
</script>

<template>
  <div class="net-tool">
    <div class="net-form">
      <div class="net-field">
        <label>目标主机</label>
        <input v-model="form.host" class="input" placeholder="IP 或域名" @keyup.enter="run" />
      </div>
      <div class="net-field grow">
        <label>端口（逗号或区间）</label>
        <input
          v-model="form.ports"
          class="input"
          placeholder="80,443,8000-8010"
          @keyup.enter="run"
        />
      </div>
      <button class="btn primary" :disabled="state === 'running'" @click="run">
        <Icon name="play" :size="12" /> {{ state === 'running' ? '扫描中…' : '开始扫描' }}
      </button>
    </div>

    <div class="net-chips">
      <span class="chips-label">端口预设</span>
      <button v-for="p in PRESETS" :key="p.label" class="chip-btn" @click="usePreset(p)">
        {{ p.label }}
      </button>
      <span class="chips-note">共 {{ portCount }} 个端口<span v-if="portCount > 200">，数量较多会明显变慢</span></span>
    </div>

    <ResultBox
      :state="state"
      :title="
        state === 'running'
          ? `正在扫描 ${form.host} 的 ${portCount} 个端口（已等待 ${waited}s）`
          : res
            ? `${form.host} · 扫描 ${res.scanned} 个端口`
            : ''
      "
      :summary="summary"
      :error="err"
      :raw="''"
      :report="report"
      :elapsed="elapsed"
      name="portscan-report"
      empty="填写目标主机与端口后开始扫描"
    >
      <table v-if="res" class="tbl">
        <thead>
          <tr>
            <th style="width: 110px">端口</th>
            <th>服务</th>
          </tr>
        </thead>
        <tbody>
          <tr v-if="!res.open.length">
            <td colspan="2" class="dim">未发现开放端口</td>
          </tr>
          <tr v-for="p in res.open" :key="p.port">
            <td class="mono ok">{{ p.port }}</td>
            <td>{{ p.service || '未知服务' }}</td>
          </tr>
        </tbody>
      </table>
    </ResultBox>
  </div>
</template>
