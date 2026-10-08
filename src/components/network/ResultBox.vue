<script setup>
/**
 * 结果容器：把"执行状态 + 摘要 + 操作 + 正文"统一成一套。
 *
 * 此前每个工具面板各写一套结果展示，有的只弹 toast（两千字输出直接没了）、
 * 有的把原始文本直接贴进表格，且都没有复制/导出——诊断完贴不到工单里。
 * 现在结果一律经过这里：成功有标题与摘要、失败有原因、任何时候都能
 * 复制 / 导出 / 展开原文。
 */
import { computed, ref } from 'vue'
import Icon from '../Icon.vue'
import { toast, cleanErr } from '../../store'
import { api } from '../../api'

const props = defineProps({
  /** idle（未开始）| running（执行中）| ok（成功）| err（失败） */
  state: { type: String, default: 'idle' },
  /** 结果标题，通常是"目标 + 关键参数" */
  title: { type: String, default: '' },
  /** 一行摘要，如 "平均 1.5ms · 丢包 0%" */
  summary: { type: String, default: '' },
  /** 失败原因（state === 'err' 时展示） */
  error: { type: String, default: '' },
  /** 原始输出，用于"原文"折叠与兜底复制 */
  raw: { type: String, default: '' },
  /** 复制/导出优先生成的纯文本报告；留空则退回 raw */
  report: { type: String, default: '' },
  /** 耗时（ms），为 0 时不展示 */
  elapsed: { type: Number, default: 0 },
  /** 初始空态文案 */
  empty: { type: String, default: '填好参数后开始' },
  /** 导出文件名前缀（后端会再清洗一次，只保留安全字符） */
  name: { type: String, default: 'network-report' },
})

const showRaw = ref(false)
const exporting = ref(false)

const body = computed(() => props.report || props.raw)

/** 复制到剪贴板。clipboard API 在 WebView 里可能被拒，降级到 textarea 方案 */
async function copy() {
  if (!body.value) return
  try {
    await navigator.clipboard.writeText(body.value)
    toast('已复制到剪贴板')
    return
  } catch {
    // 落到下面的降级路径
  }
  const ta = document.createElement('textarea')
  ta.value = body.value
  ta.style.cssText = 'position:fixed;top:-9999px;opacity:0'
  document.body.appendChild(ta)
  ta.select()
  let ok = false
  try {
    ok = document.execCommand('copy')
  } catch {
    ok = false
  }
  document.body.removeChild(ta)
  toast(ok ? '已复制到剪贴板' : '复制失败，请展开原文手动选择', ok ? 'ok' : 'err')
}

/** 导出为 txt。WebView 里 a[download] 不会触发保存，因此交给后端写盘 */
async function exportTxt() {
  if (!body.value) return
  exporting.value = true
  try {
    const path = await api.netExportReport(props.name, body.value)
    toast(`已导出到 ${path}`)
  } catch (e) {
    toast('导出失败: ' + cleanErr(e), 'err')
  } finally {
    exporting.value = false
  }
}
</script>

<template>
  <div class="net-result" :class="'is-' + state">
    <div v-if="state === 'idle'" class="nr-empty">
      <Icon name="info" :size="15" />
      <span>{{ empty }}</span>
    </div>

    <div v-else-if="state === 'running'" class="nr-running">
      <span class="nr-spin"></span>
      <span>{{ title || '正在执行…' }}</span>
    </div>

    <div v-else-if="state === 'err'" class="nr-err">
      <Icon name="alert" :size="16" />
      <div class="nr-err-body">
        <div class="nr-err-t">{{ title || '执行失败' }}</div>
        <div class="nr-err-m">{{ error || '未知错误' }}</div>
      </div>
    </div>

    <template v-else>
      <div class="nr-head">
        <div class="nr-head-l">
          <span class="nr-dot"></span>
          <span class="nr-title">{{ title }}</span>
          <span v-if="elapsed" class="nr-elapsed">
            <Icon name="clock" :size="11" />{{ elapsed }} ms
          </span>
        </div>
        <div class="nr-acts">
          <button class="btn sm ghost" title="复制为纯文本" @click="copy">
            <Icon name="copy" :size="12" /> 复制
          </button>
          <button class="btn sm ghost" :disabled="exporting" title="导出到下载目录" @click="exportTxt">
            <Icon name="download" :size="12" /> {{ exporting ? '导出中…' : '导出' }}
          </button>
          <button v-if="raw" class="btn sm ghost" @click="showRaw = !showRaw">
            <Icon name="chevron" :size="12" :class="{ rot90: showRaw }" /> 原文
          </button>
        </div>
      </div>

      <div v-if="summary" class="nr-summary">{{ summary }}</div>

      <div class="nr-body"><slot /></div>

      <pre v-if="showRaw && raw" class="nr-src">{{ raw }}</pre>
    </template>
  </div>
</template>

<style scoped>
.net-result {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--panel);
  padding: 14px 16px;
}
.net-result.is-ok { border-color: #24384f; }

.nr-empty,
.nr-running {
  display: flex;
  align-items: center;
  gap: 9px;
  color: var(--text-faint);
  font-size: 13px;
  padding: 10px 2px;
}
.nr-running { color: var(--text-dim); }

.nr-spin {
  width: 13px;
  height: 13px;
  border: 2px solid var(--border-2);
  border-top-color: var(--cyan);
  border-radius: 50%;
  animation: nr-rot 0.75s linear infinite;
  flex-shrink: 0;
}
@keyframes nr-rot { to { transform: rotate(360deg); } }

.nr-err {
  display: flex;
  gap: 10px;
  color: var(--danger);
  padding: 4px 2px;
}
.nr-err-t { font-weight: 600; font-size: 13px; }
.nr-err-m {
  font-size: 12.5px;
  color: #fca5a5;
  margin-top: 4px;
  white-space: pre-wrap;
  word-break: break-word;
  user-select: text;
}

.nr-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
  margin-bottom: 10px;
}
.nr-head-l { display: flex; align-items: center; gap: 8px; min-width: 0; }
.nr-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--ok);
  flex-shrink: 0;
}
.is-err .nr-dot { background: var(--danger); }
.nr-title {
  font-weight: 600;
  font-size: 13px;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.nr-elapsed {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  color: var(--text-faint);
  font-family: var(--mono);
}
.nr-acts { display: flex; gap: 6px; flex-shrink: 0; }
.nr-acts :deep(svg).rot90 { transform: rotate(90deg); }

.nr-summary {
  font-size: 12.5px;
  color: var(--text-dim);
  padding: 7px 10px;
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.03);
  margin-bottom: 12px;
  user-select: text;
}

.nr-body { min-width: 0; }

.nr-src {
  margin-top: 12px;
  background: rgba(0, 0, 0, 0.32);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 11px 13px;
  font-family: var(--mono);
  font-size: 11.5px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 300px;
  overflow: auto;
  color: #a9bccf;
  user-select: text;
}
</style>
