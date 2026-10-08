// 网络工具箱静态数据：OUI 厂商表 / 运维命令库（源自 network-toolbox）

export const OUI_TABLE = {
  '000C29': 'VMware', '005056': 'VMware', '000569': 'VMware',
  'F8FFC2': 'TP-LINK', '50C7BF': 'TP-LINK', 'A4F773': 'TP-LINK', '5CFBEF': 'TP-LINK',
  'D4CA6D': '锐捷 Ruijie', 'D4C9EF': '锐捷 Ruijie',
  '28D126': 'H3C', '5C9E08': 'H3C',
  'F4F95D': '华为 Huawei', 'A0B87E': '华为 Huawei', '28FAA0': '华为 Huawei',
  'CC34D6': '华为 Huawei', 'B4542F': '华为 Huawei',
  'C4E984': 'Apple', 'A4B199': 'Apple', '3CF011': 'Apple', 'F0C1F1': 'Apple', 'E0C767': 'Apple',
  'DC2B2A': '小米 Xiaomi', '640980': '小米 Xiaomi', 'F0B429': '小米 Xiaomi', '68DFDD': '小米 Xiaomi',
  'D8C7C8': '联想 Lenovo', 'A08869': '联想 Lenovo',
  '38F8B7': 'Dell', 'B8CA3A': 'Dell',
  '18DED7': 'Intel', 'D4EE07': 'Intel', 'C8D7B0': 'Intel',
  'F4F1E1': 'Unifi/Ubiquiti', 'F06E0B': 'Unifi/Ubiquiti',
  'E46D30': '中兴 ZTE', '640F28': '中兴 ZTE',
  '803F5D': '海康威视', 'A49B13': '海康威视', '50E549': '海康威视',
  '3CDA07': '大华 Dahua', '9CA10A': '大华 Dahua', '78019B': '大华 Dahua', '0012D1': '大华 Dahua',
  '00E0C5': '科达 Kedacom',
}

export function lookupVendor(mac) {
  const oui = (mac || '').replace(/[:\-.]/g, '').toUpperCase().slice(0, 6)
  return OUI_TABLE[oui] || '未知厂商'
}

export const VENDOR_CMDS = {
  huawei: {
    name: '华为交换机 VRP',
    groups: [
      { title: '基础信息', cmds: ['display version', 'display device', 'display interface brief', 'display cpu-usage', 'display memory-usage', 'display temperature'] },
      { title: 'VLAN 配置', cmds: ['vlan 10', 'port link-type access', 'port default vlan 10', 'port trunk allow-pass vlan 10 20', 'display vlan'] },
      { title: '故障排查', cmds: ['display logbuffer', 'display trapbuffer', 'display mac-address', 'display arp', 'ping -c 20 192.168.1.1', 'tracert 8.8.8.8'] },
      { title: '端口管理', cmds: ['int g0/0/1', 'shutdown / undo shutdown', 'port link-type trunk', 'display error-down recovery'] },
    ],
  },
  cisco: {
    name: '思科 Cisco IOS',
    groups: [
      { title: '基础信息', cmds: ['show version', 'show inventory', 'show interfaces status', 'show processes cpu', 'show memory statistics'] },
      { title: 'VLAN 配置', cmds: ['vlan 10', 'interface vlan 10', 'switchport mode access', 'switchport access vlan 10', 'show vlan brief'] },
      { title: '故障排查', cmds: ['show log', 'show ip arp', 'show mac address-table', 'ping 192.168.1.1', 'traceroute 8.8.8.8', 'debug ip icmp'] },
    ],
  },
  h3c: {
    name: 'H3C Comware',
    groups: [
      { title: '基础信息', cmds: ['display version', 'display device verbose', 'display interface brief', 'display cpu-usage', 'display memory'] },
      { title: 'VLAN 配置', cmds: ['vlan 10', 'port access vlan 10', 'port trunk permit vlan 10 20', 'display vlan all'] },
      { title: '故障排查', cmds: ['display logbuffer', 'display ip routing-table', 'display arp', 'display mac-address', 'ping -c 20 192.168.1.1'] },
    ],
  },
  ruijie: {
    name: '锐捷 RGOS',
    groups: [
      { title: '基础信息', cmds: ['show version', 'show device', 'show interface status', 'show cpu', 'show memory'] },
      { title: 'VLAN 配置', cmds: ['vlan 10', 'switchport mode access', 'switchport access vlan 10', 'show vlan'] },
      { title: '故障排查', cmds: ['show logging', 'show arp', 'show mac-address-table', 'ping 192.168.1.1', 'traceroute 8.8.8.8'] },
    ],
  },
  windows: {
    name: 'Windows 网络/系统',
    groups: [
      { title: '网络诊断', cmds: ['ipconfig /all', 'ipconfig /release && ipconfig /renew', 'ping -t 223.5.5.5', 'tracert -d 8.8.8.8', 'pathping 8.8.8.8'] },
      { title: '端口与连接', cmds: ['netstat -ano | findstr 8080', 'netstat -an | find "ESTABLISHED"', 'tasklist | findstr <pid>'] },
      { title: '系统诊断', cmds: ['sfc /scannow', 'dism /online /cleanup-image /restorehealth', 'systeminfo', 'msinfo32'] },
    ],
  },
  linux: {
    name: 'Linux 网络/服务器',
    groups: [
      { title: '网络诊断', cmds: ['ip addr show', 'ip route show', 'cat /etc/resolv.conf', 'ping -c 20 223.5.5.5', 'traceroute 8.8.8.8', 'ss -tulnp'] },
      { title: '端口与服务', cmds: ['ss -tn state established', 'lsof -i :8080', 'systemctl status sshd', 'journalctl -u sshd --no-pager -n 50'] },
      { title: '故障排查', cmds: ['tcpdump -i eth0 port 80 -c 100', 'iftop -i eth0', 'nslookup baidu.com', 'curl -v http://192.168.1.1'] },
    ],
  },
}
