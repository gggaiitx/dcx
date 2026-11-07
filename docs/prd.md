# DCX 文件检索 · 产品需求与开发方案（PRD）

| 项 | 内容 |
|---|---|
| 文档版本 | v1.0（方案定稿，待实施） |
| 日期 | 2026-11-05 |
| 状态 | 需求已确认，进入 P0 前待第 11 节事项拍板 |
| 适用工程 | `C:\Users\Administrator\WorkBuddy\Windhawk\dcx`（Rust workspace，11 crates） |
| 新增应用 | `dcx-search.exe`（crate `dcx-search`） |
| 交付边界 | 本文档只定义需求与方案；**本次不修改任何源码、不改任何既有线协议文件** |

---

## 0. 文档约定

- 需求编号 `FR-xx`（功能）、`NFR-xx`（非功能）、`R-xx`（风险）。
- 尺寸单位：**pt = egui 逻辑点**；Win32 API 与屏幕坐标是**物理像素**，换算关系 `物理 = pt × scale`，`scale = dpi / 96`。本机实测 125%，故 `scale = 1.25`。
- 「默认关闭」「启用」「常驻」是三个不同概念，定义见 §3.1，全文按该定义使用。
- 引用现有代码一律给出相对路径与符号名；行号以 2026-11-05 工作区快照为准。

---

## 1. 背景、目标与边界

### 1.1 背景

DCX 平台当前进程模型为：`dcx-host.exe`（注入看门狗 / 托盘 / 唯一设置中心 / 应用注册表）、`dcx_engine.dll`（注入 Explorer）、`dcx-box.exe`（桌面收纳盒独立应用）。新增「文件检索」作为第二个独立应用。

用户已确认三项决策：

1. **采用双层架构**：文件名检索走常驻内存索引 + 增量监听；内容检索才用 `grep-searcher` 流式扫候选集。
2. **默认关闭**，在桌面任意位置通过**全局快捷键**唤起 UI。
3. UI 参考截图：支持切换盘符/路径，**并新增「检索文件名 / 检索内容」模式切换**。

### 1.2 产品目标

| 目标 | 可度量结果 |
|---|---|
| G1 全盘文件名检索达到「输入即出」 | 索引就绪后，输入到首批结果 ≤ 100 ms |
| G2 内容检索可用且可控 | 在候选集上流式出结果，可取消、有上限、不卡 UI |
| G3 零打扰 | 默认状态下无 `dcx-search.exe` 进程、无后台磁盘 IO、无开机索引 |
| G4 任意位置唤起 | 全局热键在桌面、资源管理器、任意前台应用下均可唤起 UI |
| G5 融入 DCX | 设置集中在 host 唯一设置窗口，不自启、不加托盘、不碰引擎线协议 |

### 1.3 非目标（本期不做）

- **网页搜索**：截图中的「网页搜索」标签仅作视觉占位/禁用态，不接任何网络检索。
- 全盘文件内容倒排索引、PDF/Office 正文抽取、压缩包内检索、OCR。
- 文件搬移 / 重命名 / 删除等写操作（检索是只读功能）。
- 独立设置窗口、独立托盘图标、独立开机自启项。
- 占用任务栏槽位（`box_slot.txt` 属于收纳盒与引擎的既有契约）。
- NTFS MFT / USN journal 提权通道（列为可选 P4，见 §10）。

### 1.4 术语

| 术语 | 定义 |
|---|---|
| 名称索引 | 常驻内存的「路径 → 元数据」紧凑表，由一次冷启动遍历建立，查询只查内存 |
| 增量监听 | 用 `ReadDirectoryChangesW`（`notify`）把创建/删除/重命名事件回写到名称索引 |
| 候选集 | 内容检索前，由名称索引按作用域、扩展名、大小、mtime 筛出的待读文件集合 |
| generation | 每次查询自增的 `u64`，用于让旧查询的工作线程自行退出并丢弃过期结果 |
| 按需态 / 常驻态 | 见 §3.2 状态机 |

---

## 2. 关键决策与依据

### 2.1 为什么文件名必须走索引

Everything 官方 FAQ 明确：只索引文件与目录名，25 万文件约 5 秒、100 万文件约 1 分钟建索引；100 万文件约 100 MB 内存 / 45 MB 磁盘；结果实时随文件系统变化更新；**内容不建索引，`content:` 检索很慢**；NTFS 索引需要 Everything 服务或管理员权限。

结论：若每次击键都用 `ignore::WalkParallel` 重新遍历全盘，延迟随文件数线性增长且连续输入会叠加遍历，无法达到 G1。因此采用：

```
冷启动一次并行遍历  →  内存名称索引  →  变更监听增量维护  →  查询只查内存
                                          ↘ 内容检索：从索引取候选集 → grep-searcher 流式扫
```

### 2.2 为什么「默认关闭」与「全局热键」不矛盾

`apps.rs::spawn_enabled()` 会拉起所有 `enabled=true` 应用并用 5 秒看门狗保活；`default_enabled()` 恒为 `true`。若直接登记新应用，安装即触发全盘索引，违反 G3。

解法：**把热键监听放在常驻的 host 进程，把检索进程按需拉起**。host 本来就是开机自启的平台，它注册全局热键；按下热键时才 `launch("search")`。这样「默认关闭」= 不常驻、不自启、不预建索引，但功能始终可通过热键获得。

### 2.3 借鉴的开源项目（取用 / 不取用）

| 项目 | 取用 | 不取用 | 许可证 |
|---|---|---|---|
| ripgrep：`ignore` / `grep` facade / `grep-regex` / `grep-searcher` | `WalkBuilder`、`WalkParallel`、`WalkState::{Continue,Skip,Quit}`；`Searcher → Matcher → Sink` 三段式；BOM 嗅探、二进制检测、行号 | CLI 参数层、printer、pcre2；优先用 `grep` facade（0.4.x）而非手拼 6 个子 crate | MIT OR Unlicense |
| fd | `BatchSender`（默认 0x100 / 批）、有界通道容量 `2 × threads` 做背压、`ReceiverBuffer` 的 100 ms 缓冲/流式切换、原子 `quit_flag` | `-x` 执行类、颜色输出 | MIT / Apache-2.0 |
| Everything | 「名称索引 + 变更增量」产品模型；`ext:` / `path:` / `content:` 查询语法；`[作用域] 共匹配 N 个文件` 状态栏 | 闭源 SDK、ETP/HTTP 服务、MFT 提权（P4 再评估） | 闭源免费软件，只借鉴行为 |
| VS Code 搜索 | 结构化流式结果（`begin/match/end/summary`）、UI 侧结果上限、取消即终止 | 子进程 IPC（本项目同进程更轻） | MIT |
| `notify` | Windows `ReadDirectoryChangesW` 后端（16 KB 缓冲 + 完成例程 + alertable wait） | `PollWatcher`（全盘轮询开销过大） | CC0-1.0，需法务复核 |
| `nucleo-matcher` | fzf 兼容打分、UTF-32/SIMD 模糊排序（项目自测 Linux 内核语料约 2.3 ms，skim 约 17.4 ms） | `nucleo` 高层 picker（API 仍在演进）；若 MPL-2.0 不可用则改自研加权打分 | MPL-2.0，**需法务确认** |
| `ntfs-reader` / `usn-journal-rs`（可选 P4） | MFT 初扫 + `FSCTL_QUERY/READ_USN_JOURNAL` 增量思路 | 本期不引入（需管理员权限） | MIT/Apache-2.0 等，逐个复核 |

