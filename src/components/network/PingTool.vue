<script setup>
/**
 * Ping：连通性、延迟波动与丢包。
 *
 * 相比此前：统计数字收了信息密度（5 张小卡而不是 4 张占满整行）、
 * 波形图补上纵轴刻度与上限标注（否则看不出波动幅度到底多大）、
 * 长任务显示已等待秒数（ping 是阻塞调用，拿不到中间进度但至少要让人知道没卡死）。
 */
import { computed, onUnmounted, reactive, ref, watch } from 'vue'
import Icon from '../Icon.vue'
import ResultBox from './ResultBox.vue'
import { api } from '../../api'
import { toast, cleanErr } from '../../store'
import { COMMON_TARGETS, loadPref, savePref } from '../../netPrefs'

const form = reactive({
  host: loadPref('ping.host', '223.5.5.5'),
  count: Number(loadPref('ping.count', 8)) || 8,
  size: Number(loadPref('ping.size', 32)) || 32,
})

const state = ref('idle')
const res = ref(null)
const err = ref('')
const elapsed = ref(0)
const waited = ref(0)

let timer = null

watch(() => form.host, (v) => savePref('ping.host', v))
watch(
  () => [form.count, form.size],
  () => {
    savePref('ping.count', form.count)
    savePref('ping.size', form.size)
  }
)
onUnmounted(() => {
  if (timer) clearInterval(timer)
})

async function run() {
  if (state.value === 'running') return
  const host = form.host.trim()
  if (!host) {
    toast('请填写目标地址', 'err')
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
    res.value = await api.netPing(
      host,
      Math.min(Math.max(+form.count || 4, 1), 50),
      Math.min(Math.max(+form.size || 32, 1), 1400)
    )
    elapsed.value = Date.now() - t0
    state.value = 'ok'
  } catch (e) {
    err.value = cleanErr(e)
    state.value = 'err'
    toast('Ping 失败: ' + err.value, 'err')
  } finally {
    clearInterval(timer)
    timer = null
  }
}

/** 抖动：相邻两次 RTT 的平均绝对差，比标准差更贴近"卡顿感" */
const stats = computed(() => {
  const t = res.value?.times || []
  if (t.length < 2) return { jitter: null }
  const d = t.slice(1).reduce((a, v, i) => a + Math.abs(v - t[i]), 0) / (t.length - 1)
  return { jitter: Math.round(d * 10) / 10 }
})

/** 纵轴从 0 起算（否则小波动会被放大成"剧烈抖动"），上限取整刻度 */
const CHART = { w: 640, h: 152, padL: 44, padR: 14, padT: 14, padB: 22 }

/**
 * 挑一组"整齐"的纵轴刻度：4 等分且步长是整数。
 * 早期实现用 top 的 0/25%/50%/75%/100% 直接四舍五入，延迟普遍在个位数毫秒时
 * 会画出 `0 1 2 3 3` 这种重复标签（上限 3.45 → 最后一格也是 3），一眼就很业余。
 */
function niceAxis(rawMax) {
  const steps = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000]
  const need = Math.max(rawMax * 1.15, 1) // 留 15% 余量，曲线不贴顶
  const step = steps.find((s) => s * 4 >= need) || Math.ceil(need / 4)
  return { step, top: step * 4 }
}

const chart = computed(() => {
  const times = res.value?.times || []
  if (!times.length) return null
  const { step: tickStep, top } = niceAxis(Math.max(...times))
  const innerW = CHART.w - CHART.padL - CHART.padR
  const innerH = CHART.h - CHART.padT - CHART.padB
  const gap = times.length > 1 ? innerW / (times.length - 1) : 0
  const x = (i) => CHART.padL + i * gap
  const y = (v) => CHART.padT + innerH - (v / top) * innerH
  const avg = times.reduce((a, b) => a + b, 0) / times.length
  const ticks = [0, 1, 2, 3, 4].map((i) => ({ v: i * tickStep, y: y(i * tickStep) }))
  // 横轴标号：样本多时抽稀，避免数字糊成一片
  const stride = Math.ceil(times.length / 10)
  const xLabels = times
    .map((_, i) => i)
    .filter((i) => i % stride === 0 || i === times.length - 1)
    .map((i) => ({ i, label: String(i + 1), x: x(i) }))
  return {
    top,
    avg,
    ticks,
    xLabels,
    avgY: y(avg),
    pts: times.map((v, i) => `${x(i)},${y(v)}`).join(' '),
    nodes: times.map((v, i) => ({ x: x(i), y: y(v), v, i })),
  }
})

/** 延迟着色阈值按国内网络调：<30ms 优秀、<80ms 可用、再高就该关注了 */
function rttClass(v) {
  if (v == null) return ''
  if (v < 30) return 'ok'
  if (v < 80) return 'warn'
  return 'bad'
}
function rttColor(v) {
  if (v == null) return '#5c6b82'
  if (v < 30) return '#34d399'
  if (v < 80) return '#fbbf24'
  return '#f87171'
}

const boxTitle = computed(() => {
  if (state.value === 'running') {
    return `正在 Ping ${form.host}（已等待 ${waited.value}s，${form.count} 次探测最长约 ${form.count * 2}s）`
  }
  if (state.value === 'ok' && res.value) {
    return `${form.host} · 发包 ${res.value.sent} · 收到 ${res.value.received}`
  }
  return ''
})

