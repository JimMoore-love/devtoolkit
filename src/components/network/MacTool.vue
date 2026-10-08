<script setup>
/**
 * MAC 与厂商：按 OUI 前缀判断设备厂商。
 * 纯本地计算，不发请求。支持完整 MAC（12 位）或仅 OUI（6 位）。
 */
import { ref, watch } from 'vue'
import Icon from '../Icon.vue'
import { toast } from '../../store'
import { lookupVendor, OUI_TABLE } from '../../networkData'
import { loadPref, savePref } from '../../netPrefs'

const input = ref(loadPref('mac.value', ''))
const res = ref(null)
const errMsg = ref('')

watch(input, (v) => savePref('mac.value', v))

function lookup() {
  errMsg.value = ''
  res.value = null
  const clean = input.value.replace(/[^0-9a-fA-F]/g, '').toUpperCase()
  if (clean.length !== 12 && clean.length !== 6) {
    errMsg.value =
      clean.length === 0
        ? '请输入 MAC 地址'
        : `解析出 ${clean.length} 位十六进制，需要 6 位（OUI）或 12 位（完整 MAC）`
    toast('MAC 格式错误，示例 00:0C:29:D1:4B:07', 'err')
    return
  }
  const oui = clean.slice(0, 6)
  res.value = { oui, vendor: lookupVendor(clean), full: clean }
}

const knownCount = Object.keys(OUI_TABLE).length
</script>

<template>
  <div class="net-tool">
    <div class="net-form">
      <div class="net-field grow">
        <label>MAC 地址</label>
        <input
          v-model="input"
          class="input"
          placeholder="00:0C:29:D1:4B:07 或只填前 6 位 000C29"
          @keyup.enter="lookup"
        />
      </div>
      <button class="btn primary" @click="lookup">
        <Icon name="search" :size="12" /> 查询厂商
      </button>
    </div>

    <div class="net-chips">
      <span class="chips-note">内置 {{ knownCount }} 条常见 OUI 前缀，纯本地匹配，不联网</span>
    </div>

    <div v-if="errMsg" class="net-msg err">{{ errMsg }}</div>

    <div v-else-if="res" class="card mac-card">
      <div class="mac-line">
        <span class="mac-k">OUI 前缀</span>
        <span class="mono mac-oui">{{ res.oui }}</span>
      </div>
      <div class="mac-line">
        <span class="mac-k">厂商</span>
        <span
          class="mac-vendor"
          :class="{ unknown: res.vendor === '未知厂商' }"
        >{{ res.vendor }}</span>
      </div>
      <div v-if="res.vendor === '未知厂商'" class="net-msg warn">
        本地库未收录该前缀。可到 IEEE 的 OUI 公开列表核对，注意本地库不联网、更新随版本发布。
      </div>
    </div>
  </div>
</template>

<style scoped>
.mac-card { display: flex; flex-direction: column; gap: 10px; }
.mac-line { display: flex; align-items: baseline; gap: 14px; }
.mac-k { font-size: 12.5px; color: var(--text-faint); width: 74px; flex-shrink: 0; }
.mac-oui { font-size: 16px; font-weight: 700; color: var(--cyan); user-select: text; }
.mac-vendor { font-size: 15px; font-weight: 600; color: var(--ok); user-select: text; }
.mac-vendor.unknown { color: var(--text-dim); font-weight: 500; }
</style>
