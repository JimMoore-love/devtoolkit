/**
 * 静态自检（五类在 vite build 里都不会报错、只在运行时出问题的错误）：
 *
 * 1) 模板引用了未定义标识符 —— 表现是整个页面白屏（曾因漏 import onUnmounted 踩过）
 * 2) `<Icon name="xxx">` 用了不存在于 Icon.vue 的图标名 —— 表现是渲染出一个 info 兜底图标，
 *    错得静默，肉眼很难发现
 * 3) api.js 里 invoke 了后端没有注册的命令 —— 表现是点了按钮毫无反应（invoke 直接 reject）
 * 4) 前后端各写一份的常量漂移 —— 表现是"用户点了才被拒"
 * 5) `<script setup>` 里用了未导入的 Vue API —— 表现是整片内容区空白、连空态都不显示
 *    （编译器把未导入的标识符当全局变量放过，vite build 与第 1) 类检查都发现不了）
 *
 * 用法：node scripts/check-template-refs.mjs
 */
import fs from 'node:fs'
import path from 'node:path'
import { parse, compileScript } from '@vue/compiler-sfc'

const SRC = path.resolve(process.cwd(), 'src')
const ROOT = process.cwd()

function walk(dir) {
  const out = []
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name)
    if (e.isDirectory()) out.push(...walk(p))
    else if (e.name.endsWith('.vue')) out.push(p)
  }
  return out
}

// 这些是模板里合法的全局对象 / Vue 内置属性，不算未定义
const ALLOW = new Set([
  'Math', 'JSON', 'Date', 'Number', 'String', 'Boolean', 'Object', 'Array', 'console',
  '$slots', '$attrs', '$props', '$emit', '$refs', '$el', '$parent', '$root',
])

let failed = 0
const files = walk(SRC)

// ---------------- 1. 模板未定义引用 ----------------

console.log('—— 模板引用 ——')
for (const file of files) {
  const source = fs.readFileSync(file, 'utf-8')
  const { descriptor, errors } = parse(source, { filename: file })
  if (errors.length) {
    console.log(`✖ ${path.relative(SRC, file)} 解析失败: ${errors[0].message}`)
    failed++
    continue
  }
  if (!descriptor.scriptSetup) continue

  let compiled
  try {
    compiled = compileScript(descriptor, { id: 'check', inlineTemplate: true })
  } catch (e) {
    console.log(`✖ ${path.relative(SRC, file)} 编译失败: ${e.message}`)
    failed++
    continue
  }

  const missing = new Set()
  for (const m of compiled.content.matchAll(/_ctx\.([A-Za-z_$][\w$]*)/g)) {
    if (!ALLOW.has(m[1])) missing.add(m[1])
  }

  if (missing.size) {
    console.log(`✖ ${path.relative(SRC, file)} → 模板引用了未定义的: ${[...missing].join(', ')}`)
    failed++
  } else {
    console.log(`✔ ${path.relative(SRC, file)}`)
  }
}

// ---------------- 2. Icon 名称存在性 ----------------

