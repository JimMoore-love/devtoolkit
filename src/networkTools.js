/**
 * 网络工具箱的工具目录。
 *
 * 之前 11 个工具是一条平铺的横向标签栏，类别混杂（诊断 / 查询 / 计算 / 静态文档），
 * 找东西只能靠眼睛扫。这里按用途分成三组，导航据此渲染分组侧栏。
 *
 * 每个工具声明：
 *   key    路由键（NetworkView 用它选面板组件）
 *   label  显示名
 *   icon   Icon.vue 里的图标名（此前用的是 emoji，跨系统渲染不一致且与应用其余部分割裂）
 *   hint   一句话说明，鼠标悬停与面板副标题都会用到
 */
export const NET_GROUPS = [
  {
    key: 'diag',
    label: '诊断',
    tools: [
      { key: 'ping', label: 'Ping', icon: 'ping', hint: '连通性、延迟波动与丢包' },
      { key: 'trace', label: '路由追踪', icon: 'route', hint: '逐跳时延，定位瓶颈在哪一跳' },
      { key: 'scan', label: '端口扫描', icon: 'scan', hint: '探测目标主机开放的端口' },
      { key: 'dns', label: 'DNS 查询', icon: 'globe', hint: 'A / AAAA / CNAME / MX / NS / TXT' },
      { key: 'speed', label: '测速', icon: 'gauge', hint: '下载带宽 · 延迟 · 抖动' },
    ],
  },
  {
    key: 'lookup',
    label: '查询',
    tools: [
      { key: 'local', label: '本机网络', icon: 'nic', hint: '网卡、IP、网关、DNS 与出口 IP' },
      { key: 'arp', label: '主机发现', icon: 'monitor', hint: 'ARP 表中的同网段设备' },
      { key: 'route', label: '路由表', icon: 'table', hint: 'IPv4 路由条目与默认网关' },
      { key: 'mac', label: 'MAC 厂商', icon: 'tag', hint: '按 OUI 前缀判断设备厂商' },
    ],
  },
  {
    key: 'util',
    label: '工具',
    tools: [
      { key: 'wol', label: 'WOL 唤醒', icon: 'power', hint: '向目标网卡发送开机魔术包' },
      { key: 'calc', label: '网络计算', icon: 'calculator', hint: '子网划分、供电、布线、PoE 预算' },
      { key: 'ref', label: '命令速查', icon: 'book', hint: '常见厂商设备的排障命令' },
    ],
  },
]

/** 扁平化的工具列表，按导航顺序排列 */
export const NET_TOOLS = NET_GROUPS.flatMap((g) => g.tools)

/** 默认打开的工具 */
export const DEFAULT_NET_TOOL = 'ping'

/** 按 key 取工具定义 */
export function toolOf(key) {
  return NET_TOOLS.find((t) => t.key === key) || NET_TOOLS[0]
}
