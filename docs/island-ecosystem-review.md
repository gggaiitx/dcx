# 灵动岛 / 信息岛生态盘点与功能接入点评估

> 2026-11-07 · 调研目的：在 `dcx-island` 已完成「时间 / 性能 / 任务提醒 / 剪贴板 / 四路消息聚合」之后，
> 回答「还有哪些**值得接**的能力」以及「怎么接」。
> 本文只做判断与方案，不含代码改动。所有机制结论标注来源；未核实项显式标注「待实机确认」。

---

## 一、结论先行

1. **这个品类已经收敛出五件「最低门槛」**：媒体会话、通知历史、剪贴板、待办/番茄钟、系统监控。
   `dcx-island` 已有后三件中的剪贴板、待办、监控，缺**媒体会话**与**通知历史**。这两项不是差异化，是补齐。
2. **真正有杠杆的不是「再加一个功能」，而是开一个本地推送 API**。WinIslands 用一套
   `POST /v1/island/push` + `GET /v3/ws` 让「任何东西都能上岛」（官网与 README 均列出，默认端口 **9840**），
   它的 30 多个组件没有一个比这套接口更值钱。理由见 §5。
3. **DCX 的独占优势不在岛本身，在它挂着三样别人没有的东西**：Explorer 注入引擎（`dcx_engine.dll` + XAML 量测）、
   文件索引（`dcx-search`）、收纳库（`dcx-organizer`）。最高价值的新功能是**把这三样的入口搬到岛上**，
   而不是去追 macOS 那边的歌词、摄像头镜像、颜色拾取器。理由见 §4.2。
4. **有一条又便宜又贴合当前工作流的能力被所有同类项目忽略了**：长任务进度上岛
   （构建、文档生成、AI 任务）。它就是 §5 那套 API 的第一个消费者，也是验证这套 API 是否够用的最佳用例。
5. **两个能力明确不该做**：逐字歌词（与 egui 渲染栈、常驻 CPU 预算冲突，且非工作场景）、
   手机通知同步（需要手机端 App 与配对协议，投入产出比在本项目里不成立）。
6. **一个安全缺口要提前定规则**：本地推送 API 如果不带 token，本机任何进程都能往 topmost 窗口写任意文案
   （伪造提醒、钓鱼）。见 §5.3。

---

## 二、生态盘点

### 2.1 Windows 原生系（与 DCX 同赛道，可直接对照）

| 项目 | 许可 / 栈 | 关键能力 | 对 DCX 的参考价值 |
|---|---|---|---|
| **WinIsland**（WinIslandProject） | GPL-3.0 / Rust + D3D12 | 媒体与歌词、液态玻璃、CPU/内存/磁盘/网络/GPU 小组件、计时器、通知镜像、插件市场、灵言 AI | 已分析过（`island-app-plan.md`）。**GPL 不可复制**，只借交互与取数口径 |
| **fly-island**（flyl1u） | MIT / Tauri 2 + React 19 | 待办与每日笔记、图文剪贴板、QQ/微信/QQ 邮箱提醒、快捷翻译、AI 状态灯、媒体控制 | 已分析过。通知渠道的**能力边界**写得最诚实（微信只读任务栏关注信号） |
| **WinIslands**（JudeKwong） | **MIT** / .NET 8 + WPF | SMTC 媒体控制 + 逐字歌词（LRC / AMLL TTML）、30+ 组件、通知历史、**本地 HTTP/WebSocket 上岛 API**、番茄钟、快捷启动器、文件复制与下载进度上岛、**会议静音助手**、规则引擎、录屏自动勿扰 | **本次最重要的发现**。接口设计可直接对标，MIT 允许参考实现思路 |
| **Dynamic Island for Windows**（Drey0911） | MIT / C# | NAudio 音频频谱、LibreHardwareMonitor（CPU/GPU 温度）、PresentMon FPS、**QRCoder 二维码传文件到手机**、耳机电池（HeadsetControl / Logitech HID++ / DualSense）、麦克风静音、亮度、翻译、剪贴板、自定义 API | 硬件传感器的取数路线（LibreHardwareMonitor 是 MPL-2.0，可单独引用）；二维码互传是低成本跨设备方案 |
| **WinLand**（luolangaga） | WinUI 3 / .NET 10 | 插件化胶囊、拖拽动作（文件/文件夹/文本/图片）、插件市场 | 插件系统已明确排除，只借鉴「拖拽载荷分发」的模型 |

