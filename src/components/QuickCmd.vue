<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue'
import Icon from './Icon.vue'
import Modal from './Modal.vue'
import { api, fmtDate } from '../api'
import { store, toast, refreshConfig, cleanErr } from '../store'

const cmd = ref('')
const cwd = ref(localStorage.getItem('dk.cwd') || 'E:\\ai_tools')
const running = ref(false)
const result = ref(null) // { stdout, stderr, code, duration_ms, timed_out, truncated }
/** 记录本次真正执行的那条命令：用户执行后在输入框里继续改字，
 *  提示行不能跟着变——否则显示的是新命令配旧输出，排查时会被带偏。 */
const lastRun = ref(null)
const elapsed = ref(0)
const pendingDanger = ref(null)

/** 明显会破坏系统或数据的命令，执行前拦一下。宁可多问一句，不可误删一次。 */
const DANGER_PATTERNS = [
  { re: /\brm\s+(-[a-z]*\s+)*-[a-z]*[rf]/i, why: '递归/强制删除（rm -rf）' },
  { re: /\bdel\s+\/[a-z]*[sq]/i, why: '批量删除文件（del /S /Q）' },
  { re: /\brd\s+\/s/i, why: '递归删除目录（rd /S）' },
  { re: /\bformat\s+[a-z]:/i, why: '格式化磁盘' },
  { re: /\bmkfs\b/i, why: '创建文件系统（会抹掉分区数据）' },
  { re: /\bshutdown\b/i, why: '关机 / 重启' },
  { re: /\bdiskpart\b/i, why: '磁盘分区操作' },
  { re: /\breg\s+delete\b/i, why: '删除注册表项' },
  { re: /\btaskkill\b.*\/f/i, why: '强制结束进程' },
  { re: /drop\s+(table|database)/i, why: '删除数据库对象' },
  { re: /\bgit\s+push\b.*(--force|-f)\b/i, why: '强制推送（可能覆盖远端历史）' },
]

function dangerOf(text) {
  const hit = DANGER_PATTERNS.find((d) => d.re.test(text))
  return hit ? hit.why : ''
}

let ticker = null
function startTicker() {
  stopTicker()
  elapsed.value = 0
  ticker = setInterval(() => {
    elapsed.value = Math.floor((Date.now() - startedAt) / 1000)
  }, 1000)
}
function stopTicker() {
  if (ticker) clearInterval(ticker)
  ticker = null
}
let startedAt = 0
onUnmounted(stopTicker)

onMounted(() => {
  refreshConfig()
})

function requestRun() {
  const text = cmd.value.trim()
  if (!text || running.value) return
  const why = dangerOf(text)
  if (why) {
    pendingDanger.value = { cmd: text, why }
    return
  }
  run()
}

async function run() {
  const text = cmd.value.trim()
  if (!text || running.value) return
  const workdir = cwd.value.trim()
  running.value = true
  result.value = null
  lastRun.value = { cmd: text, cwd: workdir }
  startedAt = Date.now()
  startTicker()
  try {
    localStorage.setItem('dk.cwd', workdir)
    result.value = await api.execCommand(text, workdir)
  } catch (e) {
    // 构造命令失败、工作目录不存在这类问题在这里；
    // 超时则不走这里——后端会正常返回并带 timed_out 标记
    toast('执行失败: ' + cleanErr(e), 'err')
  } finally {
    stopTicker()
    // 耗时就以后端返回的为准（前端计时含 IPC 往返，会偏大）
    if (result.value) elapsed.value = Math.round(result.value.duration_ms / 1000)
    running.value = false
    refreshConfig()
  }
}

function confirmDanger() {
  const c = pendingDanger.value
  pendingDanger.value = null
  if (c) run()
}

function rerun(h) {
  if (!h || !h.command) return
  cmd.value = h.command
  // 旧版本的历史记录没有 cwd 字段，别把输入框写成 undefined
  if (h.cwd) cwd.value = h.cwd
  requestRun()
}

const badgeText = computed(() => {
  const r = result.value
  if (!r) return ''
  return r.timed_out ? '超时已终止' : `exit ${r.code}`
})
</script>

