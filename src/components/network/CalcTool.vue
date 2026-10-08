<script setup>
/**
 * 网络计算：子网划分 + 弱电预算（供电 / 散热 / 布线 / PoE）。
 *
 * 纯本地计算，不发请求。输入一律"严格拒绝 + 说清原因"，
 * 不做静默兜底——原实现里 `+prefix || 24` 会把用户输入的 0 悄悄改成 24。
 */
import { computed, reactive, ref, watch } from 'vue'
import Icon from '../Icon.vue'
import { toast } from '../../store'
import { loadPref, savePref } from '../../netPrefs'

const TABS = [
  { key: 'subnet', label: '子网划分', icon: 'globe' },
  { key: 'elec', label: '弱电预算', icon: 'bolt' },
]
const tab = ref(loadPref('calc.tab', 'subnet'))
watch(tab, (v) => savePref('calc.tab', v))

// ---------------- 子网划分 ----------------

const sn = reactive({
  ip: loadPref('subnet.ip', '192.168.1.1'),
  prefix: Number(loadPref('subnet.prefix', 24)),
})
const snRes = ref(null)

watch(
  () => [sn.ip, sn.prefix],
  () => {
    savePref('subnet.ip', sn.ip)
    savePref('subnet.prefix', sn.prefix)
  }
)

const toIp = (n) => `${(n >>> 24) & 255}.${(n >>> 16) & 255}.${(n >>> 8) & 255}.${n & 255}`

function calcSubnet() {
  const parts = String(sn.ip).trim().split('.')
  if (parts.length !== 4) {
    toast('IP 格式错误，应形如 192.168.1.1', 'err')
    snRes.value = null
    return
  }
  const nums = parts.map((p) => Number(p))
  if (nums.some((n) => !Number.isInteger(n) || n < 0 || n > 255)) {
    toast('IP 每段必须是 0–255 的整数', 'err')
    snRes.value = null
    return
  }
  // 注意不能写成 `Number(...) || 24`：那样输入 0 会被静默改成 24
  const raw = Number(sn.prefix)
  if (!Number.isInteger(raw) || raw < 0 || raw > 32) {
    toast('前缀长度必须是 0–32 的整数', 'err')
    snRes.value = null
    return
  }
  const prefix = raw
  const mask = prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0
  const num = ((nums[0] << 24) | (nums[1] << 16) | (nums[2] << 8) | nums[3]) >>> 0
  const net = (num & mask) >>> 0
  const bcast = (net | (~mask >>> 0)) >>> 0

  // /31 是点对点链路（两个地址都可用），/32 是单主机——都不能按 2^n-2 算
  let usable
  let range
  if (prefix === 32) {
    usable = 1
    range = toIp(net)
  } else if (prefix === 31) {
    usable = 2
    range = `${toIp(net)} - ${toIp(bcast)}`
  } else {
    usable = 2 ** (32 - prefix) - 2
    range = `${toIp(net + 1)} - ${toIp(bcast - 1)}`
  }

  snRes.value = {
    ip: String(sn.ip).trim(),
    prefix,
    mask: toIp(mask),
    network: toIp(net),
    broadcast: prefix >= 31 ? '—（无广播地址）' : toIp(bcast),
    usable,
    range,
    isPrivate: /^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(String(sn.ip).trim()),
  }
}

// ---------------- 弱电预算 ----------------

const elec = reactive({
  v: Number(loadPref('elec.v', 12)),
  i: Number(loadPref('elec.i', 2)),
  n: Number(loadPref('elec.n', 8)),
  w: Number(loadPref('elec.w', 500)),
  rooms: Number(loadPref('elec.rooms', 30)),
  per: Number(loadPref('elec.per', 60)),
  cams: Number(loadPref('elec.cams', 16)),
  cw: Number(loadPref('elec.cw', 15)),
})

const r1 = (n) => Math.round(n * 10) / 10
const safe = (n) => (Number.isFinite(n) && n > 0 ? n : 0)

const elecOut = computed(() => {
  const power = safe(elec.v) * safe(elec.i) * safe(elec.n)
  const cableTotal = safe(elec.rooms) * safe(elec.per)
  const cableLoss = cableTotal * 1.1
  const poe = safe(elec.cams) * safe(elec.cw) * 1.1
  return {
    power: { total: r1(power), recommend: r1(power * 1.5) },
    cooling: { btu: r1(safe(elec.w) * 3.412), kw: r1(safe(elec.w) / 1000) },
    cable: { total: r1(cableTotal), loss: r1(cableLoss), boxes: Math.ceil(cableLoss / 305) },
    poe: { total: r1(poe), recommend: r1(poe * 1.3) },
  }
})

watch(
  () => ({ ...elec }),
  (v) => {
    for (const k of Object.keys(v)) savePref('elec.' + k, v[k])
  },
  { deep: true }
)
</script>

