# 信息岛生态评估的落地性复核与实施契约

> 2026-11-08 · 配套文档：`island-ecosystem-review.md`（生态盘点与接入点评估）。
> 本文是对该报告的**代码级复核**结论与本轮落地的实施契约：范围、设计决策、验收口径。
> 分支：`feat/island-ecosystem`。基线：2026-11-07 深色主题工作区交付已先行提交保留。

---

## 一、复核总结论

报告可落地。P0/P1 无硬性技术障碍；主判断（本地推送 API 是最高杠杆、岛做三应用统一入口）
与代码事实吻合。复核中确认的关键地基：

| 报告假设 | 代码事实 | 证据 |
|---|---|---|
| 外部事件有统一进 UI 的管线 | 专用线程 + mpsc → housekeeping 200ms 排空 → `request_repaint` | `notify/mod.rs:90-149`、`app.rs:290-304` |
| 卡片可复用 flash 脉冲且不弹面板 | `flash_envelope` 12s；「岛闪烁、不抢桌面」是明文设计 | `app.rs:1283-1294`、`app.rs:355-359` |
| WinRT 已在后台线程使用 | `RoInitialize(MTA)` + `UserNotificationListener` 15s 轮询 | `notify/qq.rs:89-143` |
| SMTC feature 门控「待实测」 | **已静态确认**：windows 0.62.2 有 `Media_Control = ["Media"]` | registry 源码 Cargo.toml:209 |
| organizer 可被岛直接链接 | 纯库 crate，box/host 已 link；`counts()/desktop_items()` 只读 | `dcx-organizer/Cargo.toml`、`lib.rs:879,759` |
| search 无对外查询接口 | bin-only，`main.rs:80` 只认 `--help`，全仓库无 pipe/socket | `dcx-search/src/main.rs:22-30,80-86` |
| 无 HTTP 服务端/token | 属实；但手写协议有先例（IMAP/SMTP/WinHTTP 客户端） | `notify/mail.rs`、`sys.rs:767-914` |

## 二、对报告的五处修正（已并入设计）

1. **A5 不是改一行**：`take_due` 先标 `fired_on` 再返回（`tasks.rs:457-459`），全屏期间照常调用
   再压警报 = 提醒永久丢失。做法：**全屏期间不调 `take_due`**，退出全屏后由既有 `STALE_AFTER`
   语义决定补发或丢弃。判定独立于 `hide_fullscreen` 开关（`self.parked` 被其门控，`app.rs:529`）。
2. **token 不能由岛写进 `island.toml`**：岛只写 `[ui]`（`patch_ui`），设置中心合并写只保留磁盘
   `[ui]`（`settings_app.rs:2202-2212`），岛生成的 `[api].token` 会被防抖写回冲掉。
   做法：**token 放独立文件** `%LOCALAPPDATA%\dcx\island_api_token.txt`，岛首次启动生成，
   设置中心只读/重置；服务端按 mtime 重读。这是对「凭据只进凭据管理器」惯例的显式破例，
   理由：脚本要能一行读到。
3. **`--push` 必须在单实例逻辑前分支**：`main.rs:88-92` 二次启动会 touch `island_show`
   （toggle 开关），否则每次推送都会意外开合面板。
4. **B1 的新鲜度与内存账**：search 默认关，快照只在它运行时每小时/退出写
   （`search app.rs:276-306`）；lib 化常驻索引要把 159MB 快照搬进常驻小窗。
   选路：**`dcx-search.exe --query` 无头 CLI 模式**（每次约 0.5s 加载、零常驻内存、
   在单实例互斥前分支），岛侧工作线程调用 + 300ms 防抖。快照缺失/签名不符时诚实报
   「索引不可用，请先运行文件检索」。
5. **B2 的 `dropped_files` 只是框架层**：岛内零拖拽代码；winit 自注册 OLE drop target，
   不得二次 `RegisterDragDrop`（`dcx-box/shell_io.rs:3-6`）。`absorb` 同步阻塞、跨卷回退
   复制+删除、无进度回调（`organizer/lib.rs:996,1465-1489`）——**岛必须放工作线程**，
   不抄 box 的 UI 线程直调。

另有两处比报告更乐观：A2 的 inbox 本就不落盘（隐私口径天然成立），但继承
「未打包 exe 通常 `GetAccessStatus=Unspecified`」的可用性门槛（`qq.rs:99-103`），降级路径已有；
HTTP 服务端不背重依赖（axum 带 tokio 与 egui 线程模型不符），`std::net::TcpListener` 手写。

## 三、范围