### 2.2 macOS 参照系（不计许可证，只看产品形态）

| 产品 | 形态特点 | 可迁移的判断 |
|---|---|---|
| Boring Notch（GPL-3.0，~11k★） | 音乐 + 可视化 + 文件架（AirDrop）+ 日历 + 系统 HUD 替换 | 「文件架」是 macOS 特色，但**等价物在 Windows 上就是拖拽暂存**，DCX 有收纳盒可承接 |
| Atoll（GPL-3.0，~4.8k★） | 实时活动（充电/Focus/录屏/下载）、锁屏小组件、CPU/GPU/内存/网络/磁盘、计时器、剪贴板、**颜色拾取器** | 「实时活动」是一个值得学的抽象：**有开始-有结束、有进度的事件** |
| NotchNook（$15 或 $2/月） | 音乐、日历（**Meet/Zoom/Teams/Webex 一键入会**）、笔记、待办、计时器、摄像头镜像、文件 Tray | 「日历 + 一键入会」是付费产品里被验证的核心卖点，Windows 侧**没有对等实现** |
| Droppy（$9.99） | 文件架为核心 + 剪贴板（含 OCR）+ **Claude Code / Codex / Cursor 会话与用量追踪** + 44 个扩展 | AI 编程会话状态已被单列为产品能力，与本地长任务进度同源 |
| DynamicLake | iPhone 式通知、来电、iMessage/WhatsApp/Telegram/Slack 通知、蓝牙设备连接提醒 | 通知聚合的深度依赖平台消息 API，Windows 侧拿不到等价物 |

### 2.3 跨设备（评估后建议后置）

| 项目 | 许可 | 能力 | 结论 |
|---|---|---|---|
| KDE Connect | GPL v2+ | 通知同步、**共享剪贴板双向**、文件传输、媒体控制、手机当触控板、SMS、找手机、演示遥控 | 能力最全，但要两端装 App + 配对；GPL 只能参考协议思路 |
| LocalSend | 开源（MIT 类） | 局域网 HTTPS 直传，无账号无服务器 | **只做「收」的一半成本很低**：本机起一个 HTTPS 监听 + 岛内显示二维码，手机扫码即传。与 Dynamic Island for Windows 的 QRCoder 方案同构 |
| Sefirah | 开源 | Windows↔Android 剪贴板/通知/媒体/文件/投屏/SMS | 需注意其文档记录 Android 15 敏感通知受限、传输实验性 |

---

## 三、能力矩阵：哪些是门槛，哪些是差异

