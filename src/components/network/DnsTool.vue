<script setup>
/**
 * DNS 查询：A / AAAA / CNAME / MX / NS / TXT / SOA。
 *
 * 这是网络工具箱此前完全缺失的一项——排查"域名解析到哪了、邮件为什么收不到"
 * 用的就是它。后端已把 nslookup 的中文输出解析成结构化记录，
 * 这里按类型表格化，并单独标出规范名与别名（A 记录查询时域名常被 CNAME 走）。
 */
import { computed, reactive, ref, watch } from 'vue'
import Icon from '../Icon.vue'
import ResultBox from './ResultBox.vue'
import { api } from '../../api'
import { toast, cleanErr } from '../../store'
import { loadPref, savePref } from '../../netPrefs'

const TYPES = ['A', 'AAAA', 'CNAME', 'MX', 'NS', 'TXT', 'SOA']

/** 常用域名：排查外部服务解析是否正常时的第一梯队 */
const COMMON_DOMAINS = [
  { label: '百度', value: 'www.baidu.com' },
  { label: '腾讯', value: 'qq.com' },
  { label: '阿里', value: 'www.aliyun.com' },
  { label: 'GitHub', value: 'github.com' },
]

const form = reactive({
  host: loadPref('dns.host', 'www.baidu.com'),
  rtype: loadPref('dns.rtype', 'A'),
})
const state = ref('idle')
const res = ref(null)
const err = ref('')

watch(() => form.host, (v) => savePref('dns.host', v))
watch(() => form.rtype, (v) => savePref('dns.rtype', v))

/**
 * 空结果的原因不同，给的话就该不同。
 * nslookup 查不到东西有两种截然相反的成因：域名真没有该类型记录，
 * 或者应答服务器压根没响应（超时）。后者常被误读成"域名不存在"，
 * 所以按原文里的超时特征分开提示。
 */
const emptyHint = computed(() => {
  const raw = res.value?.raw || ''
  if (/超时|timed out|unreachable/i.test(raw)) {
    return '查询超时：应答服务器没有响应。常见于解析指向了不可达的 DNS（如内网 DNS 掉线、依赖 VPN 的解析）。可换常用域名试一次，或检查本机 DNS 设置。'
  }
  return `没有解析到 ${res.value?.rtype} 记录。若域名确实存在，请展开「原文」查看 nslookup 的报错。`
})

async function run() {
  if (state.value === 'running') return
  const host = form.host.trim()
  if (!host) {
    toast('请填写域名', 'err')
    return
  }
  state.value = 'running'
  res.value = null
  err.value = ''
  try {
    res.value = await api.netDnsLookup(host, form.rtype)
    state.value = 'ok'
  } catch (e) {
    err.value = cleanErr(e)
    state.value = 'err'
    toast('DNS 查询失败: ' + err.value, 'err')
  }
}

const summary = computed(() => {
  const r = res.value
  if (!r) return ''
  const parts = []
  if (r.server) parts.push(`应答服务器 ${r.server}`)
  if (r.canonical) parts.push(`规范名 ${r.canonical}`)
  if (r.aliases.length) parts.push(`别名 ${r.aliases.join(', ')}`)
  parts.push(`耗时 ${r.elapsed_ms} ms`)
  return parts.join(' · ')
})

const report = computed(() => {
  const r = res.value
  if (!r) return ''
  const L = [
    `DNS 查询 ${r.host} (${r.rtype})`,
    `时间：${new Date().toLocaleString('zh-CN')}`,
    `服务器：${r.server || '未知'} · 耗时 ${r.elapsed_ms} ms`,
  ]
  if (r.canonical) L.push(`规范名：${r.canonical}`)
  if (r.aliases.length) L.push(`别名：${r.aliases.join(', ')}`)
  L.push('', '类型\t值\t说明')
  for (const a of r.answers) L.push(`${a.rtype}\t${a.value}\t${a.extra || '-'}`)
  L.push('', '原始输出：', r.raw)
  return L.join('\n')
})

function pick(v) {
  form.host = v
  run()
}
</script>

<template>
  <div class="net-tool">
    <div class="net-form">
      <div class="net-field grow">
        <label>域名</label>
        <input
          v-model="form.host"
          class="input"
          placeholder="如 www.baidu.com（不要带 http://）"
          @keyup.enter="run"
        />
      </div>
      <div class="net-field">
        <label>记录类型</label>
        <select v-model="form.rtype" class="input">
          <option v-for="t in TYPES" :key="t" :value="t">{{ t }}</option>
        </select>
      </div>
      <button class="btn primary" :disabled="state === 'running'" @click="run">
        <Icon name="play" :size="12" /> {{ state === 'running' ? '查询中…' : '查询' }}
      </button>
    </div>

    <div class="net-chips">
      <span class="chips-label">常用域名</span>
      <button v-for="d in COMMON_DOMAINS" :key="d.value" class="chip-btn" @click="pick(d.value)">
        {{ d.label }}
      </button>
    </div>

    <ResultBox
      :state="state"
      :title="res ? `${res.host} · ${res.rtype} 记录` : state === 'running' ? '正在查询…' : ''"
      :summary="summary"
      :error="err"
      :raw="res?.raw || ''"
      :report="report"
      :elapsed="res?.elapsed_ms || 0"
      name="dns-report"
      empty="填写域名后查询。A/AAAA 查地址，MX 查邮件服务器，TXT 常含 SPF 反垃圾配置"
    >
      <table v-if="res" class="tbl">
        <thead>
          <tr>
            <th style="width: 84px">类型</th>
            <th>值</th>
            <th style="width: 130px">说明</th>
          </tr>
        </thead>
        <tbody>
          <tr v-if="!res.answers.length">
            <td colspan="3" class="dim">
              {{ emptyHint }}
            </td>
          </tr>
          <tr v-for="(a, i) in res.answers" :key="i">
            <td><span class="tag ok">{{ a.rtype }}</span></td>
            <td class="mono">{{ a.value }}</td>
            <td class="dim">{{ a.extra || '—' }}</td>
          </tr>
        </tbody>
      </table>
    </ResultBox>
  </div>
</template>