**本轮落地**：P0（A2、A5、推送 API、A1）→ P1（B3、B4、B2、C1）→ P2（B1、C3）。
**维持不做**：逐字歌词/频谱、GPU/温度/FPS、插件系统、手机通知同步、摄像头镜像/拾色器/天气。
**不排期**：C4 日历入会、D1 局域网收件、B5 任务栏预览、A3 HUD 接管、A4 设备电量。

## 四、实施设计（按推进顺序）

### 4.1 A2 通知历史（默认关）

- `[inbox] system: bool = false`。开启后 WinRT 监听不再按 QQ 过滤，全量 Toast 进收件箱；
  QQ 渠道开关语义不变（`system=false` 时维持现状）。
- `Source` 增加单元变体 `System`（保持 `Copy`，全仓按值传递），应用显示名放
  `InboxItem.app: String`（取 `AppInfo.DisplayInfo.DisplayName`，缺省回退 AUMID 末段）；
  渠道圆点/激活逻辑对 System 不做 App 激活（无可靠 AUMID 激活路径），点击仅标已读。
- 监听线程启动条件从 `inbox.qq` 放宽为 `inbox.qq || inbox.system`。
- 隐私：内存 ring（现有 keep 裁剪），**不落盘**；设置页文案「默认关闭 · 只读 · 不落盘」。
- 授权不可用时沿用现有 `ChannelState::Unavailable/Denied` 降级。

### 4.2 A5 全屏勿扰

- `win.rs` 暴露 `pub fn shell_busy() -> bool`（包装现有 `shell_says_no_popups`，
  QUNS_BUSY/D3D_FULL_SCREEN/PRESENTATION/APP）。
- `[behavior] quiet_fullscreen: bool = true`。
- `on_tick` 中 `quiet_fullscreen && shell_busy()` 时**跳过 `take_due`**（提醒整体暂缓，
  外发 dispatch 随提醒一起暂缓）；退出全屏后由 `STALE_AFTER` 决定补发。
- 与 `hide_fullscreen` 完全独立：后者管可见性，前者管提醒。

### 4.3 本地推送 API（P0 主件）

- **传输**：手写 HTTP/1.1，`std::net::TcpListener` 仅绑 `127.0.0.1`；每连接一线程，
  读超时 5s；头部 ≤8KB、body ≤64KB；不支持 chunked；一律 `Connection: close`。
- **配置**：`[api] enabled = true, port = 9841`（可改；bind 失败 → 写日志降级禁用，不阻塞岛）。
- **鉴权**：除 `GET /v1/health` 外全部要 `Authorization: Bearer <token>`，否则 401。
  token = 32 字节随机 hex（`BCryptGenRandom` FFI，无新依赖），存独立文件（见修正 2），
  服务端按 mtime 重读；设置页可查看/复制/重置。
- **端点**：`POST /v1/island/push`（同 id 更新并续期）、`GET /v1/island/active`、
  `DELETE /v1/island/push/{id}`、`GET /v1/health`。
- **卡片模型**：独立 `PushCards`（不与 Inbox 混）：`id/title/body/progress(0..1)/
  ttl_secs(默认12, 上限3600)/action/created_at`。UI 线程单写者：服务端线程经 mpsc 投递，
  housekeeping 排空入表；`GET active` 经 `Arc<Mutex>` 只读快照。TTL 到期 UI 线程移除。
- **动作白名单**（无「执行命令」）：`open_url`（仅 http/https）、`open_folder`
  （必须是现存目录，走 explorer.exe）、`show_panel`（仅合法 tab key）。
- **呈现**：新 Tab「动态」：卡片列表（标题/正文/进度条/动作按钮/剩余时间）。
  新卡走 `flash_envelope` 脉冲、**不自动展开面板**；胶囊在任提醒优先、其次显示最新卡标题，
  **不改变岛宽**。进度卡在胶囊以百分比文本呈现。
- **CLI**：`dcx-island.exe --push '<json>'`（单实例逻辑前分支；读 token 文件 → POST →
  打印响应 → 退出码反映成败）。不触碰 `island_show`。
- **设置页**：「推送 API」卡片：启用开关、端口、token 显示/复制/重置、降级说明。

### 4.4 A1 媒体会话

- windows crate 加 `Media_Control` feature。新 `media.rs` 后台线程：
  `GlobalSystemMediaTransportControlsSessionManager` + 会话/属性/播放/时间轴事件，
  事件置 dirty + 1s 兜底轮询；状态经 mpsc 进 UI（与其他源同构）。