const summary = computed(() => {
  const r = res.value
  if (!r) return ''
  if (r.received === 0) return '全部超时：目标不可达或屏蔽了 ICMP。可换 TCP 端口扫描验证是否真的不通。'
  const parts = [
    `平均 ${r.avg_ms} ms`,
    `最小 ${r.min_ms} ms`,
    `最大 ${r.max_ms} ms`,
    `丢包 ${r.loss_pct}%`,
  ]
  if (stats.value.jitter != null) parts.push(`抖动 ${stats.value.jitter} ms`)
  return parts.join(' · ')
})

/** 复制/导出的纯文本报告 */
const report = computed(() => {
  const r = res.value
  if (!r) return ''
  const L = [
    `Ping ${form.host}`,
    `时间：${new Date().toLocaleString('zh-CN')}`,
    `参数：${r.sent} 次 · ${form.size} 字节`,
    '',
    `最小 ${r.min_ms ?? '—'} ms · 平均 ${r.avg_ms ?? '—'} ms · 最大 ${r.max_ms ?? '—'} ms · 丢包 ${r.loss_pct}%`,
  ]
  if (stats.value.jitter != null) L.push(`抖动 ${stats.value.jitter} ms`)
  L.push('', `逐次延迟（ms）：${r.times.join(', ') || '无'}`, '', '原始输出：', r.raw)
  return L.join('\n')
})

function pick(v) {
  form.host = v
}

// 首次进入自动跑一次：打开就能看到网络状况，不用先点一次按钮
run()
</script>

<template>
  <div class="net-tool">
    <div class="net-form">
      <div class="net-field grow">
        <label>目标地址</label>
        <input
          v-model="form.host"
          class="input"
          placeholder="IP 或域名，如 223.5.5.5"
          @keyup.enter="run"
        />
      </div>
      <div class="net-field sm">
        <label>次数</label>
        <input v-model.number="form.count" class="input" type="number" min="1" max="50" />
      </div>
      <div class="net-field sm">
        <label>包大小 B</label>
        <input v-model.number="form.size" class="input" type="number" min="1" max="1400" />
      </div>
      <button class="btn primary" :disabled="state === 'running'" @click="run">
        <Icon name="play" :size="12" /> {{ state === 'running' ? '测试中…' : '开始' }}
      </button>
    </div>

    <div class="net-chips">
      <span class="chips-label">常用目标</span>
      <button v-for="c in COMMON_TARGETS" :key="c.value" class="chip-btn" @click="pick(c.value)">
        {{ c.label }}
      </button>
    </div>

    <ResultBox
      :state="state"
      :title="boxTitle"
      :summary="summary"
      :error="err"
      :raw="res?.raw || ''"
      :report="report"
      :elapsed="elapsed"
      name="ping-report"
      empty="填写目标地址后开始 Ping"
    >
      <template v-if="res && res.times.length">
        <div class="stat-grid">
          <div class="stat-card">
            <div class="sc-num" :class="rttClass(res.avg_ms)">
              {{ res.avg_ms ?? '—' }}<small> ms</small>
            </div>
            <div class="sc-label">平均延迟</div>
          </div>
          <div class="stat-card">
            <div class="sc-num" :class="res.loss_pct > 0 ? 'bad' : 'ok'">
              {{ res.loss_pct }}<small>%</small>
            </div>
            <div class="sc-label">{{ res.received }}/{{ res.sent }} 收到</div>
          </div>
          <div class="stat-card">
            <div class="sc-num">{{ res.min_ms ?? '—' }}<small> ms</small></div>
            <div class="sc-label">最小</div>
          </div>
          <div class="stat-card">
            <div class="sc-num">{{ res.max_ms ?? '—' }}<small> ms</small></div>
            <div class="sc-label">最大</div>
          </div>
          <div class="stat-card">
            <div class="sc-num" :class="rttClass(stats.jitter)">
              {{ stats.jitter ?? '—' }}<small> ms</small>
            </div>
            <div class="sc-label">抖动</div>
          </div>
        </div>

        <svg v-if="chart" :viewBox="`0 0 ${CHART.w} ${CHART.h}`" class="net-chart">
          <line
            v-for="(t, i) in chart.ticks"
            :key="'g' + i"
            :x1="CHART.padL"
            :y1="t.y"
            :x2="CHART.w - CHART.padR"
            :y2="t.y"
            stroke="#1e2b3f"
            stroke-width="1"
          />
          <text
            v-for="(t, i) in chart.ticks"
            :key="'l' + i"
            :x="CHART.padL - 8"
            :y="t.y + 3.5"
            text-anchor="end"
            class="net-axis"
          >
            {{ t.v.toFixed(0) }}
          </text>
          <line
            :x1="CHART.padL"
            :y1="chart.avgY"
            :x2="CHART.w - CHART.padR"
            :y2="chart.avgY"
            stroke="#34d399"
            stroke-width="1"
            stroke-dasharray="4 4"
          />
          <polyline
            :points="chart.pts"
            fill="none"
            stroke="#22d3ee"
            stroke-width="2"
            stroke-linejoin="round"
          />
          <circle
            v-for="n in chart.nodes"
            :key="n.i"
            :cx="n.x"
            :cy="n.y"
            r="2.6"
            :fill="rttColor(n.v)"
          />
          <text
            v-for="l in chart.xLabels"
            :key="'x' + l.i"
            :x="l.x"
            :y="CHART.h - 5"
            text-anchor="middle"
            class="net-axis"
          >
            {{ l.label }}
          </text>
        </svg>
        <div v-if="chart" class="net-chart-cap">
          横轴为第 n 次探测 · 绿色虚线为平均 {{ chart.avg.toFixed(1) }}ms · 纵轴上限
          {{ chart.top.toFixed(0) }}ms · 点色按阈值区分
        </div>
      </template>
    </ResultBox>
  </div>
</template>