依赖版本以实施时 crates.io 当期为准；参考锚点：`grep 0.4.1`（2025-10-22）、`grep-searcher 0.1.17`（2026-07-15）、`ignore 0.4.x`、ripgrep `15.2.0`（2026-07-15）。

---

## 3. 运行模型与状态机

### 3.1 「默认关闭」的精确定义

| 维度 | 默认行为 |
|---|---|
| `apps.toml [apps.search].enabled` | `false` |
| 开机 / host 启动时 | **不**拉起 `dcx-search.exe`，**不**建索引 |
| 后台磁盘 IO | 0（host 只监听热键，不扫描） |
| 全局热键 | **仍然有效**，由 host 常驻监听 |
| UI | 默认不显示 |
| 首次唤起 | 立即显示窗口；索引在后台线程建立，结果边建边出 |

设置中心把开关命名为**「开机预建索引（常驻）」**而非「启用文件检索」，副标题写明「关闭时仍可用快捷键唤起；首次唤起会即时建立索引」，避免用户误以为功能被禁用。

### 3.2 三态状态机

```
 [休眠 Sleep]  enabled=false，无 search 进程
      │ Alt+Space（host 热键）
      ▼
 [按需 On-demand]  host 调 launch("search")；UI 立即显示；索引后台建
      │ 关闭窗口 / Esc
      ├── residency=release   → 进程退出，索引丢弃 → 回到 [休眠]
      ├── residency=keep_5m   → 进程保留 5 min，超时回 [休眠]
      ▼
 [常驻 Resident]  enabled=true（用户在设置中心开启）
      host 自启即拉起，窗口移到屏幕外，索引 + 增量监听持续运行
      热键只做「前置窗口」，不重复拉起
```

- 常驻态隐藏窗口必须**移到屏幕外 `(-32000,-32000)`**，不能用 `ViewportCommand::Visible(false)`——eframe 隐藏后 `update` 不再被调用。
- 常驻态点击穿透沿用 `SetWindowRgn`，不要回退到 `HTTRANSPARENT`（后者只转发同线程窗口）。
- `residency` 默认 `release`；`keep_5m` 兼顾「关掉后马上又想搜」的体验。

### 3.3 全局热键链路（新增能力，当前仓库无实现）

`Grep RegisterHotKey|WM_HOTKEY` 对 `dcx/crates` 全库无命中，需从零加 `crates/dcx-host/src/hotkey.rs`：

1. **专用线程 + 隐藏消息窗口**：创建 `HWND_MESSAGE` 隐藏窗口并注册窗口类/WndProc，在该线程调用 `RegisterHotKey(hwnd, 1, MOD_ALT | MOD_NOREPEAT, VK_SPACE)`。不采用 `hWnd = NULL` 的形式——那样 `WM_HOTKEY` 投递到线程消息队列，host 主线程的 `dcx_core::Platform::run()` 消息泵不保证把它派发到处理逻辑。
2. **收到 `WM_HOTKEY`**：读 `search.toml` 的 `activation.hotkey_enabled`；若已有实例（`FindWindowW("文件检索")`）则 `PostMessageW(hwnd, WM_DCX_SEARCH_SHOW, …)` 前置窗口；否则调用 `apps::launch_on_demand("search")`。
3. **注册失败**（`ERROR_HOTKEY_ALREADY_REGISTERED` 或组合被占用）：写 `host.log`，设置中心「桌面整理」页显示「快捷键被占用，请改键」，**不阻塞平台启动**。
4. **改键**：设置中心改 `search.toml` → host 热键线程按 mtime 轮询（复用 `config_stamp()` 的 `(len, mtime)` 廉价令牌思路）→ `UnregisterHotKey` 后重新注册。
5. **已知边界**：UIPI 下前台为管理员权限窗口、或全屏独占应用时热键可能不到达；host 未运行时热键无效。两点须写进设置中心说明文案。

默认组合建议 `Alt + Space`（与常见启动器一致）；支持 `Ctrl/Alt/Shift/Win + 键` 自定义。不得硬编码不可改。

### 3.4 进程生命周期与孤儿防护

- `apps::spawn_enabled()` 仅拉起 `enabled=true` 且 `activation=AutoStart` 的条目；按需启动的实例**不进看门狗保活列表**，退出后不会被 5 秒看门狗重新拉起。
- 新增 `apps::launch_on_demand(id)`：跳过 `is_enabled()` 判断，但仍走 `is_running()` 幂等短路、`exe_path()` 同目录定位与 `CHILDREN` 登记。
- `AppEntry.window_title` 必须与 `dcx-search` 的 `WIN_TITLE = "文件检索"` **逐字一致**，否则 `stop()` 的 `FindWindowW` 找不到用户手工双击的实例。
- `D:\dcx-dist\stop-dcx.bat` 的 `taskkill /f` 绕过 `apps::stop_all()`，必须追加 `taskkill /f /im dcx-search.exe >nul 2>&1`，否则强杀 host 后留下孤儿索引进程。

---

## 4. 用户界面规格

### 4.1 参考截图解析（本期基线）