| 能力 | WinIsland | fly-island | WinIslands | Dynamic Island | **dcx-island 现状** | 判断 |
|---|---|---|---|---|---|---|
| 时钟 / 日期 | ✅ | — | ✅ | ✅ | ✅ | 已有 |
| CPU / 内存 / 磁盘 / 网络 | ✅ | — | ✅ | ✅ | ✅ | 已有（GPU 未做，见 §6） |
| 媒体会话（SMTC） | ✅ | ✅ | ✅ | ✅ | ❌ | **门槛，建议补** |
| 通知历史 / 镜像 | ✅ | 部分 | ✅ | ✅ | 部分（QQ 单渠道） | **门槛，建议补** |
| 剪贴板历史 | — | ✅ | ✅ | ✅ | ✅ | 已有 |
| 待办 / 提醒 | ✅ 计时器 | ✅ | ✅ | ✅ | ✅（含间隔任务） | 已有，且规则模型更细 |
| 番茄钟 / 专注 | — | — | ✅ | — | ❌ | 低成本，可选 |
| 快捷启动器 | — | — | ✅ | — | ❌ | 低成本，可选 |
| 本地推送 API | 插件（GPL） | — | ✅ | ✅ 自定义 API | ❌ | **最高杠杆，见 §5** |
| 逐字歌词 | ✅（GPL） | — | ✅ | ✅ | ❌ | 不做（§6） |
| 系统 HUD 替换（音量/亮度） | — | — | ✅ | ✅ | ❌ | 中成本，可选 |
| 硬温度 / FPS | ✅ GPU | — | — | ✅ 温度/FPS | ❌ | 不做（§6） |
| 拖拽暂存 | — | — | ✅ 中转站 | ✅ | ❌ | 与收纳盒打通才有意义（§4.2） |
| 手机互传 / 通知同步 | — | — | — | 二维码 | ❌ | 互传后置；通知同步不做 |
| 日历 + 一键入会 | — | — | — | — | ❌ | 价值明确但取数成本待确认（§4.3） |

---

## 四、功能接入点（核心）

分类依据是**数据源可得性**与**与 DCX 现有能力的咬合度**，不是功能好不好看。

### 4.1 A 类：系统事件源（数据源在 Windows 内，成本可估）

| # | 接入点 | 数据源 | 成本 | 价值 | 风险 |
|---|---|---|---|---|---|
| A1 | **媒体会话**（播放/暂停/切歌、标题、封面、进度） | WinRT `Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager`（WinIslands / Dynamic Island 均用它） | 中：需给 `windows` crate 加媒体 feature，**具体 feature 名待实测门控**（同类经验：`ApplicationModel` 曾必须显式添加才通过编译） | 高：这是「瞄一眼」信息密度最高的一项，也是同类产品的公共基线 | 低。个别旧播放器不注册全局会话，需窗口标题兜底（WinIslands 已知限制） |
| A2 | **通知历史**（把已有 `UserNotificationListener` 从「只筛 QQ」扩到全量） | 已有 WinRT 能力（`notify/qq.rs` 已在用） | **低**：过滤器从 `AppUserModelId == "QQ"` 放宽，另存一份历史 | 高：岛成为「刚才那条弹窗说了什么」的兜底 | 中：全量镜像涉及隐私，须默认关闭 + 明确说明「不落盘」 |
| A3 | **系统 HUD 接管**（音量/亮度/麦克风静音） | `IAudioEndpointVolume` 回调 / `IAudioMeterInformation`；亮度走 `WMI` 或 `PhysicalMonitor` API | 中高：要判断「用户是按了音量键」而不是「别的程序调了音量」 | 中：消除系统左上角大方块，是 macOS 那批产品的立身之本 | 中：亮度 API 在部分外接显示器上不可用 |
| A4 | **设备电量**（蓝牙耳机 / 手柄 / 电池） | `Windows.Devices.Power`（WinIslands 用它读电池）；Logitech HID++ / DualSense 报表（Dynamic Island 路线） | 中：通用电池低，耳机要看品牌协议 | 中：开会前知道耳机电量 | 低 |
| A5 | **录屏 / 全屏时自动勿扰** | `SHQueryUserNotificationState`（**已在用**，见 `win.rs::foreground_is_fullscreen`） | **极低**：现成判定，只需把「泊走」改成「泊走 + 静默提醒」 | 中：避免演示时岛弹提醒 | 低 |

> A5 是目前性价比最高的一条：判定逻辑已经在代码里跑着（`QUNS_BUSY` / `D3D_FULL_SCREEN` / `PRESENTATION` / `APP`），
> 只是当前只用于隐藏窗口，没有用于「压住提醒」。

