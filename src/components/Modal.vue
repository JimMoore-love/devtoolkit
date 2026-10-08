<script setup>
import { onUnmounted, useSlots, watch } from 'vue'

const props = defineProps({ open: Boolean, title: String, width: { type: String, default: '520px' } })
const emit = defineEmits(['close'])
const slots = useSlots()

function onKey(e) {
  if (e.key === 'Escape') emit('close')
}

// 弹窗打开期间：拦 ESC 关闭 + 锁住背景滚动。
// 不锁滚动的话，滚轮会穿透到弹窗后面的长列表上，位置莫名其妙地跑掉。
watch(
  () => props.open,
  (open) => {
    if (open) {
      window.addEventListener('keydown', onKey)
      document.body.style.overflow = 'hidden'
    } else {
      window.removeEventListener('keydown', onKey)
      document.body.style.overflow = ''
    }
  }
)

onUnmounted(() => {
  window.removeEventListener('keydown', onKey)
  document.body.style.overflow = ''
})
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="modal-mask" @click.self="emit('close')">
      <div class="modal" :style="{ width }">
        <div class="modal-head">
          <h3>{{ title }}</h3>
          <button class="icon-btn" @click="emit('close')">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <path d="M6 6l12 12M18 6L6 18" />
            </svg>
          </button>
        </div>
        <div class="modal-body"><slot /></div>
        <div class="modal-foot" v-if="slots.foot"><slot name="foot" /></div>
      </div>
    </div>
  </Teleport>
</template>