<template>
  <div>
    <div class="card" style="margin-bottom: 14px">
      <div class="field" style="margin-bottom: 10px">
        <label>工作目录</label>
        <input class="input mono" v-model="cwd" placeholder="E:\ai_tools" spellcheck="false" />
      </div>
      <div class="field" style="margin-bottom: 0">
        <label>命令</label>
        <div style="display: flex; gap: 10px">
          <input
            class="input mono"
            v-model="cmd"
            placeholder="例如：netstat -ano | findstr 3000"
            spellcheck="false"
            @keydown.enter="requestRun"
          />
          <button class="btn primary" style="min-width: 96px" :disabled="running || !cmd.trim()" @click="requestRun">
            <Icon v-if="!running" name="play" :size="13" />
            <Icon v-else name="refresh" :size="13" class="spin" />
            {{ running ? '执行中' : '执行' }}
          </button>
        </div>
        <div class="field-hint">
          命令在前台执行并等待返回；长时间不返回会被强制结束整棵进程树，已产生的输出仍会保留。
          需要常驻的服务请用「项目管理」托管。
        </div>
      </div>
      <div v-if="running" class="run-hint">
        <Icon name="refresh" :size="12" class="spin" /> 已等待 {{ elapsed }} 秒…
      </div>
    </div>

    <div v-if="result" class="card" style="margin-bottom: 14px">
      <div class="card-title">
        <Icon name="terminal" :size="14" /> 执行结果
        <span style="margin-left: auto; display: flex; gap: 8px; align-items: center">
          <span class="code-badge" :class="result.timed_out ? 'warn' : result.code === 0 ? 'ok' : 'bad'">
            {{ badgeText }}
          </span>
          <span class="tag gray">{{ result.duration_ms }} ms</span>
        </span>
      </div>

      <div v-if="result.timed_out" class="warn-banner">
        <Icon name="bolt" :size="13" />
        <span>
          命令超过执行时限，已被强制结束（连同子进程树）。上面的输出是超时前已经产生的部分。
          如果这是一条需要长期运行的命令，请改用「项目管理」创建常驻任务。
        </span>
      </div>
      <div v-if="result.truncated" class="warn-banner">
        <Icon name="bolt" :size="13" />
        <span>输出过长已被截断，这里只保留了前面一部分。完整内容请把输出重定向到文件后再查看。</span>
      </div>

      <div class="term-panel">
        <div class="prompt">PS {{ lastRun?.cwd }}&gt; {{ lastRun?.cmd }}</div>
        <div style="height: 8px"></div>
        <div v-if="result.stdout" class="out">{{ result.stdout }}</div>
        <div v-if="result.stderr" class="err">{{ result.stderr }}</div>
        <div v-if="!result.stdout && !result.stderr" style="color: var(--text-faint)">（无输出）</div>
      </div>
    </div>

    <div class="card">
      <div class="card-title">
        <Icon name="doc" :size="14" /> 历史命令
        <span class="more">点击任意记录重新执行（保留最近 50 条）</span>
      </div>
      <div v-if="store.config.history.length">
        <div v-for="(h, i) in store.config.history" :key="i" class="hist-item" @click="rerun(h)">
          <span class="cmd">$ {{ h.command }}</span>
          <span class="meta">{{ fmtDate(h.ts) }} · {{ h.duration_ms }}ms</span>
          <span class="code-badge" :class="h.code === 0 ? 'ok' : 'bad'">{{ h.code }}</span>
        </div>
      </div>
      <div v-else class="empty">
        <p>暂无历史</p>
        <p class="hint">执行的命令会自动留存在这里，重启应用后依然可见</p>
      </div>
    </div>

    <Modal :open="!!pendingDanger" @close="pendingDanger = null" title="这条命令有风险" width="520">
      <div v-if="pendingDanger">
        <div style="font-size: 13.5px; line-height: 1.8">
          即将执行的是<b style="color: var(--danger)">{{ pendingDanger.why }}</b>。
        </div>
        <div class="term-panel" style="margin-top: 10px">
          <div class="prompt">PS {{ cwd }}&gt; {{ pendingDanger.cmd }}</div>
        </div>
        <div style="color: var(--text-faint); font-size: 12px; margin-top: 10px">
          这类操作可能不可逆。请确认命令拼写与目标路径无误后再继续。
        </div>
      </div>
      <template #foot>
        <button class="btn" @click="pendingDanger = null">取消</button>
        <button class="btn danger" @click="confirmDanger">
          <Icon name="play" :size="13" /> 仍然执行
        </button>
      </template>
    </Modal>
  </div>
</template>
