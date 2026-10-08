<script setup>
/**
 * 路由表：IPv4 路由条目与默认网关。
 * 默认路由单独高亮——排查"流量走哪张网卡出去"时第一眼看的就是它。
 */
import { computed, onMounted, ref } from 'vue'
import Icon from '../Icon.vue'
import ResultBox from './ResultBox.vue'
import { api } from '../../api'
import { toast, cleanErr } from '../../store'

const state = ref('idle')
const res = ref(null)
const err = ref('')

async function load() {
  state.value = 'running'
  err.value = ''
  try {
    res.value = await api.netRouteTable()
    state.value = 'ok'
  } catch (e) {
    err.value = cleanErr(e)
    state.value = 'err'
    toast('读取路由表失败: ' + err.value, 'err')
  }
}

onMounted(load)

const defaults = computed(
  () => res.value?.routes.filter((r) => r.dest === '0.0.0.0' || r.dest === 'default') || []
)
const gw = computed(() => defaults.value[0]?.gateway || '')

const summary = computed(() => {
  const r = res.value
  if (!r) return ''
  if (!r.count) return '路由表为空，或系统输出格式有变化——请展开「原文」核对。'
  const parts = [`${r.count} 条路由`]
  if (defaults.value.length) parts.push(`默认网关 ${gw.value}`)
  else parts.push('未找到默认路由（可能未连接网络）')
  return parts.join(' · ')
})

const report = computed(() => {
  const r = res.value
  if (!r) return ''
  return [
    'IPv4 路由表',
    `时间：${new Date().toLocaleString('zh-CN')}`,
    `共 ${r.count} 条 · 默认网关 ${gw.value || '无'}`,
    '',
    '目标网络\t网关\t掩码\t接口\t跃点',
    ...r.routes.map((x) => `${x.dest}\t${x.gateway}\t${x.mask}\t${x.iface}\t${x.metric}`),
  ].join('\n')
})
</script>

<template>
  <div class="net-tool">
    <div class="net-form">
      <button class="btn primary" :disabled="state === 'running'" @click="load">
        <Icon name="refresh" :size="12" /> {{ state === 'running' ? '读取中…' : '刷新路由表' }}
      </button>
      <span class="net-inline-note">读取 route print -4</span>
    </div>

    <ResultBox
      :state="state"
      :title="res ? `IPv4 路由表 · ${res.count} 条` : '正在读取…'"
      :summary="summary"
      :error="err"
      :raw="''"
      :report="report"
      name="route-report"
      empty="点击刷新读取路由表"
    >
      <table v-if="res" class="tbl">
        <thead>
          <tr>
            <th>目标网络</th>
            <th>网关</th>
            <th>掩码</th>
            <th>接口</th>
            <th style="width: 70px">跃点</th>
          </tr>
        </thead>
        <tbody>
          <tr v-if="!res.routes.length">
            <td colspan="5" class="dim">无路由数据</td>
          </tr>
          <tr
            v-for="(r, i) in res.routes"
            :key="i"
            :class="{ 'row-default': r.dest === '0.0.0.0' || r.dest === 'default' }"
          >
            <td class="mono">{{ r.dest }}</td>
            <td class="mono" :class="{ ok: r.dest === '0.0.0.0' || r.dest === 'default' }">
              {{ r.gateway }}
            </td>
            <td class="mono dim">{{ r.mask }}</td>
            <td class="mono dim">{{ r.iface }}</td>
            <td class="dim">{{ r.metric }}</td>
          </tr>
        </tbody>
      </table>
    </ResultBox>
  </div>
</template>

<style scoped>
.row-default { background: rgba(34, 211, 238, 0.06); }
</style>
