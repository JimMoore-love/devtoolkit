<script setup>
/**
 * 测速：下载带宽 + 延迟 + 抖动 + 丢包。
 *
 * 此前只有"下载速度"一个数字，但决定"视频会议卡不卡、远程桌面跟不跟手"的
 * 恰恰是延迟与抖动。数据源也换成国内镜像——实测 Cloudflare 在国内网络下
 * 基本不可达，默认把它排在第一位等于默认让用户看到失败。
 */
import { computed, onUnmounted, reactive, ref, watch } from 'vue'
import Icon from '../Icon.vue'
import ResultBox from './ResultBox.vue'
import { api } from '../../api'
import { toast, cleanErr } from '../../store'
import { loadPref, savePref } from '../../netPrefs'

const SOURCES = [
  { value: 'ustc', label: '中科大镜像（国内）' },
  { value: 'aliyun', label: '阿里云镜像（国内）' },
  { value: 'tuna', label: '清华镜像（国内）' },
  { value: 'cloudflare', label: 'Cloudflare（国际）' },
]

const form = reactive({ source: loadPref('speed.source', 'ustc') })
const state = ref('idle')
const res = ref(null)
const err = ref('')
const waited = ref(0)
const elapsed = ref(0)

let timer = null

watch(() => form.source, (v) => savePref('speed.source', v))
onUnmounted(() => {
  if (timer) clearInterval(timer)
})

async function run() {
  if (state.value === 'running') return
  state.value = 'running'
  res.value = null
  err.value = ''
  waited.value = 0
  const t0 = Date.now()
  timer = setInterval(() => {
    waited.value = Math.round((Date.now() - t0) / 1000)
  }, 200)
  try {
    res.value = await api.netSpeedtest(form.source)
    elapsed.value = Date.now() - t0
    state.value = 'ok'
  } catch (e) {
    err.value = cleanErr(e)
    state.value = 'err'
    toast('测速失败: ' + err.value, 'err')
  } finally {
    clearInterval(timer)
    timer = null
  }
}

/** 单项评级：阈值按"日常办公够不够用"定，不追求绝对数值高低 */
function rate(kind, v) {
  if (v == null) return null
  const t = {
    mbps: [
      [100, '优秀', 'ok'],
      [50, '良好', 'ok'],
      [20, '一般', 'warn'],
      [0, '偏慢', 'bad'],
    ],
    latency: [
      [20, '优秀', 'ok'],
      [50, '良好', 'ok'],
      [100, '一般', 'warn'],
      [Infinity, '偏高', 'bad'],
    ],
    jitter: [
      [5, '稳定', 'ok'],
      [20, '尚可', 'ok'],
      [50, '波动大', 'warn'],
      [Infinity, '很不稳定', 'bad'],
    ],
  }[kind]
  if (kind === 'mbps') {
    for (const [min, label, cls] of t) if (v >= min) return { label, cls }
    return null
  }
  for (const [max, label, cls] of t) if (v < max) return { label, cls }
  return null
}

const verdict = computed(() => {
  const r = res.value
  if (!r) return ''
  const lat = r.latency_ms
  const jit = r.jitter_ms
  if (lat == null) return ''
  if (lat < 30 && (jit == null || jit < 5) && r.mbps >= 50) {
    return '链路质量良好，视频会议与远程桌面可以正常使用。'
  }
  if (lat >= 100 || (jit != null && jit >= 50)) {
    return '延迟或抖动偏高，实时音视频可能卡顿；若目标是国内站点，建议排查本地网络或运营商链路。'
  }
  if (r.mbps < 20) {
    return '带宽偏低，大文件传输会比较慢；若为共享网络请避开高峰时段复测。'
  }
  return '链路质量中等，日常使用无明显问题。'
})