### 4.2 B 类：DCX 独占串联（竞品结构上做不到）

这一类是本文的主判断：**DCX 有三个应用和一个注入引擎，岛应该成为它们的统一入口**。

| # | 接入点 | 咬合的对象 | 成本 | 为什么别人做不到 |
|---|---|---|---|---|
| B1 | **岛内直接搜文件** | `dcx-search`（已有索引） | 中：需要一个「查一次返回前 N 条」的进程间调用。当前 `dcx-search` 只有 GUI 入口（`main.rs:80` 读 `env::args()`），**没有对外查询接口** | 竞品没有文件索引，只能调 Everything 或自建 |
| B2 | **拖文件到胶囊 = 直接收进收纳盒** | `dcx-organizer`（`absorb` / `move_into_dir` 已实现） | 中低：胶囊是常驻顶部的天然 drop target；`dropped_files` 已有（eframe 内置） | 竞品没有收纳库，「拖拽暂存」只能暂存不能归档 |
| B3 | **收纳库待整理条数上岛** | `dcx-organizer::counts()` / `desktop_items()` | **低**：读 `organizer.toml` + 扫目录，与 `Perf` 页同级 | 同上 |
| B4 | **剪贴板条目一键收进盒子**（文本存成 `.md`/`.txt`，图片存 `.png`） | `clipboard.rs` + `organizer` | 低 | 同上 |
| B5 | **任务栏窗口列表 / 悬停预览** | `dcx_engine.dll`（注入 Explorer + XAML 量测） | **高**：要新增一条引擎线协议，且与 `box_slot.txt` 的既有职责冲突 | 独家能力，但成本与风险都高，**建议只做评估不排期** |

> B1–B4 的共同点：**都是「岛作为入口，工作仍由已有应用完成」**，不需要重写任何业务逻辑。
> 这也是唯一能让「第四个应用」不变成第四个孤岛的做法。

### 4.3 C 类：效率外设

| # | 接入点 | 成本 | 判断 |
|---|---|---|---|
| C1 | **长任务进度上岛**（构建、文档生成、AI 任务、下载） | **低**（是 §5 那套 API 的第一个消费者） | **强烈建议**。Droppy 与 WinIslands 都把「AI 会话进度」单列为卖点，说明这类「有开始-有结束、有进度」的事件是岛的天然内容。DCX 自己的构建就能当第一个用例 |
| C2 | 番茄钟 / 专注模式（联动系统勿扰） | 低 | 可选。与既有「间隔提醒」模型重叠，要避免做成两套定时器 |
| C3 | 快捷启动器（面板输入即搜应用/命令） | 低中 | 可选。与 B1 共用一套「输入框 → 结果列表」，做了 B1 就顺带有了 |
| C4 | **日历 + 一键入会** | 中高：取数依赖 Outlook COM 或本地日历，**本机是否存在可读日历源待确认** | 价值明确（NotchNook、Dynamic Notch 都把 Join 按钮当核心卖点），但 Windows 侧没有平台级日历 API，成本不确定 |
| C5 | 文件中转站（拖拽暂存） | 低 | **不建议单独做**：与 B2 重复。要么直接归入收纳盒，要么不做 |

### 4.4 D 类：跨设备

| # | 接入点 | 成本 | 判断 |
|---|---|---|---|
| D1 | 局域网互传的「收」一半（起 HTTPS 监听 + 岛内二维码） | 中 | 可后置。做法已被 Dynamic Island for Windows 验证（QRCoder 生成二维码） |
| D2 | 手机通知同步 | **高**：需要手机端 App + 配对 + 凭证 | **不做**。与 §6 的合规取向一致 |

---

## 五、最高杠杆的一条：本地推送 API

### 5.1 为什么这比做十个功能值钱

三个理由，按重要性排序：

1. **它把「功能」变成「接口」**。每加一个功能都要改岛、改设置页、改文档、重新部署；开一套 API 之后，
   新需求由**调用方**解决 —— 飞总自己的构建脚本、文档生成管线、任何未来应用。
