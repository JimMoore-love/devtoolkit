# DevToolkit · 开发工具助手

> 一个跑在本机的开发环境「总控台」：看清谁在占端口、谁在跑项目，一键启停，随手排障。

端口发现 / 进程管理 / 项目托管 / 快速命令 / 网络工具箱 / MCP 接入，全部收在一个桌面应用里。

**技术栈**：Rust + Tauri 2 + Vue 3 + Vite ｜ **体积**：单文件安装包，无外部运行时依赖

---

## 功能

### 端口与服务发现
扫描本机监听端口，关联到具体进程与命令行，一眼看出「这个端口是谁占的」。

### 进程管理
按占用资源排序、查看进程详情与命令行，可结束进程。**受保护端口**上的进程禁止结束，避免误杀常驻服务。

### 项目托管
把本地项目登记进来，用命令行一键启停。带**接管（takeover）**能力：当端口已被终端里手起的进程占用时，自动杀掉并重新纳入托管。

### 快速命令
常用运维命令的收藏夹，按项目分组，带环境变量与工作目录。

### 网络工具箱（12 件）

| 分组 | 工具 |
|---|---|
| 诊断 | Ping、路由追踪、端口扫描、DNS 查询（A/AAAA/CNAME/MX/NS/TXT）、测速（带宽·延迟·抖动） |
| 查询 | 本机网络（网卡/IP/网关/DNS/出口 IP）、主机发现（ARP 同网段设备）、路由表、MAC 厂商（OUI 反查） |
| 工具 | WOL 唤醒（魔术包）、网络计算（子网划分·供电·布线·PoE 预算）、命令速查（常见厂商排障命令） |

### MCP 接入
内置 MCP 服务端，可让 Claude Code / 其他 MCP 客户端直接驱动 DevToolkit 的端口与进程能力。

---

## 架构

```
devtoolkit-app/
├── src/                        # Vue 3 前端
│   ├── App.vue
│   ├── api.js                  # 与控制口通信 + 常量镜像（与 Rust 侧一致性由自检脚本比对）
│   ├── portSpec.js             # 端口声明解析（前端唯一一份）
│   ├── store.js                # 全局状态
│   ├── components/
│   │   ├── Dashboard.vue       # 概览
│   │   ├── PortsView.vue       # 端口
│   │   ├── ProcessesView.vue   # 进程
│   │   ├── ProjectsView.vue    # 项目托管
│   │   ├── QuickCmd.vue        # 快速命令
│   │   ├── NetworkView.vue     # 网络工具箱外壳
│   │   ├── McpView.vue         # MCP 设置
│   │   └── network/            # 12 个网络工具 + 公共组件
│   └── networkTools.js         # 工具箱导航定义
├── src-tauri/                  # Rust 后端
│   ├── src/
│   │   ├── main.rs             # Tauri 命令层（含单元测试）
│   │   ├── ports.rs            # 端口声明解析（后端唯一一份，宽松/严格两条出口）
│   │   ├── network.rs          # 网络工具箱实现（Ping/Trace/Scan/DNS/测速…）
│   │   ├── control.rs          # 控制口服务端（仅 127.0.0.1 + token）
│   │   ├── control_proto.rs    # 控制口协议：命令枚举（穷举 match，漏分支即编译失败）
│   │   ├── mcp_clients.rs      # MCP 客户端管理
│   │   ├── bin/devtoolkit-mcp.rs  # MCP 服务端入口
│   │   └── platform/           # Windows / Unix 进程与命令差异抽象
│   ├── capabilities/           # Tauri 权限声明
│   └── Cargo.toml
├── scripts/                    # 校验与自测脚本（见下）
├── installers/                 # Windows / macOS 安装脚本
└── docs/
```

**Rust 后端直接依赖仅 5 个**：`tauri` / `serde` / `serde_json` / `sysinfo` / `encoding_rs`。

### 端口声明只有一份解析规则

`3000,8080`、`3000-3010`、`8080，9000` 这类写法在**整个仓库里只有一套规则**：

| 规则 | 值 |
|---|---|
| 分隔符 | `,` `;` `，` `；` `\|` 以及任意空白 |
| 端口号 | 1–65535 的十进制整数（拒绝 0、拒绝 `+80`、拒绝 `-1`） |
| 区间 | 起止颠倒视为非法；一次最多展开 **2000** 个 |
| 顺序 | 保持书写顺序、去重 |
| 非法项 | 宽松路径（运行时判定 / 端口扫描）**跳过**；严格路径（保存配置）**报错并说明原因** |

