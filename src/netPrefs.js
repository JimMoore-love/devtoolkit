/**
 * 常用诊断目标与输入记忆。
 *
 * 目标地址是最常重复输入的东西（每次诊断都要打一遍 223.5.5.5 或域名），
 * 因此提供一键填充的常用项，并把上次输入记在本地，下次打开即回填。
 * 全部存 localStorage，不出本机。
 */
const PREFIX = 'devtoolkit.net.'

/** 常用诊断目标 */
export const COMMON_TARGETS = [
  { label: '阿里 DNS', value: '223.5.5.5' },
  { label: '114 DNS', value: '114.114.114.114' },
  { label: '腾讯 DNS', value: '119.29.29.29' },
  { label: '本机', value: '127.0.0.1' },
  { label: '百度', value: 'www.baidu.com' },
  { label: '国际出口', value: '8.8.8.8' },
]

/** 读取上次输入；隐私模式等场景下 localStorage 不可用，此时静默返回默认值 */
export function loadPref(key, fallback = '') {
  try {
    const v = localStorage.getItem(PREFIX + key)
    return v === null || v === '' ? fallback : v
  } catch {
    return fallback
  }
}

export function savePref(key, value) {
  try {
    localStorage.setItem(PREFIX + key, String(value))
  } catch {
    // 存不了不影响使用
  }
}