2. **它让 DCX 的三个应用之间有了统一的上报通道**。收纳盒同步完 → 岛显示「已收纳 12 个」；
   检索索引重建 → 岛显示进度。这类跨应用反馈目前没有任何通道。
3. **竞品已验证它是刚需**。WinIslands 的官网首页把「给开发者：把你的软件送上灵动岛」列为独立板块，
   提供 7 个端点与完整示例 —— 这不是彩蛋，是被当作产品定位在推。

### 5.2 接口形状建议（对标 WinIslands，按 DCX 约束调整）

WinIslands 的实际端点（官网 + README 一致）：

```text
POST   /v1/island/push            推送/更新一张卡片
POST   /v3/island/push            v1 超集：图片 / 动态进度 / 心跳
PATCH  /v3/island/push/{id}       部分更新
DELETE /v1/island/push/{id}       移除卡片
GET    /v1/island/active          查询当前活跃卡片
GET    /v3/ws                     WebSocket 双向通道
GET    /v1/health                 健康检查
```

它载体的字段包括：标题 / 正文 / 图标 / 副标题、进度、按钮（打开链接 / 启动程序 / 执行命令 / notify 回调）、
输入框、图片、动态进度、**心跳续期**、深浅主题、强调色、优先级队列。还有一条值得抄的设计约束：
**推送不改变灵动岛宽度**（避免第三方内容把岛撑变形的抖动）。

DCX 版本的建议差异（不是照搬）：

| 项 | 建议 | 理由 |
|---|---|---|
| 传输 | 先只做 **HTTP over 127.0.0.1**，WebSocket 后置 | `sys.rs` 已有手写 WinHTTP **客户端**；服务端要另写监听。本地回环不需要 TLS，省掉证书与 SChannel 服务端配置 |
| 端口 | 固定默认值 + `island.toml` 可改；启动时 bind 失败则**降级为禁用并写日志**，不阻塞岛 | 端口冲突不能让整个应用起不来 |
| 鉴权 | **必须带 token**（`Authorization: Bearer <token>`），token 首次启动生成、写 `island.toml`、设置页可查看/重置 | 见 §5.3 |
| 卡片模型 | 复用现有 `InboxItem` 的思路但**独立一张表**，最多 N 张活跃卡片、带优先级与 TTL | 不与「消息」混在一起：收件箱是历史，卡片是「正在发生」 |
| 与提醒的关系 | 卡片走**同一个 `flash_envelope` 脉冲**，但不触发面板自动展开 | 与 §9.11③ 的既有结论一致：不为一条信息抢桌面 |
| 暴露给本机脚本 | `dcx-island.exe --push <json>` 子命令（走同一套本地 HTTP） | 构建脚本不用引 HTTP 库，一行命令即可 |

### 5.3 安全缺口（必须在第一版就定）

本机任意进程都能 `POST 127.0.0.1:<port>`。没有 token 时，任何程序（包括被诱导运行的脚本）都能在这个
**topmost 窗口**上写任意文案与按钮 —— 按钮类型还包含「执行命令」（WinIslands 的 `action.type` 有这一项）。

三条底线：

1. **默认只绑 `127.0.0.1`**，不开 `0.0.0.0`。
2. **Bearer token 必填**，token 存 `island.toml`（不是凭据管理器，因为它要能被脚本读到；设置页可一键重置）。
3. **`action.type` 不支持「执行命令」**。只允许「打开 URL / 打开文件所在目录 / 切到某个面板页」。
   一个 topmost 的提醒窗口如果能执行命令，它就是一个钓鱼入口。

---

## 六、明确不做