| 区域 | 截图内容 | 本期处理 |
|---|---|---|
| 顶部带 | 地球图标「网页搜索」｜文件夹图标「文件搜索」（蓝色高亮/当前页）；右上角最小化、最大化、关闭 | 保留两标签，网页搜索为**禁用占位**；窗口控制按钮保留 |
| 搜索行 | 左端带下拉箭头的路径图标；输入框占位「输入关键词快速查找本地文件，支持系统通配符?和*，例如：我的工作计划」；右端放大镜 | 保留；**右端新增「文件名 / 内容」分段切换** |
| 路径浮层 | 桌面、此电脑、Win11ProX64 (...)、Data (D:) | 保留并动态枚举卷标/盘符；增加「自定义路径」分组 |
| 分类行 | 文件、图片、压缩包、应用程序、视频、音频、其他（蓝色图标 + 灰字横排） | 保留，复用既有分类规则 |
| 结果区 | 大面积白色留白 | 虚拟列表，见 §4.5 |
| 底部 | `[此电脑] 共匹配0个文件` | 保留并扩展进度/耗时 |
| 配色 | 白底、浅灰描边、蓝色强调、深灰文字 | 对齐 DCX `Skin`（亮色 accent `0x0E7C7E`／暗色 `0x4FD8CC`），截图蓝可作次级强调 |

### 4.2 窗口与布局

- 默认尺寸 **880 × 620 pt**，最小 640 × 480 pt，可拖拽缩放并记住尺寸/位置。
- 唤起位置可配：`mouse`（鼠标所在显示器工作区左上偏中，默认）/ `center` / `last`。
- 标题栏：推荐**无边框 + 自绘 40 pt 标题带**以 1:1 复刻截图并与 DCX 自绘风格一致；P0 允许先用 `with_decorations(true)` 系统标题栏 + 独立标签行快速验证，P1 前切换为自绘。
- 每帧 `ctx.set_pixels_per_point(scale)`；所有窗口定位按物理像素换算（这是收纳盒重写时踩过的坑：误设 `1.0` 会在 125% 下把 UI 缩成 80% 并造成点击错位）。
- 单实例互斥体 `DCX_SEARCH_SINGLETON`（命名互斥体，不用 `FindWindowW` 判重——winit 窗口比进程晚几帧，存在竞态）。

### 4.3 控件明细

| # | 控件 | 位置/尺寸 | 行为 |
|---|---|---|---|
| C1 | 标签「文件搜索」 | 标题带左侧，选中态 | 当前页；「网页搜索」禁用 |
| C2 | 路径选择器 | 搜索行左端，36 × 32 pt + 下拉箭头 | 点击弹出浮层；显示当前作用域图标与短名 |
| C3 | 路径浮层 | 锚定 C2 下方，宽 220 pt | 分组：`桌面` / `此电脑` / 固定卷（卷标 + 盘符 + 可用空间）/ `最近使用` / `选择文件夹…`；支持键盘上下选择；插入 U 盘后重新枚举 |
| C4 | 关键词输入框 | 搜索行中部，flex | 占位文案随模式变化；`*`/`?` 通配符；`Ctrl+F` 聚焦并全选；输入防抖 **80 ms** 后发查询 |
| C5 | **模式分段控件「文件名｜内容」** | 搜索行右端、放大镜左侧，2 × 44 × 28 pt | **新增需求**；默认「文件名」；切换清空结果、重置 generation、改写占位文案与状态栏提示；`Ctrl+1`/`Ctrl+2` 快捷切换 |
| C6 | 搜索按钮（放大镜） | 搜索行最右，32 × 32 pt | 立即查询（输入框回车等价）；内容模式下作为「开始/停止」切换 |
| C7 | 分类快捷行 | 搜索行下方，7 等分 × 64 pt | 「文件（全部）」「图片」「压缩包」「应用程序」「视频」「音频」「其他」；单选高亮；与模式正交 |
| C8 | 结果虚拟列表 | 主体，flex | 见 §4.5 |
| C9 | 状态栏 | 底部 28 pt | `[作用域] 共匹配 N 个文件`；右侧 `扫描 x / 命中 y · 用时 z ms`；冷启动显示索引进度条 |

### 4.4 新增：文件名 / 内容模式切换（FR 核心）

| 维度 | 文件名模式（默认） | 内容模式 |
|---|---|---|
| 数据源 | 内存名称索引 | 名称索引筛出的候选集 + `grep-searcher` 读文件 |
| 占位文案 | 「输入关键词快速查找本地文件，支持系统通配符 ? 和 *，例如：我的工作计划」 | 「输入要查找的文件内容，支持字面量或正则；大文件与二进制默认跳过」 |
| 查询语法 | `*` / `?` 通配符、smart case、空格 AND、`ext:`、`path:`、`regex:` | 字面量 / 正则、大小写、整词；同样支持 `ext:` / `path:` 缩小候选 |
| 首批结果 | ≤ 100 ms | 取决于候选集，需显示进度与「正在检索…」，可随时取消 |
| 结果行 | 图标 / 名称 / 路径 / 大小 / 修改时间 | 增加 行号 + 命中行预览 + 关键词高亮 |
| 状态栏提示 | `[此电脑] 共匹配 N 个文件` | 追加 `内容检索 · ≤2 MB · 编码 自动 · 已扫描 x/y` |
| 取消 | generation 自增即完成 | `Esc` 或再次点放大镜 → generation 自增 + `WalkState::Quit`，100 ms 内收敛 |

模式选择持久化到 `search.toml` 的 `ui.last_mode`；切换时**不得**重建索引。

### 4.5 结果区

- 手写虚拟滚动（复用 `dcx-box/src/view.rs` 的经验：`ScrollArea` 会吞掉行交互），行高 32 pt（内容模式 44 pt，两行显示预览）。
- 文件图标复用 `dcx-box/src/file_icon.rs` 的取图标逻辑；缺失时按分类给占位图标。
- 双击打开（`ShellExecuteW`）；`Ctrl+Enter` 打开所在文件夹并选中；右键菜单：打开、打开所在文件夹、复制文件名、复制完整路径、复制所在目录。
- 内容模式双击定位到命中行（调用系统默认编辑器，行号跳转为 P3 可选）。
- 结果硬上限默认 2000 条，超出显示「仅显示前 2000 条，请细化关键词」。
- 空态三区分：无输入（显示最近使用/提示）、索引建立中（进度 + 已扫描数）、无匹配（保留关键词，给出通配建议）。

### 4.6 主题、字体与可访问性

- `Skin::system()` 按 `dcx_win32::apps_use_light_theme()` 切亮/暗；亮色 `accent 0x0E7C7E`、暗色 `0x4FD8CC`（与设置中心 `Palette` 一致）。
- CJK 字体复用 `dcx-box/src/ui.rs::setup_fonts()` 的回退顺序（`msyh.ttc → msyh.ttf → simsun.ttc → malgun.ttf`），插入到 `FontDefinitions` 首位。
- 所有可交互控件有键盘路径（`Tab` 焦点环、`Enter`、`Esc`）；文本对比度 ≥ 4.5:1；不依赖颜色单独表达状态。

