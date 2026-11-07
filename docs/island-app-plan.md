# DCX 第三个独立应用：桌面信息岛（移植方案）

> 2026-11-07 · 分析对象：[WinIsland](https://github.com/WinIslandProject/WinIsland) 与 [island.flyliu.org / fly-island](https://github.com/flyl1u/fly-island)
> 目标：把"时间、系统性能、快捷任务与提醒、多渠道路由通知、剪贴板内容"做成 DCX 的**第三个独立应用**。
> 明确排除：**插件系统**、**灵言（LingoSoul）AI 助手**。
> 本文是开发方案，不含代码改动；所有结论均标注来源文件 / API，未核实项显式标注「待实机确认」。

---

## 一、结论先行

1. **两个链接是两个不同项目，不是一个产品的仓库 + 官网。**
   - `github.com/WinIslandProject/WinIsland` = **WinIsland**，GPL-3.0（2026-06-06 由 MIT 变更），Rust 2024 + D3D12 渲染，主线是媒体/歌词/液态玻璃/通知/资源小组件/插件/灵言 AI。（"官网 tanikaze.icu / 852 commits" 属快照期数据，未在 README 中复核，已移出事实断言。）
   - `island.flyliu.org` = **fly-island**（原 FocuSD），MIT（`LICENSE` 于 2026-10-27 才加入，此前版本的许可状态不明确），Tauri 2 + React 19 + Rust，主线是效率聚合：待办、图文剪贴板、QQ/微信/QQ 邮箱提醒、翻译、AI 状态灯、媒体控制。
2. **用户要的 5 项功能横跨两个项目**，没有一项能靠"复制一个模块"完成：时间/性能来自 WinIsland 的小组件体系，任务/剪贴板/消息聚合来自 fly-island。
3. **不能靠搬源码实现，也不应该搬**：WinIsland 是 GPL-3.0，复制代码会让 MIT 许可的 DCX 整体受 GPL 约束；fly-island 是 MIT 但技术栈是 Tauri/React，与 DCX 的 egui + 手写 Win32 FFI 不兼容。结论是**功能对齐 + 自主实现**，不引入两个项目的任何源文件或资源。
4. **形态建议**：屏幕顶部居中悬浮胶囊（fly-island 形态），**不占用任务栏槽位**。`box_slot.txt` 的 52 px 预留槽位是收纳盒专属，再加一个自绘按钮会与现有引擎线协议冲突；因此本次**不改 `dcx_engine.dll`、不改注入器、不动 `box_slot.txt` 协议**。
5. **接入成本**：新增 `crates/dcx-island` + `apps.rs` 注册一条 + 设置中心新增一页 + 一个全局热键 + 分发脚本补一行。平台侧改动约 6 个现有文件，其余都在新 crate 内。
6. **通知能力有硬边界**（详见 §4.5）：个人 QQ / 微信**只能入站聚合提醒**，没有官方出站发送接口；企业微信**出站**可走群机器人 webhook。**入站**若采用官方「智能机器人长连接（WebSocket）」API 模式，明确支持「无公网 IP / 内网部署」，**不需要公网回调 URL**（见 §9 修订 F1）；本期仍未实现该通道，只保留本地任务栏关注信号。方案不承诺读取聊天正文、不承诺发消息到个人号。

---

## 二、源项目分析

### 2.1 项目对照

| 维度 | WinIsland | fly-island |
|---|---|---|
| 定位 | Windows「灵动岛」视觉/媒体体验 | Windows 顶部效率聚合小岛 |
| 许可证 | **GPL-3.0**（`LICENSE`，2026-06 从 MIT 变更） | **MIT**（`LICENSE`） |
| 技术栈 | Rust 2024 + `windows 0.62.2` + D3D12/DirectComposition（`winisland-render`） | Tauri 2 + WebView2 + React 19/TS + Rust 原生命令 |
| 版本 | v2.0.0（`Cargo.toml`） | v0.2.x（官网 / Release，仍属 0.x） |
| 主 crate | `winisland-core` / `platform-windows` / `render` / `plugin-*` ×5 / `pollkit` / `settings-schema` | `clipboard_history` / `qq_notifications` / `wechat_reminders` / `qq_mail` / `media_control` / `agent_status` / `island` / `system` |
| 核心功能 | 媒体与歌词、液态玻璃、CPU/内存/磁盘/网络/GPU 小组件、时间小组件、计时器、通知镜像、日历插件、灵言 AI、插件市场 | 待办与每日笔记、图文剪贴板历史、QQ/微信/QQ 邮箱提醒、快捷翻译、AI Agent 状态灯、媒体控制 |
| 数据来源（已核实） | `src/ui/widget/time_text.rs`、`resource_usage.rs`（1 s 采样 / 400 ms 过渡）、`crates/winisland-platform-windows/src/metrics.rs` | `src-tauri/src/clipboard_history/*`、`qq_notifications/listener.rs`、`wechat_reminders/source.rs`、`qq_mail/*` |
| 对本次移植的价值 | 时间显示、性能指标定义与取数 API、紧凑/展开双形态交互 | 任务/提醒数据模型、剪贴板历史、通知渠道的**可实现边界**、凭据存放策略 |

### 2.2 功能来源映射（用户 5 项需求）

| 需求 | 主要来源 | 可复用的**机制**（非代码） | 自主实现要点 |
|---|---|---|---|
| 显示时间 | WinIsland `time_text.rs` / `rolling_time.rs`；Changelog v1.2.2「时间、日历小组件」 | 紧凑态常显 `HH:MM`，展开态显示日期/星期 | `GetLocalTime` + `SYSTEMTIME`；秒级 repaint，不引入 chrono |
| 系统性能 | WinIsland `resource_usage.rs` + `metrics.rs` | 指标集 CPU/RAM/Disk/网络上下行/GPU；1 s 采样、差分算速率、环形/条形两种样式、>75% 变色 | `GetSystemTimes` / `GlobalMemoryStatusEx` / `GetIfTable2`+`FreeMibTable` / `GetDiskFreeSpaceExW`；GPU 首版不做 |
| 快捷定义任务和提醒 | fly-island README「待办、每日笔记与归档」；WinIsland Changelog v1.4.1「计时器与结束提醒」 | 当天任务、当前任务置顶、完成标记、未完成自动结转、Markdown 归档 | 任务表 + 1 s 定时检查 + 岛内弹层提醒；重复规则与结转策略 |
| 接入 QQ / 微信 / 企业微信 / 邮箱通知 | fly-island `qq_notifications`、`wechat_reminders`、`qq_mail` | QQ = WinRT `UserNotificationListener`（AUMID `QQ`）；微信 = `RegisterShellHookWindow` + `HSHELL_FLASH`；邮箱 = IMAP 授权码 + 凭据管理器 | 企业微信为**新增渠道**；见 §5 能力矩阵 |
| 剪切版内容 | fly-island `clipboard_history/*` | 文本/图片历史、搜索/备注/收藏/删除、一键回写、条数与图片开关 | `AddClipboardFormatListener` + `WM_CLIPBOARDUPDATE` + 去重 + 本地持久化 |

### 2.3 明确不移植

| 排除项 | 理由 |
|---|---|
| 插件系统 / 插件市场 / 插件 API / 日历插件 | 用户明确排除；WinIsland 有 5 个 plugin crate + `src/plugin/`，引入会重建一套动态加载与签名校验（`ed25519-dalek`、`libloading`） |
| 灵言 LingoSoul | 用户明确排除；涉及 `src/core/assistant.rs`、`lingosoul_config.rs`、`lingosoul_install.rs`、`src/ui/lingosoul_chat.rs`、MCP / Agent 接入、语音唤醒 |
| 媒体控制、歌词、频谱、液态玻璃、D3D12 渲染 | 非需求；与 DCX 的 egui 渲染栈冲突，且 WinIsland 的歌词/渲染代码正属 GPL |
| 快捷翻译、AI 状态灯 | 非需求；翻译需外部 API Key，AI 状态灯依赖 Codex/Claude Code 钩子 |
| Windows 通知数据库直读 | 避免频繁写系统数据库（WinIsland Changelog v1.2.9 曾专门修此问题） |

---

## 三、目标架构

### 3.1 命名与进程模型

| 项 | 取值 |
|---|---|
| crate / exe | `dcx-island` / `dcx-island.exe` |
| 应用 id（`apps.toml` 键） | `island` |
| 设置中心显示名 | 「桌面信息岛」（可改为「灵动岛」） |
| 主窗口标题 | 「桌面信息岛」（必须与 `AppEntry::window_title` 逐字一致，`apps.rs:33-34`） |
| 单实例互斥体 | `DCX_ISLAND_SINGLETON`（参考 `dcx-box/src/main.rs:55-60`） |
| 默认启停 | **建议 `default_on = false`**，与 `dcx-search` 一致；常驻 topmost 窗口对现有用户的任务栏自动隐藏环境有影响，先由设置中心显式开启 |
| 默认热键 | `Alt+I`（`Alt+D` / `Alt+S` 已占用；默认值见 `dcx-host/src/hotkey.rs` 的 `box_hotkey_setting()` / `search_hotkey_setting()`，约 186/190 行。原方案引用的 29-30 行是 `BOX_HOTKEY_ID` / `SEARCH_HOTKEY_ID` 常量，不是快捷键值，见 §9 修订 F5） |
| 设置中心页 | 第 **5** 页（`PAGE_ISLAND = 4`）。当前代码已有 4 页：任务栏样式 / 桌面整理 / 文件检索 / 关于；原方案按 3 页写，见 §9 修订 F4 |
| 崩溃日志 | `%LOCALAPPDATA%\dcx\island_panic.log`（release `panic=abort`，参考 `dcx-search/src/main.rs:59-73`） |

```text
dcx-host.exe   平台：托盘 / 注入看门狗 / 统一设置中心 / 全局热键 / apps.toml
   ├── dcx-box.exe      任务栏收纳盒（不动）
   ├── dcx-search.exe   文件检索（不动）
   └── dcx-island.exe   本次新增：一个进程 = 一个 winit 事件循环 = 一个崩溃域
         ├── **一个窗口、一个 viewport**：窗口矩形按紧凑/展开内容缩放
         │     （紧凑画胶囊，展开时向下并向两侧长成胶囊 + 面板）
         └── 后台线程：剪贴板监听、性能采样、提醒计时、通知渠道轮询
                 ↓ 文件即 IPC
        %LOCALAPPDATA%\dcx\island.toml / apps.toml / island_show
        %LOCALAPPDATA%\dcx\island_tasks.json / island_clipboard.json（数据）
        Windows 凭据管理器（IMAP/SMTP 授权码 / webhook key，不落 TOML）
```

**关键约束（来自本项目既有踩坑记录）**：

- 一个进程只允许一个 eframe/winit 事件循环 → 不要另起 `run_native`。**修订（§9 O1）：最终采用「单窗口 + 单 viewport」，窗口矩形随内容缩放**，而不是两个 viewport 或 `dcx-box` 的 tile/panel 两个窗口。窗口永远不大于它正在绘制的内容，这是避免透明区域吞掉桌面点击最可靠的方式（`HTTRANSPARENT` 不跨进程转发，窗口区域在 DWM 合成面上会留残影）。
- 每帧 `ctx.set_pixels_per_point(scale)`；窗口按物理像素摆放、`Rect` 用逻辑点。本机 125% DPI，误设 1.0 会导致绘制与点击错位。
- 顶层窗口用 `WS_EX_TOOLWINDOW` + `with_always_on_top()`，`DWMWA_BORDER_COLOR = COLOR_NONE` + `DWMWA_WINDOW_CORNER_PREFERENCE = DONOTROUND`（`dcx-win32/src/lib.rs:337-351` 已有常量）。**修订（§9 F3）：「每帧 `WindowFromPoint` + `SetWindowPos(HWND_TOPMOST)`」是收纳盒在任务栏进程内抢 z 序的历史做法，对独立 topmost 窗口并非必需，当前实现只在建窗时设一次属性；是否需要逐帧重置，待实机在自动隐藏任务栏场景下确认。**
- 需要临时隐藏时把窗口移到 `(-32000,-32000)`，**不要** `Visible(false)`（隐藏后 eframe 不再调 `update`）。
- 所有 TOML/JSON 写入走 `dcx_win32::config_file`：`FileLock::acquire` → `write_atomic`，解析失败标 `note_parse_error` 并只读（`config_file.rs:91/149/214-243`）。不允许 `fs::write` + rename 回退。
- **双写合并**：`island.toml` 由宿主设置窗口与信息岛进程共写。宿主保存时必须把应用独有的 `ui` 表（上次页签、面板宽高）从磁盘读回再写，否则一次滑块改动会回滚应用刚保存的几何值。
- **凭据目标名统一**：`DCX/island/<key>` 由共享 `dcx-island-config::credential` 定义，宿主写入、信息岛读取；两处各写一份 FFI 会因目标名漂移而静默失效。

### 3.2 新 crate 目录

**实际落地结构**（与初稿目录树的差异见 §9 修订 O2）：

```text
crates/dcx-island-config/          # 共享：宿主设置窗口也要用，不链接 eframe
└── src/lib.rs                     # island.toml 模型 + load/save/repair（走 config_file）
    src/credential.rs              # CredWriteW / CredReadW / CredDeleteW，目标名统一
crates/dcx-island/
├── Cargo.toml                     # egui/eframe 0.29 + winit =0.30.13（与 host 一致）
├── build.rs                       # 照抄 dcx-search/build.rs，嵌 brand/island.ico
└── src/
    ├── main.rs                    # DPI awareness、单实例、panic hook、--help
    ├── app.rs                     # 唯一 eframe::App：单窗口几何、DPI、重绘、light-dismiss
    ├── ui/                        # compact.rs / panel.rs / theme.rs / widgets.rs
    ├── win.rs                     # 消息窗口、DWM 属性、窗口矩形、show marker
    ├── sys.rs                     # 手写 Win32 FFI：性能指标、剪贴板、WinHTTP、凭据转发
    ├── clock.rs / metrics.rs      # GetLocalTime 格式化 / 1 s 采样线程
    ├── clipboard.rs / tasks.rs    # 历史存储与回写 / 任务提醒与结转
    ├── inbox.rs / notify/         # 统一收件箱 / mod.rs + shell_hook.rs + qq.rs + mail.rs
    └── dispatch.rs                # 出站：企业微信 webhook / SMTP
```

**依赖策略（已落地）**：Win32 能力仍以手写 FFI 为主；**只为 WinRT 通知引入 `windows = 0.62`**，最小 feature 集实测必须是 `UI_Notifications`、`UI_Notifications_Management`、`Foundation`、`Foundation_Collections` **再加 `ApplicationModel`**（`UserNotification::AppInfo` / `AppUserModelId` 的 feature gate）与 `Win32_System_WinRT`（`RoInitialize` / `RoUninitialize`，走 `windows-link` 的 raw-dylib，**不需要 Windows SDK 的 import lib**，因此无需 vcvars）。异步结果用 `windows-future` 的 `IAsyncOperation::join()`（0.3 起是 inherent method，`Async` trait 私有，不能直接 `use`）。

**修订（§9 F6）**：初稿设想的 `ureq + rustls`、`imap` 客户端 crate 均未引入。IMAP/SMTP 是手写协议（仅 4 / 6 条命令）+ `native-tls`（Windows 上是 SChannel，无 OpenSSL），HTTPS 出站是手写 WinHTTP FFI，体积与构建依赖都更小。其余新增：`serde` / `serde_json` / `toml` / `regex` / `encoding_rs`（GB2312 邮件头）/ `image`（PNG 缩略图）/ `memmap2`。

---

## 四、核心功能实现方案

### 4.1 时间

- `GetLocalTime(&mut SYSTEMTIME)`；`SYSTEMTIME` 结构在 `dcx-win32/src/lib.rs:69` 已定义，只需补 FFI 声明。
- 紧凑态显示 `HH:MM`（可配置 12/24h、是否显示秒/星期/日期）；展开面板显示完整日期。
- `ctx.request_repaint_after(到下一整秒的剩余时间)`，避免每帧无意义重绘；不引入 chrono（本机时区/夏令时交给系统 API）。

### 4.2 系统性能

来源：`crates/winisland-platform-windows/src/metrics.rs`（逐个 API 已核对）与 `src/ui/widget/resource_usage.rs`（`SAMPLE_INTERVAL = 1s`、`TRANSITION_DURATION = 400ms`、阈值 0.75/0.90 变色）。

| 指标 | Win32 API | 计算 |
|---|---|---|
| CPU | `GetSystemTimes` → idle / kernel / user `FILETIME` | `1 - Δidle / Δ(kernel+user)`，两次采样差分 |
| 内存 | `GlobalMemoryStatusEx` + `MEMORYSTATUSEX` | `ullTotalPhys - ullAvailPhys`、`dwMemoryLoad` |
| 磁盘 | `GetDiskFreeSpaceExW`（`%SystemDrive%\`） | `已用 = total - free` |
| 网络 | `GetIfTable2` / `FreeMibTable` + `MIB_IF_ROW2` | 过滤 `OperStatus != Up`、`Type == 24` 回环、filter 标志；优先硬件接口计数，差分得上下行速率 |
| GPU | 首版**不做** | WinIsland Changelog v1.4.2 记录过"GPU 占用与任务管理器不一致""多 GPU 分别显示"的返工；需要 PDH/DXGI 额外调研 |

独立采样线程 1 s 一次，`Arc<Mutex<Sample>>` 交 UI 线程读取；UI 侧做 400 ms 平滑过渡。设置项：各指标开关、顺序、样式（条/环）、刷新率（1 s / 2 s / 5 s）。

### 4.3 剪贴板内容

- 独立线程建 `HWND_MESSAGE` 消息窗口（与 `dcx-host/src/hotkey.rs` 同套路），`AddClipboardFormatListener` 收 `WM_CLIPBOARDUPDATE(0x031D)`；`OpenClipboard` / `GetClipboardData` 读 `CF_UNICODETEXT`、`CF_DIB`（图片）、`CF_HDROP`（文件列表，可选）。
- 去重：内容哈希 + 与最近一条比对；忽略自身回写（设置 `self_write` 标记）防止记录循环。
- 历史：`clipboard.json`（`write_atomic` + 锁），条目 `{id, kind, sha256, text|image_path, created_at, pinned, note}`；图片单独落 `%LOCALAPPDATA%\dcx\clipboard\`。
- 设置：记录开关、是否记录图片、最大条数、单条文本上限、敏感词/正则排除（默认不记录疑似密码/令牌）、全局唤出热键。
- 面板：搜索、收藏、备注、删除、一键 `SetClipboardData` 回写。
- **隐私**：全部本地，不上传；退出时按设置清理过期条目；首次开启时给出明确提示。

### 4.4 任务与提醒

数据模型（示意，非最终字段）：

```text
Task { id, title, note, due_at, repeat(Once/Daily/Weekly/Workday/Custom),
       done, current, carry_over, channels[], created_at, done_at }
```

- 存储 `tasks.json`，所有写走 `config_file`；每日首次启动执行"未完成任务自动结转"（fly-island README 行为）。
- 快捷创建：面板输入框回车即建；支持 `今天 15:30`、`30 分钟后` 等轻量解析（首版只做绝对时间 + 分钟倒计时，避免引入自然语言解析的歧义）。
- 到点判定：1 s tick 检查 `due_at`；触发岛内弹层 + 紧凑态角标 + 可选 Windows 托盘气泡 + 可选出站推送（§5.3）。
- 归档：可选按天导出 Markdown（与 fly-island 对齐），路径可在设置里指定。
- **一个应用只有一个启停开关**：设置中心「桌面信息岛」页顶部「启用桌面信息岛」同时写 `island.toml` 的 `enabled` 与 `apps.toml`（本项目 2026-11-04 的既有约定：两处开关会造成状态不一致）。

### 4.5 通知渠道（关键边界）

#### 4.5.1 能力矩阵

| 渠道 | 入站（聚合新消息提醒） | 出站（把提醒推出去） | 依据 / 限制 |
|---|---|---|---|
| QQ | ✅ WinRT `UserNotificationListener` + `GetNotificationsAsync(NotificationKinds::Toast)`，按 `AppInfo().AppUserModelId() == "QQ"` 过滤 | ❌ 无官方个人号发送接口 | fly-island `qq_notifications/listener.rs` 中 `QQ_APP_ID = "QQ"` 为**精确相等**比较（已复核源码）；权限用**同步** `GetAccessStatus()` 读取而不是 `RequestAccessAsync()`（避免无人值守桌面弹授权框把线程挂死，见 §9 O3）。未打包 exe 常得到 `Unspecified`，此时渠道报「不可用」而非静默重试。QQ 通知注册可能需修复（该仓库内嵌 PowerShell 脚本） |
| 微信 | ✅ `RegisterWindowMessageW("SHELLHOOK")` + `RegisterShellHookWindow`，识别 `HSHELL_FLASH = HSHELL_REDRAW \| 0x8000`，按进程路径过滤 `Weixin.exe` / `WeChat.exe` | ❌ 无官方个人号发送接口 | fly-island `wechat_reminders/source.rs`；**只读"任务栏关注"信号，不读聊天正文/发送人/未读数**；安装路径可从 HKCU `Software\Tencent\Weixin` / `WeChat` 的 `InstallPath` 读（`dcx-win32/src/lib.rs:759` 的 `reg_read_string` 可直接复用） |
| 企业微信 | ⚠️ 本期复用 SHELLHOOK 关注信号；**进程名与是否触发 HSHELL_FLASH 待实机确认** | ✅ 群机器人 webhook：`POST https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=...`，20 条/分钟 | **修订（§9 F1）**：官方「智能机器人长连接」API 模式（`wss://openws.work.weixin.qq.com`，BotID + Secret，30 s 心跳）明确支持**无公网 IP / 内网部署**，不需要回调 URL、不需要消息加解密；原方案称"桌面端不具备条件"是把 URL 回调模式的限制套到了全部模式上。本期仍未实现该通道，只做本地关注提示；后续可评估作为正式入站渠道 |
| 邮箱 | ✅ IMAP（授权码，非登录密码），仅同步邮件头，正文按需读取 | ✅ SMTP | fly-island README「QQ 邮箱」；凭据存 Windows 凭据管理器，不进 TOML；首次同步不提醒历史邮件 |
| 系统通知 | 可选：统一镜像 WinRT toast | — | 需权限与 AUMID，P2 再评估 |

#### 4.5.2 入站聚合的统一模型

```text
InboxItem { id, source(QQ/WeChat/WeCom/Mail/System), title, body,
            received_at, read, payload_ref }
```

- 每个渠道一个独立线程/任务，失败降级为「渠道不可用 + 诊断文案」，**不阻塞岛体 UI**。
- **修订（§9 F8）**：渠道开关在宿主设置页，权限/错误/上次成功时间的诊断卡片在**岛内「消息」页**；宿主设置页给的是「能力边界」说明与凭据保存状态。错误统一写 `island.log`（256 KB 截断轮转），不是单独的 `island_notify.log`。
- QQ 通知与邮箱各保留最近 N 项（fly-island 为 20），**首版只内存保存，不落盘聊天内容**；微信/企业微信只存"有提醒"计数。

#### 4.5.3 出站推送（任务提醒 → 渠道）

- 企业微信群机器人 webhook：只发文本/markdown；key 按密码级管理（凭据管理器），不写日志、不进仓库；遵守 20 条/分钟限流与失败重试上限。
- 邮箱 SMTP：复用 IMAP 凭据配置中的发件地址，独立密码项。
- **QQ / 微信个人号不做**：无官方开放接口，不做协议逆向、不做 UI 自动化模拟发送（合规与稳定性风险）。设置页明确标注「不支持」，避免用户误以为能双向收发。

#### 4.5.4 语义澄清（已按 §8 决策执行）

"接入……通知"存在两种口径。**已决策：以入站聚合为主、出站推送为可选增强**（用户要求「未出成品前不要问，按规划推进」，故按推荐值自行拍板）。企业微信 webhook 与 SMTP 作为任务到点时的可选增强同时落地；QQ/微信个人号出站明确不做。

**补充事实缺口（§9 F7）**：初稿只写了"凭据存 Windows 凭据管理器"，没写**谁写入、从哪里配置**。实现中把凭据 FFI 放进共享 `dcx-island-config::credential`，宿主设置页提供三个掩码输入框 + 显式「保存 / 清除」按钮（不参与 220 ms 防抖自动保存），目标名 `DCX/island/<key>` 由共享模块统一定义。没有这一步，出站与 IMAP 渠道永远处于"未配置"状态。

---

## 五、与现有平台的集成点（逐文件）

| 文件 | 改动 | 参考位置 |
|---|---|---|
| `dcx/Cargo.toml` | `members` 增加 `"crates/dcx-island-config"` 与 `"crates/dcx-island"` **两项** | 第 3-16 行 |
| `crates/dcx-host/src/apps.rs` | `REGISTRY` 增加**第三条** `AppEntry { id:"island", name:"桌面信息岛", exe:"dcx-island.exe", window_title:"桌面信息岛", description:..., default_on:false }` | 第 46-64 行 |
| `crates/dcx-host/src/settings_app.rs` | 新增 `Page::Island`、`PAGE_ISLAND = 4`、`from_code` 分支、**侧栏第 5 项**、`page_head`、`salt`、内容 `match` 分支、`island_cards()`（唯一启停开关 + 常规/性能/任务剪贴板/入站/出站凭据/能力边界 6 张卡）、`NavIcon::Island` + fallback 线框图标、`persist_island()` / `island_tick()` / `flush()`、凭据输入缓冲与 `island_cred_row()` | `PAGE_*` 常量、`Page` enum、侧栏、`ui()` 主分发；卡片写法仿 `search_cards()`。**修订 F4：初稿按「侧栏 3 项」写，实际已有 4 页** |
| `crates/dcx-host/src/hotkey.rs` | 新增 `ISLAND_HOTKEY_ID = 3`、`Alt+I` 默认绑定、`island_show` 标记、`island_hotkey_error.txt`、`last_island_error()`；沿用 `HWND_MESSAGE` + `RegisterHotKey`。**额外约束**：热键要求 `island.toml enabled` 与 `apps.toml` 同时为 true，否则不复活已关闭的 topmost 窗口 | `hotkey_setting` / `rebind` / `fire`；默认值在 `box_hotkey_setting` / `search_hotkey_setting` 附近 |
| `crates/dcx-host/Cargo.toml` | 按推荐方案落地：只加 `dcx-island-config`，**不**依赖 GUI crate `dcx-island` | 依赖表 |
| `crates/dcx-win32/src/lib.rs` | **未改**；`SYSTEMTIME`（第 69 行）、DWM 常量（337-351）、`reg_read_string`（759）已具备。新增 FFI 全部放 `dcx-island/src/sys.rs` / `win.rs`。**注意 `SYSTEMTIME` 字段是 `hour/minute/second/milliseconds`（snake_case），不是 `wHour`**；初稿未提示，本次编译在此踩坑 | 第 69、337-351、759 行 |
| `brand/island.ico` + `brand/embed/island-64.rgba` + `tools/gen_island_icon.py` + `crates/dcx-island/build.rs` | 独立应用图标（照抄 `dcx-search/build.rs`，SDK 缺失时只警告不失败） | `dcx-search/build.rs` 全文 |
| `tools/stop-dcx-all.bat` | 增加 `ISLAND=dcx-island.exe`、第 2 步 `taskkill`、等待循环与重试列表 | 第 21-25、35-38、43、49-51 行 |
| `D:\dcx-dist\` | 增加 `dcx-island.exe`；重算 `SHA256SUMS.txt`；`start-dcx.bat` 若逐个列 exe 也要补 | 工程笔记 §5.2 |
| `README.md` | 简介、功能表、新增「桌面信息岛」小节、mermaid 架构图加 `Island` 节点、组件表、目录结构、快捷键表、设置页列表、配置文件表、文档索引（顺带修正已失效的 `prd.md` → `docs/prd.md` 链接） | 全文按章节 |
| `docs/engineering-notes.md` | 组件表、目录树、配置线协议、分发清单、设置中心页数与每应用独立页面说明 | §1 / §2 / §3 / §5.2 / §6.3 |
| `docs/island-app-plan.md`（本文） | 事实修订 + §9 复核记录 | 本文 |

**明确不需要改**：`dcx_engine.dll`、`dcx-host/src/injector.rs`、`box_slot.txt` / `box_host_alive` / `no_reserve` 线协议、收纳盒与文件检索的代码。理由：新岛是屏幕顶部独立悬浮窗口，不占任务栏槽位、不依赖引擎 XAML 量测。

---

## 六、分阶段实施与验收

| 阶段 | 范围 | 验收标准（必须实机，编译通过不算） |
|---|---|---|
| **P0 骨架** | 新 crate + `apps.rs` 注册 + 设置页 + `Alt+I` + 图标 + 部署脚本；紧凑胶囊显示时间与 CPU/内存；展开面板空壳 | ① 宿主看门狗能拉起/停止，强杀后 5 s 内重启；② `WM_CLOSE` 能正常退出；③ 125% DPI 下点击与绘制不错位；④ 任务栏自动隐藏滑出后胶囊仍可见（z 序）；⑤ `WM_NCHITTEST` 验证**紧凑态窗口矩形之外**不吞桌面点击（单窗口模型下窗口自身即等于胶囊矩形，见 §9 O1）；⑥ 四角无黑边；⑦ 只 kill `dcx-island.exe` 即生效，不动 host/Explorer |
| **P1 数据与本地功能** | 剪贴板历史（文本 + 图片）、任务/提醒、自动结转、面板四 tab、性能剩余指标与设置 | 复制文本/图片后岛内出现条目、一键回写与原内容一致；提醒到点弹层；`island_tasks.json`/`island_clipboard.json` 半写模拟不产生损坏文件；连续复制 100 条无内存增长异常 |
| **P2 入站通知** | 微信 SHELLHOOK、企业微信关注信号（待确认）、QQ WinRT 通知、邮箱 IMAP、统一收件箱与诊断卡片 | 各渠道权限被拒时显示明确文案而非静默；关闭渠道不产生 CPU 空转；QQ AUMID 过滤不误收其他应用通知；凭据不出现在任何日志/TOML |
| **P3 出站与打磨** | 企业微信 webhook、SMTP、限流重试、隐私清理、多显示器/DPI 切换、主题跟随 | 提醒推送 20 条/分钟限流不触发封禁；断网时失败可重试且不卡 UI；多显示器移动后位置正确 |

每阶段结束后按项目既有约定更新 `README.md`、`docs/engineering-notes.md` 与 `SHA256SUMS.txt`；部署后先校验 `D:\dcx-dist` 产物哈希再启动。

---

## 七、风险与合规

| 风险 | 影响 | 对策 |
|---|---|---|
| **许可证**：WinIsland 为 GPL-3.0 | 复制源码会让 MIT 的 DCX 整体受 GPL 约束 | 只借鉴功能/交互/API 选择，全部代码自主实现；不复制任何源文件、图标、字体、资源。fly-island 的 `LICENSE` 是 2026-10-27 才补上的，引用其片段须核对**该时点之后**的提交并保留版权声明 |
| WinRT 通知权限 / QQ 注册修复 | QQ 渠道可能长期不可用 | 状态机 + 诊断卡片 + 手动重试；用同步 `GetAccessStatus()` 避免授权弹窗挂死线程；不作为 P0 阻塞项 |
| 微信/企业微信 SHELLHOOK 只能拿到"关注"信号 | 不能显示正文、发送人、未读数 | UI 明确写"有新提醒"，不伪装成消息列表；企业微信是否触发 `HSHELL_FLASH` **待实机确认**，不确认不写死 |
| **企业微信入站能力被低估**（原表此行判断错误） | 原以为桌面端无法收消息，实际长连接模式可以 | 官方智能机器人长连接（`wss://openws.work.weixin.qq.com` + BotID/Secret + 30 s 心跳）支持无公网 IP。本期仍不做服务器/长连接实现（需要企业管理员配置、单连接互斥、断线重连），只保留本地关注信号；如需正式入站，另立需求评估 |
| QQ/微信个人号无出站 API | 不能"发消息到 QQ/微信" | 设置页显式标注不支持，不做逆向/UI 自动化 |
| 凭据泄露 | IMAP/SMTP 授权码、webhook key | Windows 凭据管理器（`CredWriteW`/`CredReadW`），目标名由共享 crate 统一；不进 TOML、不进日志、不进截图、不参与防抖自动保存 |
| 剪贴板隐私 | 可能记录密码、令牌、敏感文档 | 默认开启排除规则、图片可关、条数上限、本地存储、退出清理、首次开启提示 |
| topmost + 自动隐藏任务栏 | 胶囊被遮挡或吞点击 | **单窗口 = 单 viewport**，窗口矩形恒等于绘制区域，从结构上消除透明吞点击；`set_hit()` / `set_region()` 在 2026-11-05 窗口模型重构后已是死代码，**不再沿用**。真实鼠标 + 截图验证（`tools/box_shot.py`、`tools/box_input.py` 可改造复用） |
| 一个进程两个 winit 循环 | 启动失败 / 侧栏点击失效 | 只用一个 `run_native`；紧凑与展开共用同一个窗口/viewport，靠 `SetWindowPos` 改变矩形 |
| 多进程写配置 | 半写 TOML 导致状态重置 | `FileLock` + `write_atomic` + poisoned 只读，禁止 rename 回退；宿主保存时合并应用独有的 `ui` 表 |
| GPU 指标口径 | 与任务管理器数值不一致引发误报 | 首版不做 GPU；后续单独调研 PDH/DXGI 并做实机比对 |

---

## 八、三项决策（已按推荐值执行，无需再确认）

用户要求「未出成品前不要问我，按你的规划推进」，故全部按推荐值自行拍板并落地：

| # | 决策 | 结果 |
|---|---|---|
| 1 | 窗口形态 | **屏幕顶部悬浮胶囊**。不动 `dcx_engine.dll`、不占任务栏槽位、不扩展 `box_slot.txt` 协议 |
| 2 | 默认启停 | **默认关闭**，`apps.toml` 与 `island.toml` 均为 `enabled=false`，由设置中心唯一开关显式开启 |
| 3 | 通知语义 | **以入站聚合为主、出站推送为可选增强**；企业微信 webhook / SMTP 同步落地，QQ / 微信个人号出站明确不支持 |

三项均已写入代码与 §9 的复核记录。

---

## 附：本次分析的信息来源

| 结论 | 来源 |
|---|---|
| WinIsland 功能与版本 | `README.md` / `README-zh.md` / `Changelog-zh.md`（master 分支，v2.0.0，2026-10-26 发布） |
| WinIsland 技术栈与许可证 | 根 `Cargo.toml`（edition 2024、`windows = 0.62.2`、D3D12 / DirectComposition features）、`LICENSE` 变更提交 2026-06-06「Change license from MIT to GNU GPL v3」 |
| WinIsland 时间/性能组件 | `src/ui/widget/time_text.rs`、`src/ui/rolling_time.rs`、`src/ui/widget/resource_usage.rs`、`crates/winisland-platform-windows/src/metrics.rs` |
| fly-island 功能与边界 | `README.md`、`island.flyliu.org` 首页（Tasks & notes / Clipboard / Media controls / Notifications / Quick translation / AI task status） |
| fly-island 通知实现 | `src-tauri/src/qq_notifications/listener.rs`（`QQ_APP_ID = "QQ"` 精确过滤、`GetAccessStatus()` 同步读权限）、`wechat_reminders/source.rs`、`messenger_activation.rs`、`qq_mail/`、`clipboard_history/`；`LICENSE` 于 2026-10-27 加入 |
| 企业微信接口能力 | 官方 [智能机器人长连接](https://developer.work.weixin.qq.com/document/path/101463)、[长连接概述](https://developer.work.weixin.qq.com/document/path/101956)、[开发前必读](https://developer.work.weixin.qq.com/document/62131)、[接收消息（长连接）](https://developer.work.weixin.qq.com/document/62155) → 现导航到 `path/101834`；群机器人 `webhook/send` 20 条/分钟 |
| DCX 架构与约定 | `Cargo.toml`、`crates/dcx-host/src/{apps,settings_app,hotkey}.rs`、`crates/dcx-box/src/main.rs`、`crates/dcx-search/src/main.rs`、`crates/dcx-win32/src/{lib,config_file}.rs`、`README.md`、`docs/engineering-notes.md`、`tools/stop-dcx-all.bat` |
| 本次复核方式 | GitHub 仓库页 / raw 文件实读 + 官方文档实读 + 本仓库 `cargo build --workspace` / `cargo test --workspace` 实跑 |

---

## 九、方案复核与修订记录（2026-11-07）

> 分支 `feat/island-complete`。本节是**复核结论 + 落地结果**，正文中的错误处已就地加「修订」标记并指向本节的编号。

### 9.1 事实错误（F）

| # | 位置 | 原表述 | 核实结论 | 处置 |
|---|---|---|---|---|
| **F1** | §1.6、§4.5.1、§七、附录 | 企业微信入站「需要公网回调服务，桌面端不具备条件」 | **错误**。企业微信智能机器人 API 模式有两种互斥连接方式：URL 回调（需公网可访问 URL）与**长连接 WebSocket**（`wss://openws.work.weixin.qq.com`，BotID + Secret，30 s 心跳），后者官方明确列出适用场景「无公网 IP / 内网部署」「无需处理消息加解密」。`/document/62155` 现导航到「使用长连接 → 接收消息」，原方案一边引用该文档一边断言"桌面端不具备条件"，自相矛盾 | 正文四处改为：本期因需企业管理员配置 + 单连接互斥 + 断线重连，未实现该通道；只保留本地关注信号；后续可评估。另在设置页「能力边界」卡片如实写明 |
| **F2** | §4.5 全节 | 小节编号写成 `5.1`–`5.4`，而第 5 节是「与现有平台的集成点」 | 章节编号冲突，属文档结构错误 | 改为 `4.5.1`–`4.5.4`；§4.5.4 去掉「需飞总确认」 |
| **F3** | §3.1 关键约束、§七 | 「任务栏自动隐藏时栏会重新置顶，需每帧 `WindowFromPoint` + `SetWindowPos(HWND_TOPMOST)`」「沿用 z 序/region 方案」 | 这两条来自 2026-11-04 收纳盒踩坑，但收纳盒是**靠 Injector 挂在 Explorer 任务栏进程内**的自绘按钮；信息岛是独立进程的 `always_on_top` 窗口，机制不同。`set_hit()` / `set_region()` 在 2026-11-05 重构后已成为死代码 | 降级为「待实机确认」，代码只在建窗时设一次属性；风险表改为单窗口模型说明 |
| **F4** | §3.1、§5 集成表 | 设置中心「侧栏 3 项」「新增侧栏第 4 项」 | 当前 `settings_app.rs` 已有 **4 页**：任务栏样式 / 桌面整理 / 文件检索 / 关于（`PAGE_*` = 0/1/2/3）。信息岛是第 **5** 页，`PAGE_ISLAND = 4` 这个取值本身是对的 | 全文改为 5 页；`NavIcon::Island`、`salt = 4`、`flush()`、`island_tick()` 均已补齐 |
| **F5** | §3.1 | 「`Alt+D` / `Alt+S` 已占用（`dcx-host/src/hotkey.rs:29-30`）」 | 第 29-30 行是 `BOX_HOTKEY_ID` / `SEARCH_HOTKEY_ID` 常量，**不是**快捷键默认值；默认值在 `box_hotkey_setting()` / `search_hotkey_setting()`（约 186 / 190 行），且键名是 `hotkey` / `hotkey_enabled` | 修正引用位置，并写明 `island.toml` 用顶层键（与 `search.toml` 的 `[activation]` 不同） |
| **F6** | §3.2 依赖策略 | 「P2 阶段按需引入 `windows` crate，最小 feature 为 5 项」「IMAP 客户端 + `ureq` + rustls」「不整体引入 `windows 0.62`」 | 实测 `windows 0.62` 在 P0 就要引入，且 `UserNotification::AppInfo()` 由 `ApplicationModel` feature 门控、`RoInitialize` 需要 `Win32_System_WinRT`；缺任一即编译失败。最终没有引入 IMAP / ureq crate | 改为实落地依赖说明，并注明 `windows-future 0.3` 的 `join()` 是 inherent method（`Async` trait 私有）、`windows-link` 用 raw-dylib 因此**不需要 SDK import lib / vcvars** |
| **F7** | §4.5、§七 | 只写「凭据存 Windows 凭据管理器」，没有写入口 | 原方案下 IMAP / webhook 渠道**永远无法配置**：`cred_write` 在代码里是死代码，没有任何 UI 或 CLI 能写凭据 | 新增共享 `dcx-island-config::credential`，宿主设置页提供 3 个掩码输入框 + 显式「保存 / 清除」按钮；目标名 `DCX/island/<key>` 一处定义、两端共用 |
| **F8** | §4.5.2 | 「设置页提供每渠道开关 + 诊断卡片……错误写 `island_notify.log`」 | 落地后：渠道开关在宿主设置页，权限/错误/上次成功时间在**岛内「消息」页**；日志统一 `island.log`（256 KB 截断轮转），没有 `island_notify.log` | 更正描述；设置页改放「能力边界」与凭据状态 |
| **F9** | §2.1、附录 | 「WinIsland 官网 `tanikaze.icu`」「852 commits」「fly-island MIT」 | README / 仓库页未核实到官网与 commit 数（且属易变快照）；fly-island 的 `LICENSE` 是 **2026-10-27 才加入**，此前版本许可状态不明确 | 官网与 commit 数移出事实断言；许可证加时点说明，强化「不复制任何源文件」的合规结论 |
| **F10** | §4.1 | 「`SYSTEMTIME` 结构在 `dcx-win32/src/lib.rs:69` 已定义，只需补 FFI 声明」 | 行号正确，但本项目 `SYSTEMTIME` 字段是 **snake_case** `hour/minute/second/milliseconds`，不是 Win32 SDK 的 `wHour`。本次实现按 SDK 习惯写 `wHour` → 4 个编译错误 | 文档补上字段名提示（§5 集成表），代码已改 |

### 9.2 优化项（O）

| # | 优化 | 理由 | 落地 |
|---|---|---|---|
| **O1** | **两个 viewport → 单窗口单 viewport**，窗口矩形随内容缩放 | 一个永远等于绘制区域的窗口，从结构上消除「大透明窗口吞桌面点击」；不需要 `HTTRANSPARENT`（不跨进程转发）、不需要窗口区域（DWM 合成留残影）、不需要 `set_hit()` 死代码 | `app.rs::compute_geo` + `win.rs`；`native_options` 里 `with_position(-32000,-32000)` 避免启动闪屏 |
| **O2** | 配置模型拆成共享 `dcx-island-config` crate | 宿主设置窗口与信息岛进程都要读写 `island.toml` 和凭据；让 host 依赖 GUI crate 会拖进第二份 eframe | 已拆；`host/Cargo.toml` 只依赖共享 crate |
| **O3** | QQ 权限用同步 `GetAccessStatus()` 而非 `RequestAccessAsync().get()` | 后者可能弹系统授权框，无人值守桌面会把渠道线程永久挂住；fly-island 源码用的也是同步读 | `notify/qq.rs`；另加 `RoApartment` RAII，在每个退出路径 `RoUninitialize()` |
| **O4** | 热键双开关校验 | 常驻 topmost 窗口被用户关掉后，不应被一个热键复活。文件检索可以按需拉起，信息岛不行 | `hotkey.rs::fire` 要求 `island.toml enabled && apps.toml [apps.island].enabled && hotkey_enabled` |
| **O5** | 宿主保存 `island.toml` 时合并 `ui` 表 | 应用自己会写 `last_tab` / `panel_width` / `panel_height`；宿主全量写回会回滚刚拖过的面板尺寸 | `settings_app.rs::persist_island` 用 `Config { ui: disk.ui, ..self.island }` |
| **O6** | 出站走手写 WinHTTP + SChannel | 不引入 ureq / rustls / OpenSSL，构建依赖更小、证书用系统 store | `sys.rs::https_post` + `native-tls` |
| **O7** | 全量测试补齐 | 初稿的 `cargo test --workspace` 有 1 处编译错误 + 2 处断言写错：DIB 是 bottom-up（首行是最后一条扫描线）、`CpuSample.busy` 已是非 idle 部分（`busy/(idle+busy)`） | 已修 3 处；全量 **185 项测试通过** |
| **O8** | 补齐 `stop-dcx.bat` / `start-dcx.bat` 并纳入版本控制 | 分发目录里的旧 `stop-dcx.bat` 只 `taskkill` host / box / search；强杀宿主不走 `apps::stop_all()`，信息岛会变成孤儿进程。且这两个脚本原先只存在于 `D:\dcx-dist`，不在仓库里，下次打包必然再漏 | 两个脚本补进 `tools/` 并同步分发目录；`start-dcx.bat` 注释里的 `Alt+Space` 更正为 `Alt+S` |

### 9.3 落地范围

| 层次 | 状态 |
|---|---|
| 新 crate `dcx-island-config` / `dcx-island`（约 8.5k 行） | 已完成；编译错误 30 处全部修复 |
| 宿主集成：`apps.rs` 注册、`hotkey.rs` 第三条热键、`settings_app.rs` 第 5 页（6 张卡片 + 凭据入口）、`host/Cargo.toml` | 已完成 |
| 时间 / CPU / 内存 / 磁盘 / 网络、任务与提醒、图文剪贴板、四 tab 面板、统一收件箱、QQ/微信/企业微信/IMAP 入站、企业微信 webhook / SMTP 出站 | 已完成代码；**GUI / 通知渠道需实机验证** |
| `stop-dcx-all.bat`、`stop-dcx.bat`、`start-dcx.bat`、`README.md`、`docs/engineering-notes.md`、本文 | 已更新；两个启停脚本首次纳入 `tools/`（O8） |
| `D:\dcx-dist` 产物与 `SHA256SUMS.txt` | **已部署**（2026-11-07 16:08）：`dcx-host.exe` 先 `mv` 为 `.bak-20261107-1607` 再换入，`dcx-island.exe` 新增；`dcx-box.exe` / `dcx-search.exe` / `dcx_engine.dll` 未动（引擎线协议未变、且被 Explorer 映射）。`stop-dcx*.bat`、`start-dcx.bat`、`说明.txt` 同步，`SHA256SUMS.txt` 重算后 `sha256sum -c` 五项全 OK。**未重启任何进程**：运行中的宿主仍是旧代码，需托盘退出后重开（或重启系统）才生效 |

### 9.4 验证结果与遗留风险

- `cargo build --workspace`：**通过**。`cargo test --workspace`：**通过**（含 `dcx-island` 46 项、`dcx-island-config` 4 项；全量 185 通过 / 2 ignored）。
- Release 产物：`dcx-island.exe` `c658c1fe…`、`dcx-host.exe` `da86bcc2…`；分发目录 `sha256sum -c SHA256SUMS.txt` 五项全 OK；图标资源 `brand/island.ico`（13,305 B）与 `brand/embed/island-64.rgba`（16,384 B）齐备。
- 部署时**没有重启**正在运行的 `dcx-host.exe`（PID 21388）/ `dcx-box.exe`（PID 21444）：沙箱内启动 GUI 宿主会被作业对象回收并导致任务栏消失，故只替换文件。生产环境生效需由用户在桌面会话里手工重启宿主。
- 编译期修掉的坑：`egui 0.29` 无 `StrokeKind`（`rect_stroke` 只收 3 个参数）、`Context::interact` 不存在（改用 `Ui::interact` 并提前取 `Response`）、`SYSTEMTIME` 字段名、`windows` feature 门控、`combase.lib` 链接（改用 `windows-link` raw-dylib）、`Stroke::new` 浮点字面量的 future-incompat 告警。
- **未经实机验证**，按本项目既有约定「编译通过 ≠ 能用」，上线前仍需：① 125% DPI 下点击与绘制对齐；② 自动隐藏任务栏滑出时胶囊是否被遮挡（F3 待确认项）；③ 紧凑态窗口外 `WM_NCHITTEST = -1`；④ 四角无黑边；⑤ 只 kill `dcx-island.exe` 后 host 看门狗 5 s 内拉起；⑥ 企业微信是否真的触发 `HSHELL_FLASH`；⑦ 凭据保存后 IMAP / webhook 实发一次。
- 企业微信智能机器人长连接（F1 揭示的可行通道）**仍属未实现能力**，若后续要做，需独立评估 BotID/Secret 的存放、单连接互斥、30 s 心跳与断线重连，不能简单复用现有 SHELLHOOK 通道。

### 9.5 第二轮修复与实机验证（2026-11-07 17:20）

上一轮只做到「编译通过」，本轮按代码审查结论逐条修，并在本机（125% DPI、双屏上下排列、任务栏自动隐藏、主屏为下方屏）用隔离的 `LOCALAPPDATA` 跑起真进程截图验证。**没有动生产配置**（`%LOCALAPPDATA%\dcx\island.toml` 至今不存在，测试写的是 `D:\tmp\island-smoke`）。

#### 修掉的问题

| # | 症状 | 根因 | 修法 |
|---|---|---|---|
| 1 | 面板尺寸与上次页签重启必回默认 | `app.rs::set_open` 里 `if self.cfg.ui.last_tab != self.tab.key()` 恒假（`Action::Tab` 已把内存值改成同一个），全仓只有这一处 `dcx_island_config::save` | 新增 `dcx-island-config::patch_ui`（**锁内**读-改-写，只改 `[ui]`）；`app.rs::persist_ui` 在切页签 / 拖动结束 / 收起时调用，写前与 `ui_saved` 比对。顺带解决「关面板整份回写、回滚宿主 900 ms 内刚改的设置」 |
| 2 | 默认胶囊看不到 CPU / 内存 | `ui/compact.rs` 按 `rect.width() >= 250` 硬门槛画指标，而 `capsule_width` 默认 232 | 门槛改成「时钟已占空间 + 给任务标题预留 70 pt」逐个裁决；默认宽度 232 → **300**（网速读数改为最后降级项，需 ≥350 才出现） |
| 3 | 剪贴板 / 收件箱列表看不到第 8 条以后 | 两处都是 `y + row_h > bottom` 就 `break`，全文件无滚动 | `panel.rs::scroll()`（`raw_scroll_delta`）+ `scroll_bar()`；`ui.set_clip_rect(list)` 同时约束交互矩形（egui 0.29 `ui.rs:1046` 按 clip 截断 `interact_rect`），半行不会抢搜索框的点击 |
| 4 | 任务栏滑出后可永久压住胶囊 | `win.rs::ensure_raised` 从未被调用，`place()` rect 未变时提前返回 | 每 250 ms `WindowFromPoint(窗口中心)`，被遮挡才 `HWND_TOPMOST`（无遮挡时零 z 序写入） |
| 5 | 展开首帧被裁 | `update` 里 `draw` 早于 `place` | `place` 前移，按新尺寸建面再画 |
| 6 | `Alt+I` 最慢近 1 s 才响应 | 折叠态 `request_repaint_after(到下一整秒)`，而标记只在该帧读 | 折叠态间隔上限压到 `MARKER_POLL`(220 ms) |
| 7 | 提醒抢焦点 | `on_tick` 直接 `set_open(true)` → `ViewportCommand::Focus` | `set_open_with(activate=false)` + `passive` 标志：不夺焦、失焦不自动关、12 s 未交互自动收起 |
| 8 | 宿主停止 = 硬杀，丢最后 900 ms 改动 | `apps.rs::stop` 先 `child.kill()` | 先 `WM_CLOSE` → 等 1.2 s → 再 kill；`stop_all` 先对所有应用统一请求关闭，再一次性等待（避免 N × 1.2 s） |
| 9 | `island.toml` 关、`apps.toml` 开 → 每 5 s 无限拉起 | `launch()` 返回 `Ok` 即 `retry.remove()`，进程秒退也当成功，`MAX_FAILURES` 永不生效 | 记录 `spawned_at`，下个 tick 仍不在跑即计一次失败并退避（10/20/40/80/160 s → 600 s） |
| 10 | **最大化窗口会让胶囊消失** | `foreground_is_fullscreen` 的矩形启发式：任务栏自动隐藏时，最大化窗口矩形正好等于显示器矩形 | 主判据换成 `SHQueryUserNotificationState`（`QUNS_BUSY`/`D3D_FULL_SCREEN`/`PRESENTATION`/`APP` 才泊走），矩形判定降为兜底 |

顺带修的：`window_px` 被当窗口宽高用（副屏在左时为负）、`win::snap` 接入 `compute_geo`（像素对齐）、`metrics.style` 条/环真的生效（新增 `bar_card`）、`metrics.disk` / `clock.show` 接线、采样失败恢复后清 `error`、剪贴板自写去重标记改 `take()`（否则用户下一次复制可能被当成自写丢弃）。编译告警 36 → 16，剩余全部是未接线的诊断素材（`readiness`、`net_interface_names`、`clipboard_owner`、未用 `Glyph`/控件、`Dispatcher`），不是垃圾但确实还没接。

#### 实机证据（隔离 `LOCALAPPDATA`，日志 `island.log`）

- 窗口：`window ready hwnd=592058`、`scale 1.25`；`GetWindowRect` 收起态 = `(773,10)-(1148,53)`，宽 375 = 300 × 1.25，位于主屏顶部居中。
- 紧凑态：胶囊显示 `17:17 周六 ▁29% ▁26%`，指标随采样变化（29→40→43%）。
- 展开：面板在同一窗口内向下展开，页签 **任务 / 剪贴板 / 消息 / 性能**、空态文案、页脚、右下角拖拽把手齐全，四角无黑边（DWM 1 px 边框修复沿用上一轮）。**（2026-11-07 18:20 更正：此结论有误。截图确有一圈深色框，只是当时的裁切与缩放下没辨认出来，真因与修法见 §9.6。）**
- 滚动：造 14 条剪贴板数据，未滚动显示 `01–07` 且右缘出现细滚动条；滚轮 3 档后显示 `04–10`、滑块下移、悬停行出现删除按钮。
- 配置落盘：点「性能」页签后 `island.toml` 出现 `last_tab = "perf"`，`panel_width = 388.0` / `panel_height = 436.0` 一并写入，`enabled = true` 等其它表原样保留（`patch_ui` 的读-改-写生效），日志无 `ui save failed`。
- 全屏判定：当前前台为最大化窗口时 `SHQueryUserNotificationState` 返回 **5（ACCEPTS_NOTIFICATIONS）**，胶囊正常显示；旧矩形判定在同一几何下会判成全屏。

#### 本轮仍未验证

- 宿主两处改动（优雅停止、看门狗退避）**要重启 `dcx-host.exe` 才生效**，本轮只替换了文件，未重启进程（沙箱内起宿主会被作业回收并连带任务栏）。
- 提醒到点（`passive` 打开 + 12 s 自动收起）需一条到点任务实测。
- 任务栏自动隐藏滑出时的 z 序纠正未撞上遮挡（日志无 `z-order: covered`，说明本轮没触发，也就没验证到纠正动作）。
- `ui.monitor`（多屏固定）与 `clipboard.skip_password_fields`（密码框不记录）仍未接线，前者属功能缺口、后者涉及隐私承诺（设置页已注明「尽力而为，不是安全保证」），下一批优先处理 `skip_password_fields`。
- 企业微信 `HSHELL_FLASH`、IMAP / webhook 实发沿用上一轮结论：未验证。
- 部署：`D:\dcx-dist\dcx-host.exe`（`a5f2612d…`，旧文件备份为 `.bak-20261107-1720`）、`dcx-island.exe`（`0c695274…`）已换入，`SHA256SUMS.txt` 已重算。

### 9.6 胶囊/面板四周黑框：eframe 的默认 `clear_color`（2026-11-07 18:20）

**现象**：胶囊两侧圆角外出现深色月牙，展开后胶囊与面板之间是一条横贯窗口的深色带，面板左右两侧也有一条与窗口同高的深色竖带。用户截图（146 与 488 px 宽）里就是"四周一个黑框"。

**根因（确定性，不是 DWM、不是阴影）**：`dcx-island` 没有重写 `eframe::App::clear_color`。eframe 0.29 的默认实现是

```rust
// eframe-0.29/src/epi.rs:193
egui::Color32::from_rgba_unmultiplied(12, 12, 12, 180).to_normalized_gamma_f32()
```

即每帧先用 **70.6 % 的近黑** 清整块窗口矩形。信息岛只在窗口里画了胶囊和面板两个圆角矩形，其余像素（窗口四角、`GAP` 8 pt 的横带、面板比胶囊宽出来的两条竖带）就是这个清屏色，于是看起来是一圈黑框。

**像素对账**：桌面壁纸 (159,197,208)，按预乘合成 `0.294 × 壁纸 + 0.706 × 12` = (55,66,70)，实测 (53,63,68)。换一张壁纸、换 125 %/100 % 都是同一个公式，与 DWM 的 1 px 边框（`set_border_color(DWMWA_COLOR_NONE)` 早已关掉）在宽度上差一个数量级。

**修法**：`app.rs` 的 `impl eframe::App for Island` 内加

```rust
fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
    egui::Rgba::TRANSPARENT.to_array()
}
```

与 `dcx-box`（`crates/dcx-box/src/app.rs:1379`）保持一致。`with_transparent(true)` 一直是开着的，缺的就是这一步。

**排查手法（留档，因为第一次差点判错）**：临时在 `draw` 的最前面画一整块**品红**全窗口矩形重建一次，发现月牙区域是纯品红 `(249,0,249)`，说明那里没有任何图元在画 —— 排除了"阴影描边被画成实心"和"DWM 圆角"两种猜测，锁定为清屏色。另有一处**误判**要记下：第一次隔离测试时截到的深色月牙其实来自**宿主看门狗重新拉起的旧 exe**（`D:/dcx-dist` 旧构建，生产配置 `capsule_width = 272` → 341 px），与我起的测试实例窗口叠在同一位置；用 `GetWindowRect` 只量到测试实例，所以对不上账。以后对比截图先确认"屏幕上只有一个岛"。

**实机验证**（生产实例，125 % DPI，双屏上下排列）：部署后由看门狗拉起，`GetWindowRect` 收起态 `(790,10)-(1130,50)`，截图胶囊外侧为壁纸色；展开态 `(741,10)-(1180,444)`，面板左右外侧采样 `(225,213,213)`/`(255,255,255)`，胶囊与面板间横带采样 `(193…200)`，均为壁纸色，无深色带。深色像素仅剩文字字形。

**部署**：`D:/dcx-dist/dcx-island.exe` → `b98f4d65…`（旧文件备份 `.bak-20261107-1820`），`SHA256SUMS.txt` 已重算。

### 9.7 用户复检后的五条实机问题（2026-11-07 19:10）

§9.6 的结论只对了一半：黑框（清屏色）确实修掉了，但用户复检时仍然看到"四边有透明的背景"。这一轮按用户逐条反馈处理，五条全部实机验证。

**① 仍能看到浅色方框（§9.6 结论的补充与修正）**

§9.6 说"面板左右外侧与中间横带均为壁纸色，无深色带"——这是对的，但它只证明了"那里没有画深色"，没有证明"那里看起来和面板是一体的"。实际残留两个问题：

- `ui/theme.rs` 两个调色板的 `surface` alpha 是 `0xF2`（0.949），面板本体合成出来是 `0.949 × surface + 0.051 × 壁纸`。纯色壁纸上察觉不到，渐变壁纸上就是一圈比壁纸略亮的边——这就是用户说的"透明的背景"。
- 窗口矩形 = 胶囊 ∪ 面板的并集。展开后窗口比面板宽，左侧/右侧各有一条**未绘制**的竖带，中间还有 `GAP` 8 pt 的横带。这些像素是透明的（壁纸），而面板本体是 94.9 % 不透明的，两者亮度差 1~2 级，边界处就形成"方框"观感。

修法两条，缺一不可：

- `surface` 改为**完全不透明**（`0xFA/0xFF` 亮色、`0x1C/0xFF` 暗色）。
- 展开时整个窗口画**一张卡**：`app.rs::draw` 里 `card = Rect::from_min_max(Pos2::ZERO, rect.max)`，先 `shadow` 再 `round_rect`；`panel::draw` 增加 `merged: bool`，为 `true` 时跳过自身的 shadow 与 round_rect。这样窗口内不再存在"未绘制"的带。

**验证**：黑底背景板（`STATIC` + `SS_BLACKRECT` 全屏）下截图，`缝隙行 y=54` 全宽为 `(249,249,251)`，只有最外 1 px 是抗锯齿；正常壁纸下 `GetWindowRect` 收集/展开两种状态的窗口矩形内均为统一卡片色。

**② 点击胶囊闪动晃屏**

根因不是重绘，是**窗口几何一次性跳变**：展开动画只让面板内容淡入，窗口却在前一帧还是胶囊尺寸、后一帧直接变成面板尺寸（约 500 px 的 `SetWindowPos`），DWM 要重新合成整块 layered surface，视觉上是一次闪白。

修法：`compute_geo` 让窗口尺寸跟着同一个 easing 一起长——

```rust
let e = self.anim.clamp(0.0, 1.0);
let win_w = (capsule_w + (panel_w - capsule_w) * e).max(capsule_w);
let win_h = capsule_h + (GAP * scale + panel_h) * e;
```

面板矩形在 `e > 0.02` 时就开始提供（保持最终几何，由窗口裁剪），并在 `update()` 里于 `compute_geo` 之前调用新的 `advance_anim()`（原来 `draw` 推进动画，晚一帧），`schedule_repaint()` 在展开中按 16 ms 重绘。

**验证**：逐帧采样窗口矩形，约 14 帧、220 ms 从 `(790,10,1130,50)` 长到 `(760,10,1160,562)`，不再有单帧跳变。

**③ 全屏/最大化不自隐藏**

用户选择"最大化和全屏都隐藏"。`win.rs::foreground_is_fullscreen` 改为三级判定：

1. `SHQueryUserNotificationState` 为 2/3/4/7（BUSY、D3D_FULL_SCREEN、PRESENTATION、APP）→ 立即隐藏；
2. 前台窗口矩形覆盖整个显示器 → 隐藏；
3. `IsZoomed(fg)` 且前台窗口覆盖胶囊所在的横条 → 隐藏。

原来 `if let Some(park) = shell_says_no_popups() { return park; }` 会让 QUNS=5（ACCEPTS_NOTIFICATIONS）直接返回 `false`，最大化窗口永远不隐藏——改成只对 `Some(true)` 短路。

另外两个坑：

- 泊到 `(-32000,-32000)` 后 `MonitorFromWindow` 返回"最近的显示器"，双屏上下排列时会在主/副屏之间翻转，导致 `fullscreen` 每秒抖动。`Island` 新增 `mon: win::Monitor` 字段，只在未泊起时刷新。
- `housekeeping` 里 DWM 边框/圆角重修的周期由 2 s 放宽到 10 s，减少每帧 z-order 与 `SetWindowPos` 开销。

**验证**：程序创建真实全屏窗口（`MonitorFromPoint` + `GetMonitorInfoW` 取 `rcMonitor`，不是 `SPI_GETWORKAREA`）→ 岛移到 `(-32000,*)`，关闭 → 回到 `(790,10,1130,50)`；最大化窗口 `rect -9,-9,1929,1029 zoomed 1` → 同样泊起，还原后回落。

**④ 消息行错位、点击不能打开应用**

排版：行高原来是固定 40（有详情 48），详情行画在 `row.bottom() - 5`。125 % DPI 下 12 pt 标题实际需要约 17 pt，而 `title_rect` 只给了 `bottom - 17` 中的 11 pt，于是**标题框与详情行重叠 1 px**，三行字挤成一团。改为全部由字体实测值推导：

```rust
let lh_small = ui.fonts(|f| f.row_height(&FontId::proportional(9.5))).max(12.0);
let lh_title = ui.fonts(|f| f.row_height(&FontId::proportional(12.0))).max(15.0);
// row_h = PAD_TOP + lh_small + lh_title [+ lh_small] + PAD_BOT，下限 40
```

来源行与时间按第一行中线对齐（`RIGHT_CENTER`），未读点也在第一行中线；标题与详情各占一行的实测高度，用新增的 `fit_one_line()` 按**像素宽二分**截断加省略号（12 pt 中文宽约是拉丁的两倍，按字数截断在两种文本上都不对）。

唤起：新增 `crates/dcx-island/src/activate.rs`，`EnumWindows` + 进程名匹配（`weixin.exe`/`wechat.exe`/`wxwork.exe`/`qq.exe`/`qqnt.exe`/`tim.exe`），取面积最大的"可见、无 owner、有标题"窗口；`IsIconic` 时先 `ShowWindowAsync(SW_RESTORE)`，再 `BringWindowToTop` + `SetForegroundWindow`，全过程写 `island.log`。面板侧新增 `Action::OpenInbox(id, source)`：`mark_read` → `activate`，成功才收起面板，失败弹 toast。

**验证**：用临时调试钩子灌入 6 条假消息（微信/企业微信/QQ/邮箱，含/不含详情各半），截图确认三行不再重叠、长标题按宽度省略；点击"微信 / 有新消息"行后前台窗口从岛 `21430658` 变为微信 `67584`（标题"微信"），面板自动收起。调试钩子验证完即删，未留在产物里。

**⑤ 剪贴板内容看不完整**

行高原来固定 40/46，第三条及以后的行被切一半。改为按 galley 实测行数：`clip_row_h()` 用 `galley.size().y / row_height` 取整并 clamp 到 1~3 行，行高 `(lines * line + 8).max(34)`，图片固定 52；文本改为**顶对齐**绘制（原来居中，首行被切一半），溢出部分落到裁剪区外；整行 hover 显示全文（文本截 400 字）。

**验证**：灌入 10 条长度不一的记录（1 行 / 2 行 / 3 行 / 超长 URL / SQL / 多行），截图确认每条都完整显示；滚到底部可到第 10 条，滚动条行程与内容高度一致。

**另外记一条不是 bug 的**：测试过程中面板尺寸被改小（388×436 → 364.8×372），一度被我当成 `handle_resize` 的残留缺陷。用户确认是**他自己拖的**。同期仍加了两道护栏，因为它们的价值独立存在：`handle_resize` 在 `!self.open || self.anim < 0.99` 时直接返回（窗口还在长的 200 ms 里，一个起始于页签的按压会产生一次巨大的 `drag_delta`，足以把面板一次性压到 320×260），且单帧位移 clamp ±60 pt、改用 `dragged_by(Primary)`。

**验证**：连点胶囊开合 6 轮、每轮再点 3 个页签，窗口稳定在 `485 × 595`（388×436 @125 %）不变；拖右下角仍可放大到 `603 × 655`、缩小到 `525 × 615`。

**部署**：`D:/dcx-dist/dcx-island.exe` → `270C14B0…`，`SHA256SUMS.txt` 已重算；旧文件备份 `.bak-20261107-1820`、`.bak-20261107-1900`。

### 9.8 胶囊与面板合并：一行标题栏 + 关闭按钮落到右上角（2026-11-07 19:30）

用户复检后提了两点：「岛展开后要与面板融为一体，现在太分裂了」、「岛上内容以动画方式向面板铺开，关闭按钮在最右上角」。合并前的问题是**三层结构**：胶囊自己是一颗带边框的胶囊 → 中间 8 pt 透明缝 → 面板自己又是一张带边框的卡，面板里还有第二行头部（又写了一遍日期 + 待办数 + 两个按钮）。同一个日期出现两次，两条外框并排，看起来当然是两个东西。

**合并后的结构**：整个窗口就是一张卡。

- `compute_geo`：`gap = GAP * (1 - e)` 随展开收到 0；窗口高度改成 `capsule_h + panel_h * e`——不含 gap，所以收起时窗口严格等于胶囊（340×40 @125 %），展开后 `capsule_h + panel_h`。面板矩形（现在是卡片的**身体**）顶边跟着 gap 从 `capsule_h + 8` 落到 `capsule_h`。
- `app.rs::draw`：卡片从展开动画的第一帧起就画在**整个窗口**上，圆角半径随动画从 `capsule_h / 2`（胶囊）插值到 14（面板）。胶囊外壳只在 `anim < 0.02` 时由 `compact::draw` 画（`CapsuleOpts::shell`）——两个不透明表面同色同位置，交叉的那几帧看不出接缝，于是"胶囊变成卡片顶栏"是连续的一步，不是两个物体叠在一起。
- 面板不再有 `draw_header`。头部的日期、待办数、导出、关闭四样全部搬进胶囊那一行；`panel::draw` 从"卡片 + 头部 + 页签"变成"页签 + 内容 + 页脚"，`header_h = 38` 整行消失，内容区因此高 38 pt。
- 现成的"待办数"落到「任务」页签标签上（`任务 3`），与「消息」的未读数同一套写法。`draw_tabs` 里两行逻辑，不占新位置。

**"内容向面板铺开"的实现**：不是让文字飞，而是**让这一行的左右边界走**。`app.rs` 里

```rust
let cap_pad = (cap_rect.height() * 0.34).clamp(9.0, 14.0);
let lerp = |a: f32, b: f32| a + (b - a) * anim;
let row = Rect::from_min_max(
    Pos2::new(lerp(cap_rect.left() + cap_pad, full.left() + 16.0), cap_rect.top()),
    Pos2::new(lerp(cap_rect.right() - cap_pad, full.right() - 16.0), cap_rect.bottom()),
);
```

`compact::draw` 的排版本来就是"从左往右排时钟、从右往左排指标"（`left` / `right` 两个游标），所以只要喂它一个正在变宽的矩形，时钟自动向左滑、指标自动向右滑，中间的任务标题跟着撑开——这就是用户要的铺开，而且没有第二套布局代码。`CapsuleOpts::pad` 因此置 0，内缩交给调用方插值。

**关闭按钮**：`compact::draw` 的右侧游标先摆 ✕（`corner_button`，22 pt，离卡片右边 16 pt、垂直居中于顶栏），再摆导出图标，剩下的宽度才轮到指标/网速/任务标题。整条右侧簇本来就按 `right` 游标"放得下才放"，所以按钮变多时指标自然让位，不需要额外判断。✕ 与导出的字形用 `with_alpha(base, opts.corner)` 淡入，`corner = anim`，与铺开同一条曲线。

**踩到的坑（一改就中）**：`ui.allocate_rect(rect, Sense::click())` 原来在 `draw` 的**最后**，那时行内没有按钮，无碍；加了 ✕ 之后它成了同一图层里最后注册的控件，egui 把点击给最后注册的那个，于是整行的点击区把 ✕ 的点击吃掉了——点 ✕ 没反应。修法是把整行 `allocate_rect` 挪到 `draw` 的**最前面**，把 ✕/导出压在它上面；并在返回值上补 `&& !hit.close && !hit.archive`，避免一次点击同时算作"点整行"。

**交互规则也跟着改**：收起时整颗胶囊就是"打开"按钮；展开后这一行属于面板，点它不再收起（点标题栏不该关窗口），只有 ✕ 收起。失焦自动收起、Esc、热键都不变。

**实机验证**（生产实例，125 % DPI）：

- 收起 `340 × 40`；展开 `485 × 585`（388×436 + 顶栏 32 pt）。
- 展开过程逐帧采样：`340×40 → 386×214 → 439×414 → 464×506 → 475×548 → 480×568 → 483×577 → 485×585`，约 250 ms 单调增长，无跳变；抽帧拼图确认时钟向左滑、指标向右滑、卡片同步长大。
- 点 ✕ → `340×40`；点标题栏中部 → 尺寸不变（不收起）；点页签 → 正常切换；点桌面空白 → 失焦收起。
- 导出图标点击不崩溃（本机无待办，走的是空列表分支）。