| 能力 | 理由 |
|---|---|
| 逐字歌词 / 频谱 | 与 egui 立即模式渲染的帧预算冲突；且属娱乐场景，与 DCX「减少切窗口」的定位无关。要做也只在面板内做整句，不做逐字 |
| GPU / 温度 / FPS | `island-app-plan.md` 已判定首版不做（WinIsland 曾因 GPU 口径与任务管理器不一致返工）；LibreHardwareMonitor 是 MPL-2.0，可单独引用但要额外进程或驱动权限 |
| 插件系统 / 插件市场 | 已明确排除（`island-app-plan.md` §2.3）。§5 的 API 是它的**替代品**：不加载第三方代码，只接收数据 |
| 手机通知同步 | 需要手机端 App + 配对协议 + 凭证管理，本项目不成立 |
| 摄像头镜像 / 颜色拾取器 / 天气 / 壁纸取色 | macOS 场景或纯装饰，不服务「少切窗口」 |
| 任务栏窗口预览（B5） | 独家能力但成本与协议风险都高，只做评估 |

---

## 七、优先级建议

| 批次 | 内容 | 验收 |
|---|---|---|
| **P0（补齐门槛 + 打开接口）** | A1 媒体会话、A2 通知历史（默认关）、**§5 本地推送 API + token**、A5 勿扰联动 | 播放器切歌胶囊即变；全系统弹窗可在面板回看；`curl -H "Authorization: Bearer …" -d '{...}' /v1/island/push` 能在岛内出一张卡且 12 s 自消失；无 token 返回 401 |
| **P1（DCX 串联）** | B2 拖文件进胶囊收纳、B3 待整理条数、B4 剪贴板收进盒子、C1 长任务进度（自己构建先用） | 拖 3 个文件到胶囊 → 收纳库对应分类出现、桌面消失；`cargo build` 期间岛内有进度条 |
| **P2（效率外设）** | B1 岛内检索（需先给 `dcx-search` 加查询接口）、C3 快捷启动器（与 B1 共用结果列表） | 输入文件名回车即打开；`Ctrl+Enter` 定位到文件夹 |
| **探索（不排期）** | C4 日历+入会（先确认本机有无可读日历源）、D1 局域网收件、B5 任务栏预览 | 各自需要一次独立的可行性确认 |

---

## 附：信息来源

| 结论 | 来源 |
|---|---|
| WinIslands 能力、端点、端口 9840、MIT、.NET 8 + WPF | `winislands.judekwong.com` 首页「给开发者」板块、`github.com/JudeKwong/WinIslands` README（含 `docs/IslandAPI.md` 索引） |
| WinIsland（WinIslandProject）GPL-3.0 / 插件 / 灵言 / 媒体歌词 | `docs/island-app-plan.md` §2（前一轮已实读复核） |
| fly-island 能力与通知边界 | 同上 §2、§4.5 |
| Dynamic Island for Windows 的库与模块（NAudio、LibreHardwareMonitor MPL-2.0、PresentMon、QRCoder、HeadsetControl GPL-3.0） | `github.com/Drey0911/Dynamic-Island-WIN-10-11` README 的 credits 表 |
| macOS 侧产品能力对比 | 第三方汇总（crestnotch.app、dynamicnotch.tech、getdroppy.app）；**属转述，未经各自官网逐项复核** |
| KDE Connect / LocalSend / Sefirah 能力与许可 | kdeconnect.org、localsend.org、第三方对比文（转述） |
| SMTC / `Windows.Devices.Power` 的选用 | WinIslands 与 Dynamic Island README 均声明使用；**DCX 侧 feature 门控未实测** |
| DCX 现状（Tab 枚举、CapsuleState 字段、`foreground_is_fullscreen`、`dcx-search` 无对外查询接口） | 本仓库 `crates/dcx-island/src/ui/{mod,compact,panel}.rs`、`crates/dcx-island/src/win.rs`、`crates/dcx-search/src/main.rs:80`、`crates/dcx-organizer/src/lib.rs`（`absorb` / `move_into_dir` / `counts` / `desktop_items`） |
