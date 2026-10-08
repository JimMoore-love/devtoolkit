import { createApp } from 'vue'
import App from './App.vue'
import './styles.css'

/** 把致命错误显示在页面上，而不是留一个纯白窗口。
 *
 * 白屏是最难排查的故障：用户只能反馈"打不开"，界面上没有任何线索，
 * 开发者也只能靠反复重建去二分定位。把错误直接画出来，一眼就能看出是哪一步炸的。
 * 只在根节点还空着的时候接管 —— 已经渲染出内容的界面绝不能被错误提示盖掉。 */
function showFatal(detail) {
  const el = document.getElementById('app')
  if (!el || el.childElementCount) return
  const text = String(detail || '未知错误')

  const box = document.createElement('div')
  box.style.cssText =
    'padding:26px 28px;font:13px/1.7 system-ui,sans-serif;color:#f87171;' +
    'background:#0a101a;min-height:100%;box-sizing:border-box'

  const title = document.createElement('strong')
  title.textContent = '界面加载失败'

  // 用 textContent 而非 innerHTML：错误信息里可能含 HTML，不能当标记解析
  const pre = document.createElement('pre')
  pre.style.cssText = 'margin:12px 0 0;white-space:pre-wrap;word-break:break-all;font-size:11.5px'
  pre.textContent = text

  box.appendChild(title)
  box.appendChild(pre)
  el.replaceChildren(box)
  document.title = '界面加载失败 · DevToolkit'
}

window.addEventListener('error', (e) => showFatal(e.error?.stack || e.message))
window.addEventListener('unhandledrejection', (e) =>
  showFatal('未处理的 Promise 拒绝：\n' + (e.reason?.stack || e.reason))
)

const app = createApp(App)
app.config.errorHandler = (err, _instance, info) =>
  showFatal(`组件渲染异常（${info}）：\n${err?.stack || err}`)

app.mount('#app')