### 4.7 快捷键表

| 快捷键 | 动作 |
|---|---|
| `Alt + Space`（可配） | **全局**唤起 / 前置 UI；窗口已激活时再次按下隐藏 |
| `Esc` | 隐藏窗口；内容检索中先取消检索再隐藏 |
| `Enter` | 打开选中项 |
| `↑` `↓` `PageUp` `PageDown` | 移动选中项 |
| `Ctrl + F` | 聚焦关键词输入框并全选 |
| `Ctrl + 1` / `Ctrl + 2` | 切换 文件名 / 内容 模式 |
| `Ctrl + D` | 展开路径下拉 |
| `Ctrl + Enter` | 打开所在文件夹并选中文件 |
| `Ctrl + C` | 复制选中项完整路径 |
| 右键 | 结果项上下文菜单 |

---

## 5. 功能需求

| 编号 | 需求 | 优先级 | 验收要点 |
|---|---|---|---|
| FR-01 | 全局热键在任意前台应用下唤起/隐藏 UI | P0 | 桌面、资源管理器、浏览器、全屏窗口分别验证；被占用时给出改键提示 |
| FR-02 | 默认关闭：不自启、不常驻、不预建索引 | P0 | 安装后 `tasklist` 无 `dcx-search.exe`，磁盘 IO 无明显增加 |
| FR-03 | 路径/盘符切换：桌面、此电脑、各固定卷、自定义路径 | P0 | 切换后状态栏作用域文本与结果范围一致；U 盘插拔后列表刷新 |
| FR-04 | 文件名 / 内容 模式切换 | P0 | 两种模式结果互不串台；切换不重建索引、不残留旧结果 |
| FR-05 | 文件名查询：通配符、smart case、AND、`ext:`/`path:`/`regex:` | P1 | 中文路径、大小写混合、含空格路径均正确 |
| FR-06 | 分类快捷过滤（7 类） | P1 | 与 `dcx-organizer::default_categories()` 扩展名集合一致 |
| FR-07 | 冷启动全盘名称索引 + 流式出结果 | P1 | 索引未建完即可输入并看到增量结果，进度可见 |
| FR-08 | 变更监听增量维护索引 | P1 | 新建/删除/重命名后 3 s 内结果同步；不重复、不残留旧路径 |
| FR-09 | 内容检索：候选过滤 + `grep-searcher` 流式命中 | P2 | 命中行号、预览、高亮正确；可取消、有上限 |
| FR-10 | 编码策略：UTF-8 / UTF-16 BOM / GB18030 | P2 | 三种编码的中文文件各能命中，不产生乱码行 |
| FR-11 | 结果操作：打开、定位、复制路径、右键菜单 | P2 | 路径含空格/长路径时正确 |
| FR-12 | 设置项集中在 host「桌面整理」页 | P1 | 主开关、热键、内容上限、排除目录、常驻策略 |
| FR-13 | 记住窗口尺寸/位置、上次模式、上次作用域 | P3 | 重启后恢复 |
| FR-14 | 「开机预建索引（常驻）」开启后随平台自启并常驻 | P2 | 与 `apps.toml` 同步，关闭后进程真正退出 |
| FR-15 | 结果项拖出到资源管理器 | P3 | 可选 |

---

## 6. 非功能需求

| 编号 | 指标 | 目标 |
|---|---|---|
| NFR-01 | 索引建立 | 100 万文件 ≤ 120 s（保守目标；Everything 官方同量级约 1 min） |
| NFR-02 | 查询延迟 | 索引就绪后首批结果 ≤ 100 ms；输入防抖 80 ms |
| NFR-03 | 内存 | 100 万文件 ≤ 150 MB（紧凑字符串表，不用 `Vec<String>`） |
| NFR-04 | 空闲开销 | 默认态 0；常驻态空闲 CPU < 1%，不主动扫盘 |
| NFR-05 | UI 流畅度 | 输入与滚动 ≥ 60 fps；后台线程不得阻塞 egui 帧 |
| NFR-06 | 内容检索预算 | 单文件 ≤ 2 MiB、单文件 ≤ 50 命中、全局 ≤ 2000 命中、默认 8 s 时间预算，超限标记 `partial` |
| NFR-07 | 稳定性 | `panic = "abort"` 下工作线程不得 panic；所有失败走 `Result` 通道；panic hook 写 `search_panic.log` |
| NFR-08 | 隐私 | **全盘文件名索引不落盘明文**；必须持久化时加密或限制 ACL；日志不写完整路径以外的用户信息 |
| NFR-09 | 兼容 | Windows 10/11，DPI 100%–200%，亮/暗主题，NTFS/ReFS/FAT/exFAT（后两者无 USN，退化为遍历） |
| NFR-10 | 只读 | 检索过程不修改任何文件的属性、内容或位置 |

---

## 7. 技术架构

### 7.1 分层与数据流

见文首管线图（已在对话中给出）：

```
卷枚举 + ignore::WalkParallel（冷启动一次）
        → 内存名称索引 ← notify（ReadDirectoryChangesW 增量）
        ├─→ 文件名查询（子串/通配/正则/模糊排序，查内存）
        └─→ 内容查询（索引筛候选 → grep-searcher + Sink）
        → bounded channel（批处理 + generation 取消）→ egui 虚拟列表
```

### 7.2 模块清单（`crates/dcx-search/src/`）

| 文件 | 职责 |
|---|---|
| `main.rs` | `windows_subsystem`、DPI、单实例互斥体、panic hook、`--help`、启动参数 |
| `app.rs` | 唯一 `eframe::run_native` 循环、`STARTED` 守卫、`native_options`、每帧 DPI 同步、窗口隐藏/前置/位置 |
| `ui.rs` | `setup_fonts`、`app_icon`、自绘标题栏与窗口按钮 |
| `view.rs` | `Skin`、布局、控件绘制、虚拟列表、右键菜单、主题 |
| `volume.rs` | `GetLogicalDrives` / `GetDriveTypeW` 枚举卷标、盘符、容量；自定义路径管理 |
| `walk.rs` | `WalkBuilder` 配置、`WalkState` 回调、忽略规则、错误收集 |
| `index.rs` | 紧凑路径表、按卷分片、增量 apply、快照视图、查询接口 |
| `watch.rs` | `notify` watcher 生命周期、事件去抖与重命名配对 |
| `query.rs` | 语法解析、模式、generation、取消令牌、排序打分 |
| `content.rs` | 候选过滤、`SearcherBuilder` 配置、`Sink` 实现、编码与二进制策略 |
| `channel.rs` | `Ev` / `Hit` 定义、批处理器、UI 侧限流 |
| `config.rs` | `search.toml` 的 `#[serde(default)]` 结构、原子替换读写、变更戳 |

