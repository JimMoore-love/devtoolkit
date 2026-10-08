<script setup>
/**
 * 主机发现：读 ARP 表列出同网段设备。
 * 保留原来的厂商分布条形图，表格补上"已知厂商"高亮与整表复制。
 */
import { computed, onMounted, ref } from 'vue'
import Icon from '../Icon.vue'
import ResultBox from './ResultBox.vue'
import { api } from '../../api'
import { toast, cleanErr } from '../../store'
import { lookupVendor } from '../../networkData'

const state = ref('idle')
const res = ref(null)
const err = ref('')

async function load() {
  state.value = 'running'
  err.value = ''
  try {
    res.value = await api.netArpHosts()
    state.value = 'ok'
  } catch (e) {
    err.value = cleanErr(e)
    state.value = 'err'
    toast('读取 ARP 表失败: ' + err.value, 'err')
  }
}

onMounted(load)

const vendorStats = computed(() => {
  if (!res.value) return []
  const m = {}
  for (const h of res.value.hosts) {
    const v = lookupVendor(h.mac)
    m[v] = (m[v] || 0) + 1
  }
  return Object.entries(m).sort((a, b) => b[1] - a[1])
})

const known = computed(() => res.value?.hosts.filter((h) => lookupVendor(h.mac) !== '未知厂商').length || 0)

const summary = computed(() => {
  const r = res.value
  if (!r) return ''
  if (!r.total) return 'ARP 表为空。先 ping 一下网关或访问网段内设备，再回来刷新。'
  const parts = [`发现 ${r.total} 台设备`, `${known.value} 台可识别厂商`]
  const uniqMac = new Set(r.hosts.map((h) => h.mac)).size
  if (uniqMac !== r.total) parts.push(`${uniqMac} 个唯一 MAC（有重复，可能是多网卡或虚拟网卡）`)
  return parts.join(' · ')
})

const report = computed(() => {
  const r = res.value
  if (!r) return ''
  return [
    'ARP 主机发现',
    `时间：${new Date().toLocaleString('zh-CN')}`,
    `共 ${r.total} 台设备`,
    '',
    'IP\tMAC\t状态\t厂商',
    ...r.hosts.map((h) => `${h.ip}\t${h.mac}\t${h.kind}\t${lookupVendor(h.mac)}`),
  ].join('\n')
})
</script>

<template>
  <div class="net-tool">
    <div class="net-form">
      <button class="btn primary" :disabled="state === 'running'" @click="load">
        <Icon name="refresh" :size="12" /> {{ state === 'running' ? '读取中…' : '刷新 ARP 表' }}
      </button>
      <span class="net-inline-note">ARP 表只包含近期通信过的设备，扫不到全部主机是正常的</span>
    </div>

    <ResultBox
      :state="state"
      :title="res ? `同网段设备 · ${res.total} 台` : '正在读取…'"
      :summary="summary"
      :error="err"
      :raw="''"
      :report="report"
      name="arp-report"
      empty="点击刷新读取 ARP 表"
    >
      <template v-if="res && res.total">
        <div v-if="vendorStats.length" class="vendor-bars">
          <div v-for="[v, c] in vendorStats" :key="v" class="vb-row">
            <span class="vb-name" :title="v">{{ v }}</span>
            <div class="vb-bar">
              <div class="vb-fill" :style="{ width: (c / res.total) * 100 + '%' }"></div>
            </div>
            <span class="vb-count">{{ c }}</span>
          </div>
        </div>

        <table class="tbl">
          <thead>
            <tr>
              <th>IP 地址</th>
              <th>MAC 地址</th>
              <th style="width: 190px">厂商</th>
              <th style="width: 90px">类型</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(h, i) in res.hosts" :key="i">
              <td class="mono">{{ h.ip }}</td>
              <td class="mono dim">{{ h.mac }}</td>
              <td>
                <span
                  class="tag"
                  :class="lookupVendor(h.mac) === '未知厂商' ? '' : 'ok'"
                >{{ lookupVendor(h.mac) }}</span>
              </td>
              <td class="dim">{{ h.kind }}</td>
            </tr>
          </tbody>
        </table>
      </template>
    </ResultBox>
  </div>
</template>

<style scoped>
.vendor-bars { display: flex; flex-direction: column; gap: 6px; margin-bottom: 14px; }
.vb-row { display: flex; align-items: center; gap: 10px; }
.vb-name {
  width: 150px;
  font-size: 12px;
  color: var(--text-dim);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.vb-bar { flex: 1; height: 8px; border-radius: 4px; background: rgba(255, 255, 255, 0.06); overflow: hidden; }
.vb-fill { height: 100%; background: linear-gradient(90deg, #22d3ee, #34d399); border-radius: 4px; }
.vb-count { width: 26px; text-align: right; font-size: 12px; font-family: var(--mono); color: var(--text-faint); }
</style>
