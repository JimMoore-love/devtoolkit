import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

export const api = {
  sysInfo: () => invoke('sys_info'),
  listPorts: () => invoke('list_ports'),
  // 全量端口 + 项目运行态（端口页与项目管理页共用同一数据源）
  scanPorts: (projects) => invoke('scan_ports', { projects }),
  checkPorts: (ports) => invoke('check_ports', { ports }),
  listProcesses: () => invoke('list_processes'),
  processDetail: (pid) => invoke('process_detail', { pid }),
  killPid: (pid) => invoke('kill_pid', { pid }),
  getConfig: () => invoke('get_config'),
  saveProjects: (projects) => invoke('save_projects', { projects }),
  // 切换端口保护状态，返回切换后的完整保护名单
  setPortProtected: (port, enabled) => invoke('set_port_protected', { port, protected: enabled }),
  // 保存前的项目体检：errors 阻断保存，warnings 只提示
  checkProject: (project) => invoke('check_project', { project }),
  startTask: (project) => invoke('start_task', { project }),
  stopTask: (taskId) => invoke('stop_task', { taskId }),
  // 结束占用项目声明端口的外部进程并以托管方式重启；动作全在后端，见 store.takeoverProject
  takeoverTask: (project) => invoke('takeover_task', { project }),
  listTasks: () => invoke('list_tasks'),
  execCommand: (cmd, cwd) => invoke('exec_command', { cmd, cwd }),
  openInExplorer: (path) => invoke('open_in_explorer', { path }),
  // 网络工具箱（源自 network-toolbox）
  netPing: (host, count, size) => invoke('net_ping', { host, count, size }),
  netTraceroute: (host) => invoke('net_traceroute', { host }),
  netPortScan: (host, ports) => invoke('net_port_scan', { host, ports }),
  netArpHosts: () => invoke('net_arp_hosts'),
  netRouteTable: () => invoke('net_route_table'),
  netWol: (mac) => invoke('net_wol', { mac }),
  netSpeedtest: (source) => invoke('net_speedtest', { source }),
  netDnsLookup: (host, rtype) => invoke('net_dns_lookup', { host, rtype }),
  netLocalInfo: () => invoke('net_local_info'),
  netPublicIp: () => invoke('net_public_ip'),
  netExportReport: (name, content) => invoke('net_export_report', { name, content }),
  // MCP 控制口（供 AI 客户端直连）
  mcpInfo: () => invoke('mcp_info'),
  // ---- MCP 功能管理 ----
  mcpClientsDetect: () => invoke('mcp_clients_detect'),
  mcpClientWrite: (id) => invoke('mcp_client_write', { id }),
  mcpSelftest: () => invoke('mcp_selftest'),
  mcpAuditList: (limit) => invoke('mcp_audit_list', { limit }),
  mcpAuditClear: () => invoke('mcp_audit_clear'),
  mcpControlSet: (enabled) => invoke('mcp_control_set', { enabled }),
  mcpTokenReset: () => invoke('mcp_token_reset'),
  mcpPortSet: (port) => invoke('mcp_port_set', { port }),
}

export const onEvent = (name, handler) => listen(name, (e) => handler(e.payload))

/**
 * 端口声明解析：实现已收敛到 `src/portSpec.js`（前端唯一一份）。
 * 这里 re-export 是为了保持 `api.js` 的对外接口不变（ProjectsView / PortsView 都从这里 import）。
 *
 * 注意：原先这里导出的名字是 `PORT_RANGE_SPAN`，现改名为 `MAX_PORT_RANGE_SPAN`
 * 与后端常量同名（该旧名字在仓库里没有任何调用点）。
 */
export { parsePortSpec, MAX_PORT_RANGE_SPAN } from './portSpec.js'

/**
 * 系统关键进程：0 是 System Idle Process，4 是内核态的 System。
 * 后端 kill_pid 会直接拒绝，前端提前把按钮置灰，省得用户点了才吃一个错误。
 * 改动时需与 src-tauri/src/main.rs 的 PROTECTED_PIDS 同步（自检脚本会比对）。
 */
export const PROTECTED_PIDS = [0, 4]

/** 结束进程前的兜底判断，与后端同一口径 */
export function isProtectedPid(pid) {
  return PROTECTED_PIDS.includes(Number(pid))
}

export function fmtBytes(n) {
  if (!n && n !== 0) return '-'
  if (n >= 1024 ** 3) return (n / 1024 ** 3).toFixed(1) + ' GB'
  if (n >= 1024 ** 2) return (n / 1024 ** 2).toFixed(0) + ' MB'
  if (n >= 1024) return (n / 1024).toFixed(0) + ' KB'
  return n + ' B'
}

export function fmtUptime(sec) {
  if (sec < 60) return Math.floor(sec) + ' 秒'
  if (sec < 3600) return Math.floor(sec / 60) + ' 分 ' + Math.floor(sec % 60) + ' 秒'
  if (sec < 86400) return Math.floor(sec / 3600) + ' 时 ' + Math.floor((sec % 3600) / 60) + ' 分'
  return Math.floor(sec / 86400) + ' 天 ' + Math.floor((sec % 86400) / 3600) + ' 时'
}

export function fmtTime(ts) {
  const d = new Date(ts * 1000)
  const p = (x) => String(x).padStart(2, '0')
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
}

export function fmtDate(ts) {
  const d = new Date(ts * 1000)
  const p = (x) => String(x).padStart(2, '0')
  return `${d.getMonth() + 1}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
}