<template>
  <div class="net-tool">
    <div class="net-tabs">
      <button
        v-for="t in TABS"
        :key="t.key"
        class="net-tab"
        :class="{ active: tab === t.key }"
        @click="tab = t.key"
      >
        <Icon :name="t.icon" :size="13" /> {{ t.label }}
      </button>
    </div>

    <!-- 子网划分 -->
    <div v-if="tab === 'subnet'" class="net-tool">
      <div class="net-form">
        <div class="net-field">
          <label>IP 地址</label>
          <input v-model="sn.ip" class="input" placeholder="192.168.1.1" @keyup.enter="calcSubnet" />
        </div>
        <div class="net-field sm">
          <label>前缀长度</label>
          <input v-model.number="sn.prefix" class="input" type="number" min="0" max="32" />
        </div>
        <button class="btn primary" @click="calcSubnet">
          <Icon name="calculator" :size="12" /> 计算
        </button>
      </div>

      <table v-if="snRes" class="tbl">
        <tbody>
          <tr>
            <td class="dim" style="width: 130px">IP 地址</td>
            <td class="mono">{{ snRes.ip }}</td>
          </tr>
          <tr>
            <td class="dim">子网掩码</td>
            <td class="mono">{{ snRes.mask }} <span class="dim">/ {{ snRes.prefix }}</span></td>
          </tr>
          <tr>
            <td class="dim">网络地址</td>
            <td class="mono ok">{{ snRes.network }}</td>
          </tr>
          <tr>
            <td class="dim">广播地址</td>
            <td class="mono warn">{{ snRes.broadcast }}</td>
          </tr>
          <tr>
            <td class="dim">可用主机数</td>
            <td class="mono">{{ snRes.usable.toLocaleString() }}</td>
          </tr>
          <tr>
            <td class="dim">可用范围</td>
            <td class="mono">{{ snRes.range }}</td>
          </tr>
          <tr>
            <td class="dim">地址类型</td>
            <td>
              <span class="tag" :class="snRes.isPrivate ? 'ok' : ''">
                {{ snRes.isPrivate ? '私有地址（内网）' : '公网地址' }}
              </span>
            </td>
          </tr>
        </tbody>
      </table>
      <div v-else class="net-msg">填写 IP 与前缀长度后点计算。/31 为点对点链路，/32 为单主机，均不适用"减 2"规则。</div>
    </div>

    <!-- 弱电预算 -->
    <div v-else class="elec-grid">
      <div class="card">
        <div class="card-title">供电功率 P = U × I × N</div>
        <div class="net-form tight">
          <div class="net-field sm"><label>电压 V</label><input v-model.number="elec.v" class="input" type="number" /></div>
          <div class="net-field sm"><label>电流 A</label><input v-model.number="elec.i" class="input" type="number" /></div>
          <div class="net-field sm"><label>设备数</label><input v-model.number="elec.n" class="input" type="number" /></div>
        </div>
        <div class="net-msg ok">
          总功率 {{ elecOut.power.total }} W · 建议电源 {{ elecOut.power.recommend }} W（含 50% 余量）
        </div>
      </div>

      <div class="card">
        <div class="card-title">机柜散热估算</div>
        <div class="net-form tight">
          <div class="net-field sm"><label>设备总功率 W</label><input v-model.number="elec.w" class="input" type="number" /></div>
        </div>
        <div class="net-msg ok">
          发热量 {{ elecOut.cooling.btu }} BTU/h ≈ {{ elecOut.cooling.kw }} kW
          <template v-if="elec.w > 2000"> · 超过 2kW，建议机柜空调</template>
        </div>
      </div>

      <div class="card">
        <div class="card-title">网络布线预算（六类线）</div>
        <div class="net-form tight">
          <div class="net-field sm"><label>点位数量</label><input v-model.number="elec.rooms" class="input" type="number" /></div>
          <div class="net-field sm"><label>平均每点 m</label><input v-model.number="elec.per" class="input" type="number" /></div>
        </div>
        <div class="net-msg ok">
          线材 {{ elecOut.cable.total }} m · 含 10% 损耗 {{ elecOut.cable.loss }} m ≈
          {{ elecOut.cable.boxes }} 箱（305m/箱）
        </div>
      </div>

      <div class="card">
        <div class="card-title">PoE 供电预算</div>
        <div class="net-form tight">
          <div class="net-field sm"><label>摄像头数</label><input v-model.number="elec.cams" class="input" type="number" /></div>
          <div class="net-field sm"><label>每台功率 W</label><input v-model.number="elec.cw" class="input" type="number" /></div>
        </div>
        <div class="net-msg ok">
          总功率 {{ elecOut.poe.total }} W（含线损）· 推荐 PoE 交换机
          {{ elecOut.poe.recommend }} W（×1.3 安全系数）
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.net-tabs { display: flex; gap: 8px; }
.net-tab {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 7px 13px;
  border-radius: 8px;
  border: 1px solid var(--border);
  background: rgba(255, 255, 255, 0.03);
  color: var(--text-dim);
  font-size: 12.5px;
  font-family: inherit;
  cursor: pointer;
  transition: all 0.15s;
}
.net-tab:hover { background: rgba(255, 255, 255, 0.06); color: var(--text); }
.net-tab.active {
  background: rgba(34, 211, 238, 0.13);
  border-color: rgba(34, 211, 238, 0.34);
  color: var(--cyan);
  font-weight: 600;
}
.elec-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 14px; }
.card-title { font-weight: 600; font-size: 13px; margin-bottom: 12px; }
.card-title { color: var(--text); }
.net-form.tight { margin-bottom: 10px; }
.net-msg.warn { color: var(--warn); }
</style>
