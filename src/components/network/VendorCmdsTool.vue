<script setup>
/**
 * 命令速查：常见厂商设备的排障命令。
 *
 * 此前只是一大段只读文本——看到有用的命令只能手动选中复制。
 * 现在每条命令独立成行并可单独复制，整组也可一次复制走。
 */
import { computed, ref } from 'vue'
import Icon from '../Icon.vue'
import { toast } from '../../store'
import { VENDOR_CMDS } from '../../networkData'

const vendor = ref('huawei')
const cur = computed(() => VENDOR_CMDS[vendor.value] || { name: '', groups: [] })

/** 剪贴板 API 在 WebView 里可能被拒，统一走降级方案 */
async function writeText(text) {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    const ta = document.createElement('textarea')
    ta.value = text
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
    return ok
  }
}

async function copyOne(cmd) {
  const ok = await writeText(cmd)
  toast(ok ? '已复制命令' : '复制失败，请手动选中', ok ? 'ok' : 'err')
}

async function copyGroup(g) {
  const ok = await writeText(g.cmds.join('\n'))
  toast(ok ? `已复制「${g.title}」的 ${g.cmds.length} 条命令` : '复制失败，请手动选中', ok ? 'ok' : 'err')
}
</script>

<template>
  <div class="net-tool">
    <div class="net-form">
      <div class="net-field">
        <label>设备厂商</label>
        <select v-model="vendor" class="input">
          <option v-for="(v, k) in VENDOR_CMDS" :key="k" :value="k">{{ v.name }}</option>
        </select>
      </div>
      <span class="net-inline-note">命令仅作速查参考，执行前请确认设备型号与权限</span>
    </div>

    <div v-for="(g, i) in cur.groups" :key="i" class="ref-group">
      <div class="ref-head">
        <span class="ref-title">{{ g.title }}</span>
        <button class="btn sm ghost" @click="copyGroup(g)">
          <Icon name="copy" :size="12" /> 复制本组
        </button>
      </div>
      <div class="ref-cmds">
        <div v-for="(c, j) in g.cmds" :key="j" class="ref-cmd">
          <code class="ref-code">{{ c }}</code>
          <button class="ref-copy" title="复制这一条" @click="copyOne(c)">
            <Icon name="copy" :size="11" />
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.ref-group {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--panel);
  padding: 13px 15px;
}
.ref-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 10px;
}
.ref-title { font-weight: 600; font-size: 13px; color: var(--cyan); }
.ref-cmds { display: flex; flex-direction: column; gap: 3px; }
.ref-cmd {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 5px 8px;
  border-radius: 6px;
  transition: background 0.12s;
}
.ref-cmd:hover { background: rgba(255, 255, 255, 0.04); }
.ref-code {
  font-family: var(--mono);
  font-size: 12px;
  color: var(--text-dim);
  user-select: text;
  word-break: break-all;
}
.ref-copy {
  border: none;
  background: transparent;
  color: var(--text-faint);
  cursor: pointer;
  padding: 3px;
  border-radius: 5px;
  flex-shrink: 0;
  opacity: 0;
  transition: opacity 0.12s, color 0.12s;
}
.ref-cmd:hover .ref-copy { opacity: 1; }
.ref-copy:hover { color: var(--cyan); background: rgba(34, 211, 238, 0.12); }
</style>
