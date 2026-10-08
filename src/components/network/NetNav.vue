<script setup>
/**
 * 工具导航：按用途分组，替代原来 11 个平铺的横向标签栏。
 *
 * 分组的意义在于"找得到"——诊断、查询、工具三类的使用场景完全不同，
 * 混在一条标签栏里只能靠眼睛扫。
 */
import Icon from '../Icon.vue'
import { NET_GROUPS } from '../../networkTools'

defineProps({
  current: { type: String, required: true },
})
const emit = defineEmits(['pick'])
</script>

<template>
  <nav class="net-nav">
    <div v-for="g in NET_GROUPS" :key="g.key" class="nn-group">
      <div class="nn-label">{{ g.label }}</div>
      <button
        v-for="t in g.tools"
        :key="t.key"
        class="nn-item"
        :class="{ active: t.key === current }"
        :title="t.hint"
        @click="emit('pick', t.key)"
      >
        <Icon :name="t.icon" :size="15" />
        <span class="nn-text">{{ t.label }}</span>
      </button>
    </div>
  </nav>
</template>

<style scoped>
.net-nav {
  width: 156px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  gap: 15px;
  border-right: 1px solid var(--border);
  padding-right: 12px;
}
.nn-group { display: flex; flex-direction: column; gap: 2px; }
.nn-label {
  font-size: 10.5px;
  letter-spacing: 0.08em;
  color: var(--text-faint);
  padding: 0 8px 6px;
}
.nn-item {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  padding: 7px 9px;
  border: 1px solid transparent;
  border-radius: 8px;
  background: transparent;
  color: var(--text-dim);
  font-size: 13px;
  font-family: inherit;
  cursor: pointer;
  text-align: left;
  transition: background 0.14s, color 0.14s, border-color 0.14s;
}
.nn-item:hover { background: rgba(255, 255, 255, 0.045); color: var(--text); }
.nn-item.active {
  background: rgba(34, 211, 238, 0.13);
  border-color: rgba(34, 211, 238, 0.32);
  color: var(--cyan);
  font-weight: 600;
}
.nn-text { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