const summary = computed(() => {
  const r = res.value
  if (!r) return ''
  const parts = [`下载 ${r.mbps} Mbps`]
  if (r.latency_ms != null) parts.push(`延迟 ${r.latency_ms} ms`)
  if (r.jitter_ms != null) parts.push(`抖动 ${r.jitter_ms} ms`)
  if (r.loss_pct != null) parts.push(`丢包 ${r.loss_pct}%`)
  parts.push(`数据量 ${(r.bytes / 1048576).toFixed(1)} MB / ${r.duration_s}s`)
  return parts.join(' · ')
})

const report = computed(() => {
  const r = res.value
  if (!r) return ''
  return [
    `网络测速 · ${r.source}`,
    `时间：${new Date().toLocaleString('zh-CN')}`,
    r.host ? `测速源主机：${r.host}` : '',
    '',
    `下载速度：${r.mbps} Mbps（${r.kbps} KB/s）`,
    `延迟：${r.latency_ms ?? '—'} ms`,
    `抖动：${r.jitter_ms ?? '—'} ms`,
    `丢包：${r.loss_pct ?? '—'}%`,
    `数据量：${(r.bytes / 1048576).toFixed(1)} MB / 耗时 ${r.duration_s}s`,
    '',
    verdict.value,
  ]
    .filter(Boolean)
    .join('\n')
})
</script>

<template>
  <div class="net-tool">
    <div class="net-form">
      <div class="net-field grow">
        <label>测速源</label>
        <select v-model="form.source" class="input">
          <option v-for="s in SOURCES" :key="s.value" :value="s.value">{{ s.label }}</option>
        </select>
      </div>
      <button class="btn primary" :disabled="state === 'running'" @click="run">
        <Icon name="play" :size="12" /> {{ state === 'running' ? '测速中…' : '开始测速' }}
      </button>
    </div>

    <ResultBox
      :state="state"
      :title="
        state === 'running'
          ? `正在测速（已等待 ${waited}s，先探延迟约 6s，再下载 10MB）`
          : res
            ? `${res.source} · 下载 ${res.mbps} Mbps`
            : ''
      "
      :summary="summary"
      :error="err"
      :raw="''"
      :report="report"
      :elapsed="elapsed"
      name="speedtest-report"
      empty="选择测速源后开始。默认走国内镜像，Cloudflare 需要国际出口可用"
    >
      <template v-if="res">
        <div class="stat-grid">
          <div class="stat-card">
            <div class="sc-num" :class="rate('mbps', res.mbps)?.cls">
              {{ res.mbps }}<small> Mbps</small>
            </div>
            <div class="sc-label">
              下载带宽 · {{ rate('mbps', res.mbps)?.label || '—' }}
            </div>
          </div>
          <div class="stat-card">
            <div class="sc-num" :class="rate('latency', res.latency_ms)?.cls">
              {{ res.latency_ms ?? '—' }}<small> ms</small>
            </div>
            <div class="sc-label">
              延迟 · {{ rate('latency', res.latency_ms)?.label || '未测到' }}
            </div>
          </div>
          <div class="stat-card">
            <div class="sc-num" :class="rate('jitter', res.jitter_ms)?.cls">
              {{ res.jitter_ms ?? '—' }}<small> ms</small>
            </div>
            <div class="sc-label">
              抖动 · {{ rate('jitter', res.jitter_ms)?.label || '未测到' }}
            </div>
          </div>
          <div class="stat-card">
            <div class="sc-num" :class="res.loss_pct > 0 ? 'bad' : 'ok'">
              {{ res.loss_pct ?? '—' }}<small>%</small>
            </div>
            <div class="sc-label">丢包</div>
          </div>
          <div class="stat-card">
            <div class="sc-num">
              {{ (res.bytes / 1048576).toFixed(1) }}<small> MB</small>
            </div>
            <div class="sc-label">数据量 · 耗时 {{ res.duration_s }}s</div>
          </div>
        </div>
        <div v-if="verdict" class="net-msg" :class="res.mbps >= 50 && (res.latency_ms ?? 0) < 50 ? 'ok' : 'warn'">
          {{ verdict }}
        </div>
      </template>
    </ResultBox>
  </div>
</template>