### 7.3 名称索引结构

```
paths:  Vec<u8>                 // 连续存放 UTF-8 全路径
entries: Vec<Entry { off:u32, len:u32, volume:u8,
                     size:u64, mtime:i64, ext:u16, flags:u16 }>
fold:   Vec<u8>                 // 小写化 / ASCII 折叠的匹配副本（可选，按内存预算开关）
```

- 按卷分片，卷卸载时整片丢弃，避免残留假结果。
- 用 `Arc<RwLock<Snapshot>>` 或 `arc_swap` 发布不可变快照，查询线程持快照读，增量写入新快照后原子替换，避免查询时持写锁。
- `ignore` 在 Windows 把 **HIDDEN 属性**也视为隐藏，全盘检索必须 `hidden(false)`；同时用 `OverrideBuilder` 挂 `.dcxignore` 规则排除 `$Recycle.Bin`、`System Volume Information`、`WinSxS`、`node_modules`、`pagefile.sys`、`AppData\Local\Temp` 等（可在设置中心编辑）。

### 7.4 Channel 流式协议

```rust
enum Ev {
    Batch   { gen: u64, hits: Vec<Hit> },
    Progress{ gen: u64, scanned: u64, matched: u64, elapsed_ms: u64 },
    Done    { gen: u64, partial: bool, stats: Stats },
    Error   { gen: u64, path_id: Option<u32>, code: u16 },
}
struct Hit {
    kind: Name | Content,
    path_id: u32,
    line: Option<u64>,
    col: Option<u64>,
    preview: String,
    score: i32,
}
```

- 两条有界通道：遍历→索引用 `bounded(2 × threads)` 施加背压；查询→UI 用 `bounded(4096)`。
- 工作端按 **256 条或 32 ms** 成批发送（fd 的 `BatchSender` 思路），避免每命中一次就 channel send。
- UI 每帧最多取 4 批 / 512 条，`ctx.request_repaint_after(16ms)` 驱动；硬上限 2000 条。
- 所有写配置沿用「`*.tmp` + `rename`」原子替换（现有 `apps.rs::save()`、`dcx-settings::save()`、`dcx-organizer::save()` 均如此），读方解析失败回退默认值。

### 7.5 取消模型

- `AtomicU64 generation`：每次新查询自增；工作线程在遍历/读文件/发批次前比对，不匹配即 `WalkState::Quit` 并丢弃旧 `Sender`。
- UI 侧收到 `Ev` 先校验 `gen`，过期批次直接丢弃，杜绝「快速输入时旧结果回灌」。
- **不使用 `catch_unwind`**：workspace `[profile.release] panic = "abort"`，工作线程 panic 会终止整个进程；所有 I/O 错误必须走 `Ev::Error`。
- 索引线程设 `THREAD_PRIORITY_BELOW_NORMAL` 并启用电源节流，避免拖慢 Explorer（注入式平台的硬约束）。

### 7.6 增量一致性

- `notify` 递归监听每个已索引卷根目录；事件去抖 100 ms；`RENAME_FROM/TO` 按 FRN 或路径配对，配不上则退化为「删除旧 + 新增新」。
- 已知限制：监听大量目录时 `ReadDirectoryChangesW` 可能丢事件（16 KB 缓冲溢出）；对策是溢出错误触发**该卷局部重扫**并记日志，不做静默忽略。
- 网络盘 / exFAT 无原生事件保证，默认排除或按需轮询（设置项）。
- 索引建立期间产生的变更先入 pending 队列，快照发布后回放，避免「边建边改」漏项。

### 7.7 内容检索管线

1. 从名称索引按作用域 + 分类 + `ext:`/`path:` + `size ≤ 2 MiB` + 非二进制扩展名筛候选集。
2. 工作线程池（默认 `min(CPU, 4)`，IO 密集不宜开满）逐文件 `Searcher::search_path(matcher, sink)`。
3. `Sink::matched()` 收集行号、`String::from_utf8_lossy` 预览、高亮区间；`finish()` 读 `binary_byte_offset()` 判定二进制。
4. `SearcherBuilder` 必须显式配置（**库默认与 CLI 默认不同**）：
   - `line_number(true)`
   - `binary_detection(BinaryDetection::quit(b'\0'))` —— 库默认**关闭**二进制检测
   - `memory_map(MmapChoice::Never)` —— docs.rs 明确：大目录遍历时 mmap 反而更慢，且文件被并发截断时有 bus error 风险
   - `encoding(...)` —— 默认只做 BOM 嗅探（UTF-8 / UTF-16），无 BOM 一律按 UTF-8
5. **中文编码专项**：无 BOM 的 GBK/GB18030 文档在默认配置下会漏检。策略：按 `search.toml` 的 `content.encoding` 选择；`auto` 模式先按 UTF-8 试，若检测到高比例非法序列或用户显式指定，则用 `encoding(Some(encoding_rs::GB18030))` 做二次通道。混合编码目录允许按路径段指定编码。
6. 压缩文件、PDF/Office、媒体默认跳过；OneDrive/云占位符（reparse point / offline 属性）排除，避免读取触发下载。

### 7.8 Windows 专项风险与对策（R 表）