console.log('\n—— 图标名称 ——')
const iconSrc = fs.readFileSync(path.join(SRC, 'components/Icon.vue'), 'utf-8')
const pathsBlock = iconSrc.slice(
  iconSrc.indexOf('const paths = {'),
  iconSrc.indexOf('}', iconSrc.indexOf('const paths = {'))
)
const knownIcons = new Set(
  [...pathsBlock.matchAll(/^\s*([\w$]+)\s*:/gm)].map((m) => m[1])
)
if (!knownIcons.size) {
  console.log('✖ 未能从 Icon.vue 解析出任何图标名，正则可能已失效')
  failed++
} else {
  const used = new Map() // iconName -> [文件]
  for (const file of files) {
    const source = fs.readFileSync(file, 'utf-8')
    // 只看 <Icon ...> 标签内部的 name 属性，避免误伤 props 定义里的 name
    for (const tag of source.matchAll(/<Icon\b[^>]*>/g)) {
      const t = tag[0]
      // 静态写法 name="play"
      const stat = /(?:^|\s)name="([\w-]+)"/.exec(t)
      if (stat) {
        if (!used.has(stat[1])) used.set(stat[1], [])
        used.get(stat[1]).push(path.relative(SRC, file))
      }
      // 动态写法 :name="cond ? 'info' : 'bolt'" —— 把里面的字面量都取出来校验。
      // 先剔掉比较运算里的字面量（t.type === 'err'），那些是判断值不是图标名。
      const dyn = /:name="([^"]*)"/.exec(t)
      if (dyn) {
        const expr = dyn[1]
          .replace(/['"][\w-]*['"]\s*[!=]==?/g, ' ')
          .replace(/[!=]==?\s*['"][\w-]*['"]/g, ' ')
        for (const lit of expr.matchAll(/'([\w-]+)'|"([\w-]+)"/g)) {
          const n = lit[1] || lit[2]
          if (!used.has(n)) used.set(n, [])
          used.get(n).push(`${path.relative(SRC, file)}（动态）`)
        }
      }
    }
  }
  // 数据驱动的图标声明：`icon: 'xxx'` —— 侧边导航表（App.vue）与网络工具目录
  // （networkTools.js）都是这种写法。不扫进来的话"未被使用"列表会被误报淹没，
  // 真正常年没用的图标就看不见了
  const dataFiles = [
    ...files,
    ...fs
      .readdirSync(SRC)
      .filter((x) => x.endsWith('.js'))
      .map((x) => path.join(SRC, x)),
  ]
  for (const file of dataFiles) {
    const source = fs.readFileSync(file, 'utf-8')
    for (const m of source.matchAll(/\bicon:\s*'([\w-]+)'/g)) {
      if (!used.has(m[1])) used.set(m[1], [])
      used.get(m[1]).push(path.relative(SRC, file))
    }
  }
  const bad = [...used.entries()].filter(([n]) => !knownIcons.has(n))
  if (bad.length) {
    for (const [n, where] of bad) {
      console.log(`✖ 图标「${n}」不在 Icon.vue 中（用于 ${[...new Set(where)].join(', ')}），会渲染成兜底图标`)
      failed++
    }
  } else {
    console.log(`✔ 用到的 ${used.size} 个图标名都在 Icon.vue 中（共定义 ${knownIcons.size} 个）`)
  }
  const unused = [...knownIcons].filter((n) => !used.has(n))
  if (unused.length) console.log(`  · 未被使用: ${unused.join(', ')}`)
}

// ---------------- 3. 前端 invoke ↔ 后端注册命令 ----------------

console.log('\n—— 前后端命令对齐 ——')
const apiSrc = fs.readFileSync(path.join(SRC, 'api.js'), 'utf-8')
const invoked = [...apiSrc.matchAll(/invoke\(\s*'([\w_]+)'/g)].map((m) => m[1])

const mainRs = fs.readFileSync(path.join(ROOT, 'src-tauri/src/main.rs'), 'utf-8')
const handlerBlock = mainRs.slice(
  mainRs.indexOf('tauri::generate_handler!['),
  mainRs.indexOf('])', mainRs.indexOf('tauri::generate_handler!['))
)
const registered = new Set(
  handlerBlock
    .split('\n')
    .map((l) => l.replace(/\/\/.*$/, '').trim().replace(/,$/, ''))
    .filter((l) => /^[\w_]+$/.test(l) && l !== 'tauri::generate_handler![')
)

const ghostCalls = invoked.filter((c) => !registered.has(c))
if (ghostCalls.length) {
  for (const c of ghostCalls) {
    console.log(`✖ api.js 调用了未注册的后端命令「${c}」（点击后只会静默失败）`)
    failed++
  }
} else {
  console.log(`✔ api.js 的 ${invoked.length} 个 invoke 目标都已注册（后端共 ${registered.size} 个命令）`)
}

// 反向：注册了但前端与控制口都没用到的，多半是重构后的残留。
// 只提示不算失败——有些命令可能刻意留给后续使用。
const controlRs = fs.readFileSync(path.join(ROOT, 'src-tauri/src/control.rs'), 'utf-8')
const orphan = [...registered].filter(
  (c) => !invoked.includes(c) && !new RegExp(`\\b${c}\\b`).test(controlRs)
)
if (orphan.length) console.log(`  · 未在 api.js / control.rs 中出现的命令: ${orphan.join(', ')}`)

// ---------------- 4. 前后端重复常量对齐 ----------------
//
// 这类常量两边各写一份，改一边忘一边不会报错，只会在某天"用户点了按钮却被拒"时才暴露。
// 已经在源码注释里互相引用，这里再机械地对一次。

console.log('\n—— 前后端常量对齐 ——')

/** 从 rust 源码里抠出 `const NAME: ... = &[0, 4];` 这类数组字面量 */
function rustArray(rust, name) {
  const m = new RegExp(`const ${name}[^=]*=\\s*&?\\[([^\\]]*)\\]`).exec(rust)
  if (!m) return null
  return m[1]
    .split(',')
    .map((s) => s.replace(/[_\s]/g, ''))
    .filter(Boolean)
}

/** 从 js 源码里抠出 `export const NAME = [0, 4]` */
function jsArray(js, name) {
  const m = new RegExp(`export const ${name}\\s*=\\s*\\[([^\\]]*)\\]`).exec(js)
  if (!m) return null
  return m[1]
    .split(',')
    .map((s) => s.replace(/[_\s]/g, ''))
    .filter(Boolean)
}

function compareConst(label, rustName, jsName) {
  const a = rustArray(mainRs, rustName)
  const b = jsArray(apiSrc, jsName)
  if (!a || !b) {
    console.log(`✖ 无法定位常量 ${rustName} / ${jsName}（改名了？请同步更新本脚本）`)
    failed++
    return
  }
  const same = a.length === b.length && a.every((v, i) => v === b[i])
  if (same) {
    console.log(`✔ ${label} 前后端一致  — [${a.join(', ')}]`)
  } else {
    console.log(`✖ ${label} 前后端不一致：后端 [${a.join(', ')}] vs 前端 [${b.join(', ')}]`)
    console.log(`  改了一边忘了另一边，用户会在"点了才被拒"的时候才发现`)
    failed++
  }
}

compareConst('受保护 PID 名单', 'PROTECTED_PIDS', 'PROTECTED_PIDS')

// ---------------- 5. <script setup> 里漏 import 的 Vue API ----------------
//
// 这是最阴的一类：编译器把未导入的标识符当全局变量放过，`vite build` 成功，
// 模板检查也通过（它只看 `_ctx.`），只在运行时抛 `ReferenceError: xxx is not defined`。
// 若发生在 setup 顶层（例如裸写 watch(...)），整个组件直接渲染失败，
// 表现就是"这一页整片空白，连空态提示都没有"，而侧边栏与顶栏正常。
// 本项目真实踩过：ProjectsView 用了 watch 但只导入了 computed/ref/onMounted/onUnmounted。

console.log('\n—— Vue API 导入 ——')

const VUE_APIS = [
  'ref', 'shallowRef', 'reactive', 'shallowReactive', 'readonly', 'computed', 'watch',
  'watchEffect', 'watchPostEffect', 'watchSyncEffect', 'nextTick', 'toRef', 'toRefs',
  'onMounted', 'onUnmounted', 'onBeforeMount', 'onBeforeUnmount', 'onActivated',
  'onDeactivated', 'provide', 'inject', 'markRaw', 'h', 'useTemplateRef',
]

/** 剥掉注释与字符串字面量，避免把注释/文案里的 `watch(` 当成调用 */
function stripNonCode(s) {
  return s
    .replace(/\/\*[\s\S]*?\*\//g, ' ')
    .replace(/\/\/[^\n]*/g, ' ')
    .replace(/'(?:[^'\\]|\\.)*'/g, "''")
    .replace(/"(?:[^"\\]|\\.)*"/g, '""')
    .replace(/`(?:[^`\\]|\\.)*`/g, '``')
}

let apiIssues = 0
for (const file of files) {
  const source = fs.readFileSync(file, 'utf-8')
  const { descriptor } = parse(source, { filename: file })
  if (!descriptor.scriptSetup) continue

  let compiled
  try {
    compiled = compileScript(descriptor, { id: 'check', inlineTemplate: true })
  } catch {
    continue // 编译失败已在第 1 段报过
  }
  const bindings = compiled.bindings || {}
  const code = stripNonCode(descriptor.scriptSetup.content)

  const missing = []
  for (const api of VUE_APIS) {
    // 作为调用出现（排除 obj.watch( 这种成员访问），且不在 setup 绑定里
    const re = new RegExp(`(?<![\\w$.])${api}\\s*\\(`, 'g')
    if (re.test(code) && !(api in bindings)) missing.push(api)
  }

  if (missing.length) {
    console.log(`✖ ${path.relative(SRC, file)} → 用了未导入的 Vue API: ${missing.join(', ')}`)
    console.log(`   运行时会抛 ReferenceError；若在 setup 顶层，整页会渲染成空白`)
    apiIssues++
    failed++
  }
}
if (!apiIssues) console.log(`✔ ${files.length} 个组件用到的 Vue API 都已正确导入`)

console.log('')
if (failed) {
  console.log(`发现 ${failed} 处问题，均属于编译期不报错、运行期才暴露的类型。`)
  process.exit(1)
}
console.log(`全部 ${files.length} 个组件通过检查。`)