实现只有两处：后端 `src-tauri/src/ports.rs`、前端 `src/portSpec.js`
（`src/api.js` 只是 re-export，`scripts/port-guard.mjs` 直接 import 它）。
两侧一致性由 `npm run check:guards` 守 —— 它会把 `ports.rs` 里声明的用例表抠出来逐条喂给前端实现。

> 收敛之前这段逻辑有 5 份副本、规则互相矛盾，最典型的表现是
> 「端口扫描里写 `1-2000` 能跑，项目管理里写 `3000-4000` 报跨度过大」。

---

## 构建与运行

> Rust 工具链版本已由 `rust-toolchain.toml` 锁定为 **1.95.0**。
> 本项目**没有引入 `@tauri-apps/cli`**，因此不使用 `tauri dev` / `tauri build` 那套命令，
> 而是直接 `vite build` + `cargo build` 两步走。

```bash
# 1. 前端依赖
npm install

# 2. 前端产物 → dist/（Tauri 窗口从这里加载页面）
npm run build

# 3. 编译 Rust 后端
cd src-tauri && cargo build --release && cd ..

# 4. 运行
./src-tauri/target/release/devtoolkit.exe      # Windows
./src-tauri/target/release/devtoolkit          # Linux / macOS
```

只调前端时可以 `npm run dev` 起 Vite 开发服务器（浏览器里看排版），
但 `@tauri-apps/api` 相关的调用在浏览器里不可用，完整功能仍需跑第 3、4 步。

### 打包

- **Windows**：`installers/windows/install.bat` —— 用户级安装，无需管理员，不依赖 PowerShell/COM
- **macOS**：`installers/macos/build.sh` —— 产出 `.app` 与 `.dmg`
- **脚本化构建**：`build-macos.sh`（含国内 npm 镜像与 cargo 镜像配置）

---

## 控制口

DevToolkit 在 `127.0.0.1` 上开一个控制口，供外部工具（MCP 客户端、脚本）驱动：

- **默认端口 9527**，被占用则顺延（顺延时跳过受保护端口）
- **仅监听回环地址**，非 `127.0.0.1` 来源一律拒绝
- **必须携带 token**（随机生成，存在配置文件里）
- **协议**：一行请求 / 一行响应，UTF-8 JSON（**不是 HTTP**）

命令集（共 12 条）：

```
status  list_projects  scan_ports  list_ports  check_ports  list_tasks
task_log  process_detail  start_project  stop_project  takeover_project  kill_pid
```

---

## 配置

位于 `%APPDATA%\cn.devtoolkit.app\devtoolkit.json`（Linux/macOS 走对应配置目录），含：

- 受保护端口（默认 `[8000, 9528]`，改成自己的常驻服务端口即可）
- 控制口端口与 token
- 项目、快速命令、环境变量

---

## 自检脚本

改动后建议跑一遍，比人眼快：

```bash
npm run check          # 模板引用检查（Vue 模板里引用了不存在的变量会报）
npm run check:guards   # 护栏自检：保护闸门 + 端口解析单一真相（防副本复活 / 防两侧漂移）
npm run ports          # 端口规则一致性（前后端常量镜像比对）
npm run verify         # 冒烟测试（完整）
npm run verify:quick   # 冒烟测试（快速）
npm run mcp:selftest   # MCP 自检
npm run mcp:protocol   # MCP 协议测试
npm run mcp:verify     # MCP 端到端验证
npm run ui:harness     # UI 走查
npm run protect:e2e    # 受保护端口端到端
```

Rust 侧：

```bash
cd src-tauri && cargo test    # 104 个单元测试
cd src-tauri && cargo check   # 零告警
```

---

## 已知问题

- **`main.rs` 仍有 2568 行**（含大量单元测试），后续计划拆分为 `lib.rs` + `model` / `sys` / `task` / `net` / `mcp` 模块
- **`scripts/ctl-once.mjs` 把控制口端口写死成 9527**，没读配置里的 `mcp_port` —— 改过控制口端口后这个脚本会连不上
- **部分 dev 脚本依赖同步子进程**（`scripts/smoke-test.mjs`、`scripts/guard-selftest.mjs`）。在同步创建进程被拦截的机器上跑不起来（实测某台 Windows 上杀软实时扫描会让 `spawnSync` 直接返回 `EBUSY`；`vite build` / `cargo build` 这类异步启动不受影响）

详细的体检数据与优化方案见 **[`docs/工程优化方案.md`](docs/工程优化方案.md)**。

---

## 许可

暂未指定。