- 封面：缩略流解码为 RGBA（image crate 加 `jpeg` feature），媒体线程解码，UI 建纹理。
- 控制：上一首/播放暂停/下一首，UI → mpsc → 媒体线程执行 `Try*Async`。
- 呈现：面板顶部媒体行（封面/标题/艺术家/三键），有会话才出现；
  `[media] enabled = true, capsule = true`；胶囊在播放且无提醒/无活跃卡时显示曲名（截断）。
- 播放进度本地插值（position + elapsed × rate），不每帧问 WinRT。
- 不做窗口标题兜底（v1 接受个别旧播放器无会话的限制，文档注明）。

### 4.5 P1 串联

- **B3**：island 依赖 `dcx-organizer`；独立低频线程（5s + organizer.toml mtime）算
  `desktop_items().len()` 与 `counts()`，Perf 页加「桌面待整理 / 收纳库」行 +
  「打开收纳库」按钮（explorer.exe 打开库根目录）。只读，无跨进程写冲突。
- **B4**：剪贴板条目加「收进盒子」：文本写 `剪辑 <时间戳>.md`、图片**复制** PNG 到暂存
  （不能直接 absorb 原文件，会搬空剪贴板图引用），工作线程 `absorb` + `save_keep_gone`
  （patch 语义，不盲 save），完成 toast。
- **B2**：岛窗口消费 `raw.dropped_files`（winit 已注册 OLE，不重复注册）；
  `hovered_files` 时胶囊高亮；投放 → 工作线程 absorb → toast「已收纳 N 个」。
- **C1**：`tools/island-push.ps1`（通用推送 helper）+ `tools/build-island.ps1`
  （包 cargo build：开始卡 → 按 Compiling 行更新进度卡 → 成功/失败收尾卡）。

### 4.6 P2 检索 + 启动器

- **B1**：`dcx-search.exe --query <词> [--limit N] --json` 无头模式：单实例互斥**前**分支，
  读 search.toml 算签名 → `load_snapshot` → `query::run` 收集 Batch 到 Done → 打印 JSON。
  快照缺失/签名不符 → 退出码 2 + 明确报错。岛侧工作线程调同目录 search exe，300ms 防抖。
- **C3**：与 B1 共用「搜索」Tab（输入框 → 结果列表）：本地缓存开始菜单 .lnk 应用名，
  输入 ≥2 字符同时查应用与文件；Enter 打开首项，Ctrl+Enter 定位文件夹，点击行打开。
- 面板 Tab 终态：任务/剪贴板/消息/性能/动态/搜索（6 个，默认宽度内排得下，实测验证）。

## 五、验收口径

| 项 | 验收 |
|---|---|
| A2 | 默认关；开启后系统 Toast 进消息页（含应用名）；关闭即恢复 QQ-only；不落盘 |
| A5 | 全屏（QUNS_BUSY）期间到点任务不响不闪；退出全屏后 STALE_AFTER 内补发；开关独立 |
| API | `curl` 无 token → 401；带 token push → 动态页出卡、胶囊脉冲不展开、12s 自消失；同 id 续期；DELETE 移除；`--push` 不触碰 island_show；端口被占岛正常启动 |
| A1 | 播放器切歌胶囊/面板即更新；三键可控；无会话时媒体行消失 |
| B3 | Perf 页条数与桌面实际一致；打开库目录正确 |
| B4 | 文本/图片收进盒子后分类正确；剪贴板原条目仍在（图片引用不失效） |
| B2 | 拖 3 个文件到胶囊 → 收纳库出现、桌面消失；大文件不冻窗 |
| C1 | `build-island.ps1` 构建期间岛内有进度卡，结束有成败卡 |
| B1/C3 | 输入文件名回车打开；Ctrl+Enter 定位文件夹；应用名可搜；索引缺失时诚实报错 |
| 回归 | 既有四 Tab/提醒/剪贴板/四路消息/深色主题不变；`cargo test` 全绿；clippy 无新增告警 |

## 六、部署与验证

- 构建：`CARGO_TARGET_DIR=D:\cargo-dbx-target`，release。
- 部署：备份 `dcx-island.exe / dcx-host.exe / dcx-search.exe`（时间戳后缀）→
  停 host/settings/island/search（box 不动）→ cp → `start-dcx.bat`。
- 实机验证：API 全端点 curl 探针、`--push` 后 island_show 未被 touch、设置页新卡片截图、
  面板六 Tab 截图；媒体会话与全量通知若本机无法构造真实源，记录为验证缺口。
- 回滚：备份副本即时可换。

## 七、第二轮 review

落地完成后：独立审查代理过全部 diff（对照本文契约）+ clippy/test 全量跑，
发现落 `docs/review-2026-11-08-island-ecosystem.md`，关键问题修复后复验再交付。