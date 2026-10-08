<script setup>
/**
 * 网络工具箱外壳：只负责"选工具 + 渲染对应面板"。
 *
 * 此前这是一个 466 行的单体组件——11 个工具平铺成两行标签栏、emoji 当图标、
 * 每个面板各写一套结果展示、追踪结果直接把整行文本塞进表格。
 * 现在：导航走分组侧栏，图标统一 SVG，结果统一走 ResultBox，
 * 具体功能拆到 ./network/ 下按工具分文件。
 */
import { computed, ref, watch } from 'vue'
import NetNav from './network/NetNav.vue'
import PingTool from './network/PingTool.vue'
import TraceTool from './network/TraceTool.vue'
import ScanTool from './network/ScanTool.vue'
import DnsTool from './network/DnsTool.vue'
import SpeedTool from './network/SpeedTool.vue'
import LocalInfoTool from './network/LocalInfoTool.vue'
import ArpTool from './network/ArpTool.vue'
import RouteTool from './network/RouteTool.vue'
import MacTool from './network/MacTool.vue'
import WolTool from './network/WolTool.vue'
import CalcTool from './network/CalcTool.vue'
import VendorCmdsTool from './network/VendorCmdsTool.vue'
import { DEFAULT_NET_TOOL, toolOf } from '../networkTools'
import { loadPref, savePref } from '../netPrefs'

/** 工具键 → 面板组件。新增工具时在这里补一行 */
const PANELS = {
  ping: PingTool,
  trace: TraceTool,
  scan: ScanTool,
  dns: DnsTool,
  speed: SpeedTool,
  local: LocalInfoTool,
  arp: ArpTool,
  route: RouteTool,
  mac: MacTool,
  wol: WolTool,
  calc: CalcTool,
  ref: VendorCmdsTool,
}

// 记住上次用的工具，下次打开直接回到那里
const tool = ref(loadPref('tool', DEFAULT_NET_TOOL))
watch(tool, (v) => savePref('tool', v))

const cur = computed(() => toolOf(tool.value))
const panel = computed(() => PANELS[tool.value] || PingTool)
</script>

<template>
  <div class="net-shell">
    <NetNav :current="tool" @pick="tool = $event" />
    <div class="net-main">
      <div class="net-head">
        <div class="net-head-t">{{ cur.label }}</div>
        <div class="net-head-s">{{ cur.hint }}</div>
      </div>
      <component :is="panel" />
    </div>
  </div>
</template>

<style scoped>
.net-shell {
  display: flex;
  gap: 18px;
  align-items: flex-start;
}
.net-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.net-head { display: flex; align-items: baseline; gap: 10px; flex-wrap: wrap; }
.net-head-t { font-size: 15px; font-weight: 700; color: var(--text); }
.net-head-s { font-size: 12.5px; color: var(--text-faint); }
</style>