| 编号 | 风险 | 对策 |
|---|---|---|
| R-01 | 全局热键被占用 / UIPI 下失效 | 注册失败降级提示改键；设置中心写明管理员窗口与全屏场景限制 |
| R-02 | 长路径 > 260 字符 | 统一 `\\?\` 前缀访问，UI 展示时剥离前缀 |
| R-03 | junction / 符号链接循环 | `follow_links(false)` + `same_file_system(true)` + reparse point 属性检测，访问出错跳过而非中断 |
| R-04 | 访问拒绝（系统卷、其他用户目录） | 逐目录容错，`Ev::Error` 聚合计数，不中断整个遍历 |
| R-05 | 中文编码漏检 | §7.7 第 5 条 |
| R-06 | 索引明文落盘泄露全盘路径 | NFR-08：只驻留内存；持久化需加密/ACL |
| R-07 | 工作线程 panic 导致进程 abort | NFR-07，错误走通道，panic hook 落 `search_panic.log` |
| R-08 | 每进程多个 winit 事件循环 | 新应用自己一个进程、一个 `run_native`；设置窗口继续由 host `--settings` 独立进程承载 |
| R-09 | DPI 单位混用导致点击/绘制错位 | 每帧 `set_pixels_per_point(scale)`；物理/逻辑严格换算 |
| R-10 | 隐藏窗口用 `Visible(false)` 后 `update` 停摆 | 移到 `(-32000,-32000)` |
| R-11 | 强杀 host 留下孤儿检索进程 | `stop-dcx.bat` 追加 `taskkill /f /im dcx-search.exe` |
| R-12 | 许可证兼容 | `notify` CC0-1.0、`nucleo-matcher` MPL-2.0 需法务复核；不确认则用自研打分 + 直接 `ReadDirectoryChangesW` |

---

## 8. 与现有工程的接入

### 8.1 现状契约（只读勘察结论）

- `crates/dcx-host/src/apps.rs::REGISTRY` 是应用生命周期的唯一开关：`spawn_enabled()`（5 s 看门狗）、`stop_all()`、`settings_app.rs:1039` 的状态卡片全部按 `id` 参数化。
- `AppState::enabled` 缺省 `default_enabled() -> true`（`apps.rs:69-72`），**新应用必须显式给缺省值 `false`**。
- `settings_app.rs:1222` 的 `apps::set_enabled("box", enabled)` 是**硬编码 id**，新增开关必须抽象。
- `settings_app.rs:1036-1054` 的「独立应用」卡片是只读清单，注册表加一行即自动出现。
- `settings_app.rs:1080-1106` 的 `persist_box()` 采用 read-modify-write 合并视图偏好，新配置写入必须沿用，避免回滚另一个进程刚做的修改。
- `box_apps_cards()` 注释已明确：曾因两处都放开关导致 `apps.toml` 与 `organizer.toml` 不一致，新应用只能有**一个主开关**。
- `dcx-organizer::default_categories()` 已有 `doc/image/archive/media/app/other` 六类（`lib.rs:240-285`），截图 7 类可由它映射：`media` 拆成视频/音频两组，`other`→「其他」，再加「文件（全部）」。
- `dcx-organizer::desktop_dir()` 读注册表 Shell Folders 的 Desktop 重定向（本机为 `d:\Users\Administrator\Desktop`），路径下拉的「桌面」直接复用。
- `dcx-box/src/view.rs::Skin`、`dcx-box/src/ui.rs::setup_fonts()`、`dcx-box/src/file_icon.rs` 可复用。
- 既有线协议文件 `settings.toml` / `organizer.toml` / `apps.toml` / `box_slot.txt` / `box_host_alive` / `no_reserve` / `box_ui.txt` 均为稳定契约，**不改名、不复用、不新增同名文件**。

### 8.2 改动清单

| # | 文件 | 改动 |
|---|---|---|
| 1 | `Cargo.toml`（根） | `members` 加 `"crates/dcx-search"`；依赖沿用 `egui 0.29` / `eframe 0.29` / **`winit = "=0.30.13"` 精确锁**（与 eframe 同一实例，`with_any_thread` 才生效） |
| 2 | `crates/dcx-search/Cargo.toml` | 新 crate：`dcx-win32`、`dcx-organizer`、`egui`/`eframe`/`winit`、`serde`/`toml 0.8`、`ignore`、`grep`（或 `grep-regex` + `grep-searcher`）、`crossbeam-channel`、`notify`、可选 `nucleo-matcher` |
| 3 | `crates/dcx-search/src/*` | §7.2 全部模块；`build.rs` 复制 box 的版本资源脚本，换 `brand/search.ico` 与 `FileDescription` |
| 4 | `brand/search.ico`、`brand/embed/search-64.rgba` | 由 `tools/gen_brand.py` 生成，不手工改产物 |
| 5 | `crates/dcx-host/src/apps.rs` | `REGISTRY` 加 `AppEntry{ id:"search", name:"文件检索", exe:"dcx-search.exe", window_title:"文件检索", description:"全盘文件名与内容检索" }`；`AppEntry` 增 `default_on: bool`、`activation: AutoStart \| OnDemand`；`is_enabled()` 按条目取缺省；新增 `launch_on_demand(id)` |
| 6 | `crates/dcx-host/src/hotkey.rs`（新） | §3.3 隐藏消息窗口 + `RegisterHotKey` + `WM_HOTKEY` → 按需拉起/前置；`main.rs` 增加 `mod hotkey` 与启动调用、`--help` 文案 |
| 7 | `crates/dcx-host/src/settings_app.rs` | `:1222` 抽象为 `apps::set_enabled(id, enabled)`；「桌面整理」页追加「文件检索」卡片：主开关（开机预建索引/常驻）、热键录入、内容大小上限、编码、排除目录、常驻策略；**不加导航项、不加独立设置窗口** |
| 8 | `D:\dcx-dist\stop-dcx.bat` | 追加 `taskkill /f /im dcx-search.exe >nul 2>&1` |
| 9 | `D:\dcx-dist\SHA256SUMS.txt` | 追加 `dcx-search.exe` 校验和 |
| 10 | `README.md` / `D:\dcx-dist\说明.txt` | 组成表、文件边界图、分发清单同步新 crate / 新 exe |

### 8.3 配置文件

**`%LOCALAPPDATA%\dcx\search.toml`**（应用读，host 也读热键字段；双方都必须原子替换 + 容错）：

```toml
[activation]
enabled = false              # 不随平台自启、不预建索引
hotkey_enabled = true        # 热键由 host 常驻监听，始终有效
hotkey = "Alt+Space"
residency = "release"        # release | keep_5m | resident
show_on = "mouse"            # mouse | center | last

[scope]
roots = ["desktop", "computer"]
include_hidden = false
follow_links = false
exclude = ["$Recycle.Bin", "System Volume Information", "WinSxS", "node_modules"]
custom_roots = []

[content]
max_file_size_mb = 2
encoding = "auto"            # auto | utf8 | utf16 | gb18030
binary = "skip"
max_results = 2000
time_budget_ms = 8000

[ui]
last_mode = "name"           # name | content
last_scope = "computer"
last_category = "all"
window = { w = 880, h = 620 }
```

**`%LOCALAPPDATA%\dcx\apps.toml`**：

```toml
[apps.box]
enabled = true

[apps.search]
enabled = false
```

`search.toml` 与 `apps.toml` 的 `enabled` 语义不同：前者是应用自身配置项，后者是 host 的自启开关；设置中心主开关需**双写**并保证一致（沿用 `persist_box()` 的 read-modify-write 套路）。

### 8.4 部署与分发

- `dcx-search.exe` 必须与 `dcx-host.exe` **同目录**（`apps.rs::exe_path()` 用 `current_exe()` 换文件名定位）。
- `HKCU\...\Run\DCX` 仍只有一条，指向 `dcx-host.exe --no-settings`；**禁止**为新应用新增启动项。
- 每次替换二进制后重算 `SHA256SUMS.txt`；引擎 DLL 被 Explorer 映射时不可覆盖（先 `mv` 再 `cp`），并用 `grep -a "box_slot.txt" dcx_engine.dll` 校验产物。
- release 无控制台（`windows_subsystem = "windows"`），`println!` 会丢；诊断写 `%LOCALAPPDATA%\dcx\search_trace.log`（≥128 KB 截断）与 `search_panic.log`。

### 8.5 明确不改

`dcx_engine.dll`、`box_slot.txt` / `box_host_alive` / `no_reserve` / `organizer.toml` 线协议、`autostart.rs`、托盘菜单、`box` 的任何行为、以及一切与「收纳盒不移动文件」相关的既有逻辑。

---

## 9. 里程碑与验收

| 阶段 | 交付 | 验收标准 |
|---|---|---|
| **P0 骨架与热键** | 新 crate、注册表接入、默认关闭、host 全局热键、空 UI 壳 | 安装后无 `dcx-search.exe`；`Alt+Space` 在桌面/浏览器/资源管理器下唤起窗口；125% DPI 下位置与点击正确；`stop-dcx.bat` 后无孤儿 |
| **P1 文件名检索** | 卷枚举、`ignore::WalkParallel` 冷启动、内存索引、路径切换、模式切换、分类过滤、流式结果 | 100 万文件索引 ≤ 120 s、内存 ≤ 150 MB；索引未建完即可出增量结果；首批 ≤ 100 ms；连续输入无旧结果回灌；切换范围/模式结果正确 |
| **P2 内容检索与设置** | `grep-searcher` 管线、编码策略、取消、结果操作、设置中心卡片、常驻开关 | UTF-8 / UTF-16 BOM / GB18030 中文各命中；二进制不产生乱码；取消后 100 ms 内收敛；设置项双写一致 |
| **P3 增量与打磨** | `notify` 增量、重命名配对、窗口记忆、右键菜单、拖拽、暗色主题 | 新建/删除/重命名 3 s 内同步；溢出触发局部重扫；UI ≥ 60 fps |
| **P4 可选增强** | NTFS MFT + USN 提权通道、拼音首字母、trigram 内容索引 | 提权失败自动降级到 P3，不弹 UAC 阻塞主流程 |

**跨阶段硬性验证**（沿用收纳盒重写后的经验）：编译通过 ≠ 能用，每个 GUI 改动至少验证 ① 125% DPI 下绘制与点击一致；② 隐藏态不可见区不吞桌面点击（`WM_NCHITTEST` 返回 `HTTRANSPARENT`/区域外为 -1）；③ 真实 `SendInput` 输入 + 截图比对；④ 强杀 host 后无残留进程。

---

## 10. 待决事项（进入 P0 前需拍板）

| # | 问题 | 建议默认值 | 影响 |
|---|---|---|---|
| D1 | 热键组合 | `Alt + Space`，可配 | 与其他启动器冲突时需在设置中心改键 |
| D2 | 默认检索范围 | 「此电脑」（所有固定卷），可切桌面/单盘 | 决定首次索引耗时；网络盘默认排除 |
| D3 | 关闭窗口后进程保留策略 | `release`（立即释放） | `keep_5m` 体验更好但违背「零打扰」的严格解读 |
| D4 | `nucleo-matcher`（MPL-2.0）是否可用 | 可用则用；否则自研加权打分 | 影响排序质量与许可证合规 |
| D5 | 内容检索是否默认包含用户目录外的卷 | 否，仅用户目录 + 手动添加 | 影响全盘 IO 与隐私 |
| D6 | 是否接受 P4 的管理员提权 | 本期不做 | Everything 同样要求服务或管理员 |

---

## 附录 A · 参考数据出处

- Voidtools Everything FAQ：索引耗时（25 万文件约 5 s / 100 万文件约 1 min）、内存与磁盘占用（100 万文件约 100 MB / 45 MB）、USN Journal 增量、内容不索引、`content:` 慢、NTFS 索引需服务或管理员。
- ripgrep `grep-searcher` docs.rs：`SearcherBuilder::binary_detection` 库默认关闭；`memory_map` 默认 `Never` 及大目录 mmap 更慢 / bus error 警告；`encoding` 默认 BOM sniff UTF-8/UTF-16，无 BOM 按 UTF-8。
- ripgrep `ignore` 文档：Windows 隐藏判定同时看 `.` 前缀与 `FILE_ATTRIBUTE_HIDDEN`；`WalkState::{Continue,Skip,Quit}`；`WalkParallel` 基于 crossbeam 无锁工作窃取。
- fd `src/walk.rs`：`BatchSender` 默认 0x100、有界通道 `2 × threads`、`ReceiverBuffer` 100 ms 缓冲、`MAX_BUFFER_LENGTH = 1000`。
- `notify` Windows 后端：`ReadDirectoryChangesW` + 16 KB 缓冲 + overlapped 完成例程 + alertable wait；大目录监听可能丢事件。
- Microsoft Learn `FSCTL_READ_USN_JOURNAL` / 变更日志标识符：USN 操作需管理员权限；`ERROR_JOURNAL_ENTRY_DELETED` 等错误码语义。
- `nucleo` README 基准：Linux 内核源码语料，`never_matches` 约 2.30 ms（skim 约 17.44 ms）——项目自测数据，非第三方基准，仅作选型参考。

## 附录 B · 现有工程锚点速查

| 主题 | 位置 |
|---|---|
| 应用注册表 / 5 s 看门狗 | `crates/dcx-host/src/apps.rs:24-43, 168-229` |
| 设置中心「桌面整理」页 | `crates/dcx-host/src/settings_app.rs:1023-1074, 1141-1226, 1607` |
| 配置原子替换 | `apps.rs:94-101`、`dcx-settings/src/lib.rs:247-269`、`dcx-organizer/src/lib.rs:377-393` |
| 分类规则 / 桌面路径 | `dcx-organizer/src/lib.rs:240-285, 435-446` |
| 单实例 / DPI / panic hook | `crates/dcx-box/src/main.rs:39-116` |
| 每帧 DPI 同步 | `crates/dcx-box/src/app.rs:961-976`；`win.rs:26-33, 173-197` |
| 主题与字体 | `crates/dcx-box/src/view.rs:144-198`；`ui.rs:29-57` |
| 隐藏窗口 / 点击穿透 | `crates/dcx-box/src/app.rs:939-944`；`win.rs:526-578` |
| 自启 | `crates/dcx-host/src/autostart.rs:10-12, 22-25` |
| 分发 | `D:\dcx-dist\{start-dcx.bat, stop-dcx.bat, SHA256SUMS.txt, 说明.txt}` |

---

## 附录 C · 初稿实现记录（2026-11-05）

### C.1 已落地的文件

**新 crate `crates/dcx-search/`**：`Cargo.toml`、`build.rs`（独立 PE 图标/版本块）、
`main.rs`（DPI、`DCX_SEARCH_SINGLETON`、panic hook、`--help`）、
`config.rs`（`search.toml` 原子读写）、`model.rs`（`Ev`/`Hit`/`Query`、7 类分类）、
`index.rs`（卷枚举、`WalkParallel` 冷启动、紧凑字节池索引、`notify` 增量）、
`query.rs`（通配符/AND/`ext:`/`path:`/`regex:`、内容检索 `Sink`、`generation` 取消）、
`app.rs`（唯一事件循环、防抖、批次消费、快捷键、窗口隐藏/前置）、
`view.rs`（`Skin`、标签栏、路径下拉、文件名/内容分段、分类行、虚拟列表、状态栏）、
`ui.rs`（CJK 字体、图标）、`win.rs`（Win32 辅助与日志）。

**host 侧**：`apps.rs`（`AppEntry.default_on`、`REGISTRY` 加 `search`、`launch_on_demand`）、
`hotkey.rs`（`HWND_MESSAGE` + `RegisterHotKey` + `WM_HOTKEY` + 1 s 改键轮询）、
`main.rs`（`mod hotkey`、启动调用、`--help`）、`settings_app.rs`（`SearchValues`、
以 `toml::Value` 做 read-modify-write 的双写、220 ms 防抖、「文件检索」设置卡片）。

**资源与分发**：`tools/gen_search_icon.py` → `brand/embed/search-64.rgba` +
`brand/search.ico`；`stop-dcx.bat`、`start-dcx.bat`、`说明.txt`、`README.md` 同步。

### C.2 编译验证

`cargo check -p dcx-search -p dcx-host` 与
`cargo build --release -p dcx-search -p dcx-host` **均通过**（release 耗时 2 m 52 s，
新 crate 零警告；host 剩 3 条既有代码的 warning）。修复了 5 处编译错误：
`grep_searcher::Encoding` 包装类型、`Index::remove` 借用冲突、
`WalkParallel` FnMut 闭包里 `Arc` 的移动、`picked` 部分移动、`Searcher` 可变借用。

产物：

| 文件 | 大小 | SHA256 |
|---|---|---|
| `D:\cargo-dbx-target\release\dcx-search.exe` | 6,118,912 B | `344acce71c7c7a63dae1cded58c6865a16b7d18e203acce86897dd4c947a83e7` |
| `D:\cargo-dbx-target\release\dcx-host.exe` | 4,726,272 B | 随本次构建更新 |

release 构建成功也意味着 `build.rs` 找到了 Windows SDK 的 `rc.exe` 并嵌入了
`brand/search.ico`（没有 `cargo:warning` 降级提示）。**尚未做实机运行验证**。

### C.3 与 PRD 的偏差（初稿取舍，均不改变需求）

1. **标题栏**：P0 用 `with_decorations(true)` 系统标题栏（PRD §4.2 明确允许 P0 这样做），
   标签栏自绘；P1 再换无边框自绘标题带。
2. **结果列表**：用 `ScrollArea::show_rows` 真虚拟列表，未手写（PRD §4.5 手写的原因是
   行拖拽会吞事件，本期无行拖拽）。
3. **窗口前置**：host 写 `search_show` 标记文件，应用轮询后 `FindWindowW` +
   `SetForegroundWindow`；未发 `WM_DCX_SEARCH_SHOW`（PRD §3.3 的 PostMessage 方案）。
   文件 IPC 与 `box_ui.txt` 既有约定一致，且避免子类化 winit 窗口过程。
4. **`AppEntry` 只加 `default_on`，未加 `activation`**：语义等价 —— `enabled=true` 即常驻，
   看门狗按 `enabled` 保活；`launch_on_demand` 不受 `enabled` 影响，退出后不会被重新拉起。
5. **排序**：未引入 `nucleo-matcher`（MPL-2.0 待法务确认），改用自研加权打分
   （前缀 + 路径深度 + 名称长度）。
6. **内容高亮**：用 `〖命中片段〗` 文本标记代替富文本渲染（安全处理 UTF-8 边界）。

### C.4 明确未实现（后续阶段）

- `residency` 仅 `release`（关窗即退出，索引丢弃）；`keep_5m`、常驻后台预热未做。
- 拼音首字母、trigram/倒排内容索引、NTFS MFT + USN 提权通道、网页搜索标签。
- 自定义路径的文件夹选择对话框（`scope.custom_roots` 需手改 TOML 后出现在下拉）。
- 结果项拖出到资源管理器、按行跳转到编辑器、窗口尺寸/位置记忆的完整闭环。
- `notify` 重命名配对为简化实现；缓冲溢出仅提示重建，未做自动局部重扫。
- 内容检索 `auto` 编码在中文关键词首次无命中时回退 GB18030，混合编码目录可能误报。

### C.5 部署状态

本轮**未覆盖** `D:\dcx-dist` 下正在运行的二进制，只改了脚本与说明。
`dcx-search.exe` 需与 `dcx-host.exe` 同目录分发，并在 `SHA256SUMS.txt` 补一行后重算。
`stop-dcx.bat` 已追加 `taskkill /f /im dcx-search.exe`，避免强杀宿主留下孤儿索引进程。

### C.6 必须做的实机验证（编译通过 ≠ 能用）

沿用收纳盒重写的经验，GUI 改动必须实测：① 125% DPI 下绘制与点击一致；
② `Alt+Space` 在桌面、资源管理器、浏览器下分别唤起，二次按下前置；
③ 首个 100 万文件索引的耗时/内存、首批结果延迟；④ 中文 UTF-8/UTF-16/GB18030
三类文件内容命中；⑤ 快速连续输入无旧结果回灌；⑥ 强杀 host 后无残留进程。
