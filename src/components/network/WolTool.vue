<script setup>
/**
 * WOL 网络唤醒：向目标网卡广播魔术包。
 * 成败取决于对方机器是否开启 WOL 且走有线网络，界面上把前提讲清楚，
 * 避免用户误判为工具失效。
 */
import { ref, watch } from 'vue'
import Icon from '../Icon.vue'
import { toast, cleanErr } from '../../store'
import { api } from '../../api'
import { loadPref, savePref } from '../../netPrefs'

const mac = ref(loadPref('wol.mac', ''))
const state = ref('idle')
const msg = ref('')

watch(mac, (v) => savePref('wol.mac', v))

async function send() {
  if (state.value === 'running') return
  const v = mac.value.trim()
  if (!v) {
    toast('请填写目标设备 MAC', 'err')
    return
  }
  state.value = 'running'
  msg.value = ''
  try {
    msg.value = await api.netWol(v)
    state.value = 'ok'
    toast('魔术包已发送')
  } catch (e) {
    msg.value = cleanErr(e)
    state.value = 'err'
    toast('发送失败: ' + msg.value, 'err')
  }
}
</script>

<template>
  <div class="net-tool">
    <div class="net-form">
      <div class="net-field grow">
        <label>目标设备 MAC 地址</label>
        <input
          v-model="mac"
          class="input"
          placeholder="00:11:22:33:44:55"
          @keyup.enter="send"
        />
      </div>
      <button class="btn primary" :disabled="state === 'running'" @click="send">
        <Icon name="power" :size="12" /> {{ state === 'running' ? '发送中…' : '发送魔术包' }}
      </button>
    </div>

    <div v-if="msg" class="net-msg" :class="state === 'err' ? 'err' : 'ok'">{{ msg }}</div>

    <div class="card wol-note">
      <div class="wol-t">使用前提</div>
      <ul class="wol-list">
        <li>目标设备已在 BIOS / 网卡驱动中开启 Wake-on-LAN</li>
        <li>目标使用有线网络（无线网卡通常不支持 WOL）</li>
        <li>与本机处于同一广播域，或路由器已配置定向广播转发</li>
        <li>魔术包会广播到 255.255.255.255 与 192.168.255.255 的 9 / 7 端口</li>
      </ul>
      <div class="wol-tip">
        没反应时不要急着怀疑工具：先在本机对该 MAC 做一次「主机发现」，
        确认它此前在线过（ARP 表里留有记录），说明广播链路是通的。
      </div>
    </div>
  </div>
</template>

<style scoped>
.wol-note { display: flex; flex-direction: column; gap: 9px; }
.wol-t { font-weight: 600; font-size: 13px; }
.wol-list { margin: 0; padding-left: 18px; display: flex; flex-direction: column; gap: 5px; }
.wol-list li { font-size: 12.5px; color: var(--text-dim); line-height: 1.6; }
.wol-tip {
  font-size: 12px;
  color: var(--text-faint);
  border-left: 2px solid var(--border-2);
  padding-left: 10px;
  line-height: 1.7;
}
</style>
