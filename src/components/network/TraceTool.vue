<script setup>
/**
 * 路由追踪：逐跳时延，定位瓶颈在哪一跳。
 *
 * 关键改动是后端现在返回结构化的 parsed（跳数 / 三次 RTT / 地址 / 超时），
 * 此前只是把 tracert 的整行文本塞进表格——有数据但等于没解析。
 * 另外补了"增量"列：相对上一有效跳新增的时延，这才是判断
 * "延迟是在哪一跳被引入的"依据。
 */
import { computed, onUnmounted, reactive, ref, watch } from 'vue'
import Icon from '../Icon.vue'
import ResultBox from './ResultBox.vue'
import { api } from '../../api'
import { toast, cleanErr } from '../../store'
import { COMMON_TARGETS, loadPref, savePref } from '../../netPrefs'

const form = reactive({ host: loadPref('trace.host', '223.5.5.5') })
const state = ref('idle')
const res = ref(null)
const err = ref('')
const elapsed = ref(0)
const waited = ref(0)

let timer = null

watch(() => form.host, (v) => savePref('trace.host', v))
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
    res.value = await api.netTraceroute(host)
    elapsed.value = Date.now() - t0
    state.value = 'ok'
  } catch (e) {
    err.value = cleanErr(e)
    state.value = 'err'
    toast('路由追踪失败: ' + err.value, 'err')
  } finally {
    clearInterval(timer)
    timer = null
  }
}

/**
 * 逐跳数据 + 增量。
 * 增量与该跳的"上一有效跳"比较，跳过全程超时的跳——否则一个 *
 * 会把后续所有跳的增量都算错。
 */
const hops = computed(() => {
  const list = res.value?.parsed || []
  let prev = null
  return list.map((h) => {
    let delta = null
    if (h.avg_ms != null) {
      if (prev != null) delta = Math.round((h.avg_ms - prev) * 10) / 10
      prev = h.avg_ms
    }
    return { ...h, delta }
  })
})

const timeouts = computed(() => hops.value.filter((h) => h.timed_out).length)

const summary = computed(() => {
  const r = res.value
  if (!r) return ''
  const parts = [`共 ${r.parsed.length} 跳`]
  if (!r.reached) parts.push('未走完全程（末跳可能是超时节点或被屏蔽）')
  if (timeouts.value) parts.push(`${timeouts.value} 跳全程超时（对方不响应 ICMP，属常见现象）`)
  if (r.slowest) parts.push(`最慢：${r.slowest}`)
  return parts.join(' · ')
})

/** 平均时延着色：逐跳累计，阈值比 ping 放宽一些 */
function hopClass(v) {
  if (v == null) return ''
  if (v < 30) return 'ok'
  if (v < 100) return 'warn'
  return 'bad'
}

function fmtRtt(v) {
  return v == null ? '*' : v < 1 ? '<1' : v.toFixed(1).replace(/\.0$/, '')
}

const report = computed(() => {
  const r = res.value
  if (!r) return ''
  const L = [
    `路由追踪 ${form.host}`,
    `时间：${new Date().toLocaleString('zh-CN')}`,
    `结果：${r.reached ? '走完全程' : '未确认走完全程'}`,
    '',
    '跳\t延迟1\t延迟2\t延迟3\t平均\t增量\t地址',
  ]
  for (const h of hops.value) {
    L.push(
      [
        h.idx,
        ...h.rtts.map((v) => fmtRtt(v)),
        h.avg_ms ?? '-',
        h.delta ?? '-',
        h.timed_out ? '*' : h.addr || '?',
      ].join('\t')
    )
  }
  L.push('', '原始输出：', r.raw)
  return L.join('\n')
})

function pick(v) {
  form.host = v
}
</script>

<template>
  <div class="net-tool">
    <div class="net-form">
      <div class="net-field grow">
        <label>目标地址</label>
        <input
          v-model="form.host"
          class="input"
          placeholder="IP 或域名"
          @keyup.enter="run"
        />
      </div>
      <button class="btn primary" :disabled="state === 'running'" @click="run">
        <Icon name="play" :size="12" /> {{ state === 'running' ? '追踪中…' : '开始追踪' }}
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
      :title="
        state === 'running'
          ? `正在追踪 ${form.host}（已等待 ${waited}s，最长约 30s）`
          : res
            ? `${form.host} · ${res.parsed.length} 跳`
            : ''
      "
      :summary="summary"
      :error="err"
      :raw="res?.raw || ''"
      :report="report"
      :elapsed="elapsed"
      name="trace-report"
      empty="填写目标地址后开始追踪（已关闭反向域名解析以加快速度）"
    >
      <table v-if="hops.length" class="tbl">
        <thead>
          <tr>
            <th style="width: 46px">跳</th>
            <th style="width: 168px">延迟 1 / 2 / 3</th>
            <th style="width: 74px">平均</th>
            <th style="width: 74px">增量</th>
            <th>地址</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="h in hops" :key="h.idx" :class="{ 'row-slow': res?.slowest?.includes(`第 ${h.idx} 跳`) }">
            <td class="num">{{ h.idx }}</td>
            <td class="mono dim">
              <span v-for="(v, i) in h.rtts" :key="i" :class="['rtt', hopClass(v)]">
                {{ fmtRtt(v) }}<i v-if="i < h.rtts.length - 1"> / </i>
              </span>
              <span v-if="!h.rtts.length" class="dim">—</span>
            </td>
            <td class="mono" :class="hopClass(h.avg_ms)">
              {{ h.avg_ms != null ? h.avg_ms + ' ms' : '—' }}
            </td>
            <td class="mono" :class="h.delta != null && h.delta > 30 ? 'warn' : 'dim'">
              {{ h.delta != null ? (h.delta > 0 ? '+' : '') + h.delta + ' ms' : '—' }}
            </td>
            <td class="mono">
              <span v-if="h.timed_out" class="dim">* 请求超时</span>
              <span v-else>{{ h.addr || '?' }}</span>
            </td>
          </tr>
        </tbody>
      </table>
      <div v-else-if="res" class="net-msg warn">
        没有解析出任何跳。可能是目标不可达，或系统 tracert 输出格式有变——请展开「原文」核对。
      </div>
    </ResultBox>
  </div>
</template>

<style scoped>
.rtt { font-family: var(--mono); }
.rtt i { color: var(--text-faint); font-style: normal; }
.rtt.ok { color: var(--ok); }
.rtt.warn { color: var(--warn); }
.rtt.bad { color: var(--danger); }
.row-slow { background: rgba(251, 191, 36, 0.07); }
</style>
