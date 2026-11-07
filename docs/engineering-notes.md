> 工程实现与调试记录（历史文档；部分数字反映开发过程）。安装、使用与构建请看根目录 [README](../README.md)。

# DCX — Rust 桌面增强平台

用 Rust 重做的桌面视觉增强产品骨架。目标不是复刻 Windhawk，而是用同一批效果、更小的范围、可长期扩展的架构做出自己的产品。

## 1. 组成

| 模块 | 作用 | 注入 |
|---|---|---|
| `dcx-host` | **平台宿主**：任务栏注入与看门狗、系统托盘、统一设置中心、启动/停止独立应用 | 否 |
| `dcx-box` | **独立应用**：桌面收纳盒，独立进程、独立图标与界面（任务栏按钮 + 弹层） | 否 |
| `dcx-search` | **独立应用**：文件检索（`ignore` 并行遍历 + 内存名称索引 + `grep-searcher` 内容流式检索）；默认关闭，由 host 全局热键唤起 | 否 |
| `dcx-island` | **独立应用**：桌面信息岛（时间、性能采样、任务提醒、图文剪贴板、通知聚合）；默认关闭，屏幕顶部悬浮，**不占任务栏槽位、不改引擎线协议** | 否 |
| `dcx-island-config` | 共享配置模型（`island.toml`）+ Windows 凭据管理器封装；宿主设置窗口与信息岛进程共读 | — |
| `dcx-engine` | 可选任务栏模块：注入 Explorer，修改任务栏 XAML；读取设置并热重载 | 是（仅 Explorer） |
| `dcx-settings` | 任务栏设置模型 + TOML 持久化（设置中心写、引擎读，文件即 IPC） | — |
| `dcx-organizer` | 收纳盒数据层：分类规则、manifest、桌面扫描（无 UI） | — |

平台与应用是独立进程：应用崩溃不影响注入、托盘与设置中心；宿主退出时统一停止应用。
任务栏改造在独立 DLL 内，与两者都解耦。**所有设置仍集中在 `dcx-host` 的一个窗口里**，
按应用分页；应用自身不带设置窗口。

新增一个独立应用 = 加一个 crate + 在 `dcx-host/src/apps.rs` 的注册表里加一行
`AppEntry` + 一组设置卡片，不需要改设置窗口的导航或状态字段。
`AppEntry.default_on` 决定缺省启停：收纳盒为 `true`，文件检索与桌面信息岛为 `false`
（不自启、不常驻、不预建索引 / 不创建 topmost 窗口），它们的全局热键由常驻的 host
在 `hotkey.rs` 里监听。信息岛的主开关同时写 `island.toml` 与 `apps.toml`，与收纳盒、
文件检索保持「一应用一开关」的约定。
完整需求与方案见 [`prd.md`](prd.md) 与 [`island-app-plan.md`](island-app-plan.md)。

## 2. 架构

```
dcx/
├── crates/
│   ├── dcx-win32/          手写 Win32 / GDI / DWM 绑定
│   ├── dcx-core/           平台层：事件队列、Effect trait、帧循环
│   ├── dcx-effect-corners/ 效果：统一窗口圆角
│   ├── dcx-theme/          WindowGlass Split 主题数据与规则解析
│   ├── dcx-xaml/           XAML 视觉树 / 属性系统绑定
│   ├── dcx-blur/           WindhawkBlur 合成画刷
│   ├── dcx-engine/         注入 Explorer 的任务栏主题引擎（含设置热重载）
│   ├── dcx-settings/       任务栏设置模型 + TOML 持久化（设置中心/引擎共享）
│   ├── dcx-organizer/      收纳盒数据层（规则、manifest、桌面扫描）
│   ├── dcx-host/           平台：注入、托盘、统一设置中心、应用注册表、全局热键
│   ├── dcx-box/            独立应用：桌面收纳盒（独立 exe / 图标 / 界面）
│   ├── dcx-search/         独立应用：文件检索（名称索引 + 内容流式检索）
│   ├── dcx-island/         独立应用：桌面信息岛（胶囊 + 面板同一窗口）
│   └── dcx-island-config/  island.toml 模型 + 凭据管理（host 与 island 共享）
├── tools/                  构建、注入、截图脚本（含 gen_search_icon.py / gen_island_icon.py）
└── docs/                   prd.md / island-app-plan.md / review.md
```

进程与文件边界（全部在 `%LOCALAPPDATA%\dcx`）：

```
dcx-host.exe ──启动/停止──> dcx-box.exe ──心跳──> box_host_alive
     │                          ▲                      │
     │ 注入 dcx_engine.dll      │ 读 organizer.toml    │ 判活
     ▼                          │ 读 box_slot.txt      ▼
explorer.exe ──写 box_slot.txt──┘                 dcx-engine::slot
     ▲
     └── 读 settings.toml（由 dcx-host 的统一设置窗口写入）
```

`settings.toml`（任务栏主题）、`organizer.toml`（收纳盒）、`apps.toml`（应用启停）
三份文档各自独立；`box_slot.txt` / `box_host_alive` / `no_reserve` 是引擎与收纳盒
之间的**线协议，文件名保持不变**，所以拆分不需要替换已被 Explorer 映射的引擎 DLL。

`search.toml` 是文件检索与 host 共读的文档；`island.toml` 是桌面信息岛与 host 共读的
文档（两者写入一律走 `config_file::FileLock` + `write_atomic`），均**不属于**引擎线协议。
三个独立应用各有一个全局热键，都由 `dcx-host/src/hotkey.rs` 的 `HWND_MESSAGE` 隐藏窗口
承载 `WM_HOTKEY`：收纳盒读 `organizer.toml` 的 `hotkey`（默认 `Alt+d`，按下写
`box_show`），文件检索读 `search.toml` 的 `activation.hotkey`（默认 `Alt+s`，按下写
`search_show`），信息岛读 `island.toml` 顶层 `hotkey`（默认 `Alt+i`，按下写
`island_show`）。应用轮询各自的标记文件来唤出/收起窗口；热键被占用时写
`box_hotkey_error.txt` / `search_hotkey_error.txt` / `island_hotkey_error.txt`，设置中心
对应页面会提示。默认 `[apps.search] enabled=false`、`[apps.island] enabled=false`，因此
文件检索不常驻、不预建索引，信息岛不创建常驻 topmost 窗口。**信息岛热键额外要求
`island.toml enabled=true` 且 `apps.toml` 也为 true**，避免一个已关闭的应用被键盘复活。

`island_tasks.json` / `island_clipboard.json` 是信息岛独占的数据文件；IMAP 授权码、
SMTP 授权码与 Webhook Key 存 Windows 凭据管理器（目标名 `DCX/island/<key>`，由
`dcx-island-config::credential` 统一读写），**不落 TOML、不落日志**。信息岛的诊断输出写
`island.log`（256 KB 截断轮转），panic 写 `island_panic.log`。

数据流：

```
WinEvent(跨进程)
      │  callback 只入队，不做逻辑
      ▼
   QUEUE: Mutex<Vec<WindowEvent>>
      │  WM_TIMER (~8 ms) 时排空
      ▼
  Effect::on_event
      │
      ▼
  Effect::on_tick   （需要逐帧推进的效果）
```

`Effect` trait 是扩展契约：

```rust
pub trait Effect {
    fn name(&self) -> &'static str;
    fn on_event(&mut self, ev: WindowEvent, ctx: &EffectCtx) -> bool;
    fn on_tick(&mut self, ctx: &EffectCtx) {}
    fn shutdown(&mut self) {}
}
```

## 3. 当前效果

| 效果 | 状态 | 说明 |
|---|---|---|
| 任务栏 WindowGlass Split 主题 | ✅ | `dcx-engine` 注入 Explorer，应用 XAML 规则 |
| 右胶囊时钟（日期折叠/时间 13px） | ✅ | 主题规则 + 动态回写 |
| 统一窗口圆角 | ✅ | `dcx-host` + DWM 属性；只作用于常规应用窗口，shell 弹层跳过（见 3.1） |
| macOS 最小化动画 | ❌ 已移除 | 移植效果不达标，代码与 Hook 已全部删除 |
| 任务栏 Dock 式悬停放大 | ✅ | `dcx-engine::hover`，逐帧改写每个任务按钮的 `CompositeTransform`（见 3.2） |
| 桌面收纳盒 | ✅ | 独立应用 `dcx-box.exe` + `dcx-engine::slot` + `dcx-host::apps`，任务栏上固定一个按钮，按扩展名给桌面文件分类（见 3.3） |

任务栏灰线处理：
- `Taskbar.TaskbarFrame > Grid#RootGrid` 的 `BorderThickness=0`，去掉胶囊描边。
- `Taskbar.TaskbarBackground#BackgroundControl` 折叠，去掉全宽默认背景层。
- `Shell_TrayWnd` 设置 `DWMWA_BORDER_COLOR = 0xFFFFFFFE`，去掉 DWM 非客户区边框。

### 3.2 Dock 式悬停放大

设置项 `hover_effect`（设置 → 任务栏样式 → 悬停效果），两档：

| 取值 | 行为 |
|---|---|
| `Background` | 原样：图标不缩放，悬停只显示底板高亮 |
| `Dock`（默认） | 图标按距离放大并推开相邻图标，同时隐藏按钮底板（`Border#BackgroundElement` 的 `Opacity=0`），只留图标本身 |

改设置后 1 秒内热生效：引擎监听 `settings.toml` 的 mtime，重载时额外调用 `hover::on_settings_changed()`
重建几何表并重新应用底板显隐。

`crates/dcx-engine/src/hover.rs`。指针停在任务栏上时，每帧按光标到每个任务按钮中心的距离
算缩放（余弦窗，±2.8 个图标），把放大后的宽度重新排一遍得到位移，再把整排锚定在光标处——
于是指针下的图标原地变大、两侧被推开。离开后 6 ms 定时器停掉，静止时一次写入都不产生。

实现上有四个坑，都在代码注释里留了原因：

* 从 `<Style>` setter 取到的 `CompositeTransform` 是**密封**的，`put_ScaleX` 返回 `S_OK` 却什么
  都不改。必须 `XamlReader.Load` 一个临时 `<Grid>` 再取它的 `RenderTransform`，拿到的才是可
  变实例。
* `RenderTransformOrigin` 是 `Point`（两个 `f32`，x64 下走 XMM 寄存器），不能按 `fn(this, u64)`
  传。改用 `CompositeTransform.CenterX/CenterY`，并且取**图标中心**（不是按钮底边）：从底边
  放大时整块图标会离开这一排的光学中线，看起来是"歪"而不是"大"。
* 居中放大是上下双向生长的，所以峰值上限由栏高决定：`base × peak × icon_h ≤ 栏高`。`icon_h`
  取按钮里 `Image#Icon` 的实际高度（24 DIP），不是按钮高度（42 DIP），否则上限会被算小一半。
  触到上限时整体降低放大曲线，而不是让单个图标被裁。
* `UIElement.ActualOffset` 是**相对父元素**的，要沿祖先链求和才是屏幕坐标，否则第一个按钮会
  报 44.8 DIP（其实是相对 ItemsRepeater 的位置）。
* 求和的前提是"每个 handle 只有一条记录"：TAP 在每次重排都会重报元素，旧记录保留着**过期的
  parent**，沿着它求和会得到 `x ≈ -9672` 这种坐标——所有图标中心因此离光标上万像素，衰减系数
  全为 1，表现为"放大完全没反应"。`record_element` 现在按 handle 覆盖（不再追加）。
* 同理，`ItemsRepeater` 会把回收的任务按钮停在屏幕外（实测 x ≈ -9700 DIP）。它们宽度正常、
  能通过筛选，排在队首后同样会把整排坐标带偏。几何表按 `0 ≤ x ≤ 栏宽` 过滤掉这些元素。
* 锚点必须对光标做连续插值。按"最近的那个图标"取锚点，指针跨过图标边界时整排会瞬间跳一个
  图标宽度——这就是最初的"全部图标抖动"。

上限说明：任务栏的胶囊背景**就是**窗口本身，窗口边缘即硬裁剪，所以图标只能在栏内放大到把
栏填满（约 1.55×，配合 `icon_scale` 实际 ~1.75×），无法真正悬在栏外。让图标挡在栏外需要
自绘覆盖层：另开分层窗口画 128 px 图标并转发命中，属于独立工作量。
曾试过两条替代路径并**已回退**：加高 `Shell_TrayWnd`（XAML 宿主子窗口不变，超出仍被裁；
连子窗口一起加高会让任务栏整块不渲染）、悬停时收窄胶囊背景（所有图标都跟着露出，观感更差）。

### 3.3 桌面收纳盒

任务栏上一个固定按钮，点开是「桌面 + 收纳库」两层的文件面板：桌面上的文件仍由 Explorer 原生显示，收进盒子的文件被**物理移入**独立的收纳库目录。

**2026-11-04 重写：** 收纳盒的 UI 与窗口管理被完整重写，修掉两个结构性问题。

| 症状 | 根因 | 现在的做法 |
|---|---|---|
| 界面拖不动 | 面板是 eframe 的 **deferred viewport**，每帧用异步 `ViewportCommand` 重定位；窗口在指针下方移动，egui 局部坐标自我反馈，`drag_started()` 丢失；拖动状态机又跑在会被 winit 节流的第二个帧循环里 | **单窗口 + 单 egui 上下文**：`SetWindowPos` 同步定位；用 OS 光标 + `VK_LBUTTON` 在**同一个**帧循环里驱动拖动；拖动期间冻结窗口尺寸（只平移、不重建透明 surface），tile 暂停绘制；拖动结束才恢复紧凑矩形 |
| 任务栏按钮漂移 | 按钮 x 取 `slot.left`（最后一个程序图标的右边缘）。开关程序、引擎写 52 DIP 边距改变胶囊宽度，都会让它变 | 默认锚定 `slot.limit`（**托盘左边缘**）——与图标数量无关；只有任务栏矩形变化才立即重新锚定，托盘宽度抖动需持续 800 ms 才采纳；`TileMode::Fixed` 可把绝对物理像素位置锁死 |
| 按钮飘到桌面上方、面板只有 80 % 大、点击/拖动对不上（125 % DPI 实机验证发现） | `ctx.set_pixels_per_point(1.0)` 把 egui 的点当成**物理像素**，但 `GetWindowRect` / `box_slot.txt` 的数值全部除以 `scale` 变成**逻辑像素**；`SetWindowPos` 又按物理像素摆放窗口。绘制坐标系与布局坐标系差了一个 DPI 倍数，越靠近屏幕下沿偏差越大（125 % 下按钮被画到槽位上方约 190 px） | `app.rs::update` 每帧把 `pixels_per_point` 同步为 `scale_for(hwnd)`；Rect（逻辑像素）= egui 点，窗口（物理像素）= Rect × scale，三处单位统一 |

**2026-11-04 实机复核**：在 1920×1080 / 125 % / 双显示器（副屏在主屏上方）下用真实鼠标输入验证：
按钮落在任务栏预留槽位内且与其他图标齐平；拖动标题栏 150×70 物理像素，`box_ui.txt` 记录的
`panel_x/panel_y` 精确变化 120×56 pt（= 物理像素 ÷ 1.25）；连续 5 s 采样 `tile_x` 恒为 1193，
而引擎 `box_slot.txt` 的 `seq` 持续递增；把 `tile_offset` 改为 −60 pt 后按钮左移 75 物理像素，
改回 0 又精确回到 1193（证明 800 ms 滞回不会挡住用户主动改位置）。标签切换、右键菜单、
右下角改大小均命中正确。

窗口矩形是「面板 ∪ 按钮」的并集，中间大片透明区域原本会吞掉桌面点击：现在用
`WM_NCHITTEST` 子类，对两个矩形之外的像素返回 `HTTRANSPARENT`。列表也换成手写虚拟
滚动，`ScrollArea` 会把行拖拽当成「拖动滚动」吃掉。

| 模块（重写后） | 职责 |
|---|---|
| `dcx-organizer` | 规则、manifest、桌面扫描、可见性策略、忽略规则、排序/过滤/搜索、重命名与清单迁移；纯数据层 |
| `dcx-box::win` | Win32 窗口层：DPI、槽位文件、任务栏几何、`SetWindowPos`、命中穿透、DWM、心跳与运行时状态文件 |
| `dcx-box::model` | 面板状态与业务动作：虚拟标签、多选、菜单、命令队列、偏好持久化 |
| `dcx-box::view` | 全部绘制：亮/暗皮肤、标题栏、搜索、标签、工具栏、列表/网格、菜单、对话框、拖动幽灵 |
| `dcx-box::app` | 唯一的事件循环：几何、悬停/点击、键盘、拖放、命令执行 |
| `dcx-box::desk_place` / `file_icon` / `shell_io` / `ui` | 桌面图标定位 / 真实文件图标 / 打开·属性·回收站·重命名 / 字体与应用图标 |
| `dcx-host::settings_app` | 设置中心：「任务栏样式 / 桌面整理 / 文件检索 / 桌面信息岛 / 关于」五页；各应用的启停、状态与选项都在自己的页内 |
| `dcx-engine::slot` | 量测任务栏 XAML 并预留一格（**线协议未改，引擎无需重新注入**） |

旧实现整份保留在 `crates/dcx-box/src/box_ui.rs.bak`，便于对照。

**产品逻辑：桌面保持原生，收纳库是物理目录。** 不隐藏桌面图标，也不自绘桌面层。
面板的「桌面」页列出真实桌面目录里尚未收纳的顶层项目；收纳（右键「收纳到盒子」、
拖到分类/正文、或 `⋯` 菜单的「自动收纳桌面」批量确认）会**把文件移进收纳库**（默认
`文档\dcx收纳`，可在设置 →「桌面整理」→「收纳库目录」改，改路径时会把库内文件迁移
到新目录），桌面图标随之消失，文件本身不删除。分类只是
收纳库内的虚拟元数据：扩展名规则自动归类，手动拖拽/菜单只写 `organizer.toml` 的
`[overrides]`。把库内项目拖出面板或拖到「桌面」页会**移回桌面根目录**，图标重新出现；
落点由 `desk_place` 处理。库目录若被配置到桌面目录内部，启动时会回退到
`%LOCALAPPDATA%\dcx\library`，避免收纳库自己变成一个桌面图标被反复扫描。

**2026-11-05：文件夹是一等公民。** 「文件夹」是内置分类（key `folder`，旧配置在
`repair()` 里自动补上，不可删除）。文件夹行带展开箭头，展开后按当前排序在行内显示
子项（目录树最多 6 层、每层最多 500 项；搜索态保持扁平）。展开本身只读目录，不移动
任何东西；**把文件拖到文件夹行上才是物理移动**（同名自动编号、跨卷回退为复制+删除）。
库内子项拖出面板会移回桌面根目录。右键补齐了展开/折叠、打开文件夹、新建子文件夹、
移入文件夹（库内文件夹列表或系统选择器）、放回桌面、空白区新建文件夹/全选/刷新；
`Ctrl+A` 全选。

**2026-11-05：拖入/拖回补齐。** 外部拖入（资源管理器、桌面文件、其它程序的
`CF_HDROP`）按落点分流：文件夹行 = 物理移入；分类标签 = 移入收纳库并指定分类；
列表正文、「全部」「最近」= 移入收纳库并按扩展名自动分类。`.lnk`、文件、文件夹一视同仁。

**2026-11-05：移除桌面层与整层隐藏。** 期间曾尝试「隐藏 Explorer 图标层 + 自绘桌面
层」，因系统图标、点击穿透和多屏问题被放弃，代码已删除。现在不隐藏桌面图标、不改任何
文件属性；老版本留下的 `box_icons_owned` 标记会在启动时恢复图标后清掉。桌面既然恢复
原生，原来的「系统」标签页（shell 图标视图、快捷方式收纳、清空回收站）也一并移除。

**为什么位置仍要由引擎给。** Win32 拿不到「最后一个程序图标之后、托盘之前」这个位置：
`ReBarWindow32` / `MSTaskSwWClass` 的宽度停在 shell 改用 XAML 之前（实测右边缘 951 px，
最后一个图标实际结束在 1170 px），`MSTaskListWClass` 的 `TB_BUTTONCOUNT = 0`。引擎仍在
XAML 里量测并写 `box_slot.txt`（格式与采样频率不变），同时给 `TaskbarFrameRepeater`
加 52 DIP 右边距预留空位。

**变化在于怎么用这些数字。** 旧版直接把按钮放在 `left` 之后 —— 那是全任务栏最不稳定的
坐标。新版默认用 `limit`（托盘左边缘）：

```text
TileMode::Tray   （默认） left = limit - 44 - 4 + tile_offset   固定，图标增减不动
TileMode::Follow （兼容） left = left + 6 + tile_offset          跟随图标行末尾
TileMode::Fixed  （锁定） left = tile_fixed_x / dpi + tile_offset 绝对物理像素
```

`app.rs::compute_geo` 里的锚点稳定器：任务栏矩形变化（分辨率、DPI、自动隐藏滑出、任务栏
换边）才允许立即重算；`Tray` 模式下 > 2 pt 的目标变化必须连续稳定 800 ms 才采纳；
`Fixed` 模式任何测量都不改变 x。设置 →「桌面整理」→「任务栏按钮位置」提供三选一、
−80…80 pt 微调、以及「锁定到此处」（读 `box_ui.txt` 里 box 每秒发布的 `tile_x`）。

**按钮上右键即可锁定**：菜单里有「锁定当前位置 / 解除位置锁定」「锚定托盘左侧」「跟随最后
一个程序图标」「微调位置 / 更多设置…」。居中任务栏在图标增减时，预留槽位本身会跟着胶囊变宽
（实测 13→14 个图标时 `slot.limit` +26 物理像素），`Tray` 模式会如实跟随；想完全不动就用
右键「锁定当前位置」，它把当时的 `tile_x` 物理像素写进 `tile_fixed_x` 并切到 `Fixed`。

面板关闭时窗口仍是「面板 ∪ 按钮」的并集；命中矩形这时只放行按钮，其余全部 `HTTRANSPARENT`，
否则那块不可见的大矩形会吞掉桌面点击。

**弹层完整能力**

| 能力 | 实现 |
|---|---|
| 按钮位置 | 按钮右键菜单：锁定当前位置 / 解除锁定 / 托盘左侧 / 跟随图标末尾 / 跳到设置中心；三选一与微调也在设置中心；锁定值存 `tile_fixed_x`（物理像素） |
| 按钮尺寸 | 跟随任务栏可见高度（引擎应用的 XAML 行高 30–44 DIP）：按钮窗口和 hover/选中底板同步缩放，底板再向内缩 1 DIP，不会越出任务栏 |
| 按钮层级 | 窗口 owner 挂到 `Shell_TrayWnd`：开始菜单把任务栏提到更高 z 序带时抽屉图标仍在任务栏上可见；自动隐藏滑动期间临时解除 owner，由任务栏监视线程直接移动窗口（否则 owner 动画会让抽屉慢一帧），停稳后重新挂回；Explorer 重启销毁 owner 后自动重新挂接/重建 |
| 可拖动 / 可改大小 | 标题栏是拖拽手柄（位移 > 4 pt 起拖），右下角手柄改宽高；落点与尺寸落 `box_ui.txt` / `organizer.toml`；**双击标题栏**回到按钮上方；header 图钉按钮固定面板不被 light-dismiss |
| 搜索 | 顶部搜索框在**当前页**实时过滤（「桌面」页搜桌面，「全部」/分类搜收纳库；空格分词 AND，匹配名称/扩展名/路径），`Ctrl+F` 聚焦，`Esc` 清空 |
| 虚拟标签 | 「桌面」「全部」「最近 7 天」+ 配置里的所有分类；标签带计数、可横向滚动、`＋` 就地新建分类 |
| 排序与视图 | 名称 / 修改时间 / 大小 / 类型，升降序切换；列表（图标+名称+大小+时间）与网格（大图标）两种视图；手写虚拟滚动 + 可拖拽滚动条 |
| 多选与键盘 | `Ctrl`/`Shift` 点击、`Ctrl+A` 全选、`Ctrl+C` 复制路径、`↑↓` 导航、`Enter` 打开、`F2` 重命名、`Delete` 删除、`Esc` 关闭 |
| 文件夹 | 内置「文件夹」分类；行内展开/折叠（`▸`/`▾`，最多 6 层、每层 500 项）；子项可打开、复制路径、重命名、删除；网格视图里右键「展开」会切回列表 |
| 右键菜单 | 打开 / 展开折叠 / 新建子文件夹 / 在资源管理器显示 / 属性 / 复制路径或名称 / 重命名 / 移入文件夹（库内列表 + 选择器）/ 归入任一分类 / 清除手动分类 / **桌面项「收纳到盒子」・库内项「放回桌面」** / 删除到回收站 / 全选 / 打开桌面或收纳库；空白区另有刷新、新建文件夹、全选、自动收纳桌面；多选自动变成批量操作 |
| 拖拽 | 桌面项拖到分类标签 = 移入收纳库并指定分类；拖到正文 = 移入库并自动分类；库内项拖到分类标签 = 只改归类（文件不动）；拖到文件夹行 = **物理移入该文件夹**；库内项拖出面板或拖到「桌面」页 = 移回桌面（落点跟随鼠标）；从资源管理器/桌面拖入 = 文件夹行物理移入、分类标签指定归类、正文自动分类（`.lnk` 同文件）；拖动时有跟随光标的幽灵卡片 |
| 安全删除 | `SHFileOperationW(FO_DELETE, FOF_ALLOWUNDO)` → 回收站可还原；默认弹确认框，同步清理 manifest |
| 底部按钮 | 左：当前页 · 项数 · 总大小（选中时显示选中数）；右：**「收纳」单击复制收纳库路径、双击打开收纳库**；「重新分类」清空手动分类 |
| 其它 | 真实 shell 文件图标（按扩展名/路径缓存）、亮暗主题跟随系统、忽略规则（`*.tmp`、`~$*`…）、可选显示系统隐藏项、可选单击打开、记住上次分类、Toast 反馈 |
| 桌面与收纳库 | Explorer 桌面始终原生显示；收纳 = 移入 `library_root`（默认 `文档\dcx收纳`），放回 = 移回桌面根目录；不改文件属性、不隐藏图标层；旧 `hidden` 清单首次启动自动恢复并清空，`box_icons_owned` 标记自动归还图标 |

**交互**：悬停 110 ms 或点击展开；点按钮／点面板外／Esc 关闭；右上角 `⋯` 是视图与行为菜单，
`↻` 刷新，`⌃` 收起，图钉固定。所有改动即时热加载（设置中心与面板共用 `organizer.toml`，
各自的写都走读-改-写，互不覆盖：设置中心 `persist_box` 会合并面板写的视图偏好）。

已知限制：只处理主任务栏；引擎未注入时按钮位置退化为 `ReBarWindow32` 估算；任务栏自动隐藏
时按钮随栏一起消失；`WM_NCHITTEST` 子类若安装失败会每帧重试，此时透明空白区暂时仍会吞点击
（功能不受影响）。

**三个踩过的坑（都已修，别再踩）：**

1. **单位混用会把整个槽位判定丢掉。** `box_slot.txt` 里全是物理像素（引擎在设备像素里量），
   而宿主内部的 `Rect` 一律是点（px / dpi）。早期版本直接比较 `slot.bar_h - bar.height()`，
   在 125 % 下就是 `60 - 48 = 12`，超过 8 的容差 → 槽位被判为"不是同一条任务栏" → 回退到
   `ReBarWindow32` 估算 → 按钮飘到托盘右端外面（看着就是"没有图标"）。现在比较前先除以 dpi。
   自检：宿主日志里每次启动打印一行
   `[dcx-box] dpi=1.25 tile=[...] slot=Some(Slot { right, limit, bar_h, n })`，`slot=None`
   就说明槽位没读上（引擎没注入 / 文件过期 / 单位又对不上）。

2. **置顶窗口也会被任务栏盖住。** 任务栏本身就是 topmost，而且**每次自动隐藏的栏滑出来时 shell
   都会把它重新提到最前**——于是同样 topmost 的自绘按钮虽然位置完全正确，却被压在栏下面，
   表现为"位置空着、图标看不见、点也没反应"（`PrintWindow` 能画出内容，说明是 z 序不是绘制）。
   现在每帧用 `WindowFromPoint(按钮中心)` 问一句"这块像素现在归谁"，只有被别的窗口遮住时才
   重新 `SetWindowPos(HWND_TOPMOST)`：既不会和 shell 的滑入动画抢，也不会每帧抖动。

3. **桌面图标不能靠 `LVM_SETITEMPOSITION` 摆。** Win11 的桌面列表视图是 **`LVS_OWNERDATA`**
   （`SysListView32` 样式 `0x56003A40`，`LVS_OWNERDATA|LVS_ALIGNLEFT`），位置由 shell 自己维护：
   发 `LVM_SETITEMPOSITION32` 返回 1（成功），但读回来是 `(0,0)`，图标纹丝不动。唯一可用的是
   `IFolderView::SelectAndPositionItems`（`SVSI_POSITIONITEM`），也就是 Explorer 自己拖放时走的
   那条路——`dcx-box/src/desk_place.rs` 里用裸 vtable 走
   `IShellWindows → IServiceProvider → IShellBrowser → IShellView → IFolderView`（`FindWindowSW`
   在 `IShellWindows` 里是槽位 15，`QueryActiveShellView` 在 `IShellBrowser` 里也是 15——少算
   `IDispatch` 的 7 个槽 / 漏掉 `SetMenuSB` 都会拿到 `0x800706D1 PROCNUM_OUT_OF_RANGE`）。
   另外"取消隐藏"之后图标要过约 1.4 s 才进视图，所以定位必须轮询，不能同步取。

排查入口：直接运行 `dcx-box.exe` 只起按钮窗口（不注入、不托盘、不碰资源管理器），
`python tools/box_shot.py --at 1200,1078` 把光标压到任务栏再截图。
平台侧的启停开关在设置 →「桌面整理」页顶部，`%LOCALAPPDATA%\dcx\apps.toml` 是它的持久化；
文件检索的启停与选项在独立的「文件检索」页；桌面信息岛在「桌面信息岛」页，
页面里只有顶部一个「启用桌面信息岛」开关，其余都是模块选项与渠道诊断。

3. **一个进程只能有一个 winit 事件循环（拆分的结构性原因）。** 历史上设置窗口
   与收纳盒面板同处 `dcx-host`：谁先 `run_native` 谁赢，第二个必定失败
   （`EventLoop can't be recreated`），表现为"格子空着、点了没反应"，和位置算错
   完全一样。拆分后每个进程只有一个循环：`dcx-host` 跑 WinEvent/消息循环，
   `dcx-box.exe` 跑自己的 eframe 循环，设置窗口用 `current_exe --settings` 起第三个
   进程——三者互不竞争。页面通过 `DCX_SETTINGS_PAGE` 传，第二个设置实例用
   `FindWindowW("DCX 设置")` 提到前台而不是再开一个。
   中间试过"设置窗口作为主循环的 deferred viewport"，**不要走那条路**：视口里
   悬停高亮正常但 `clicked()` 收不到（侧栏、主题按钮全失效），排查成本高且不可靠。

4. **Dock 放大不能长进预留带。** 按钮是独立窗口，不会跟着图标让位；悬停放大时整排
   会向右扩张，直接压到按钮上（实测最后一个图标被放大后右边缘越过 `slot.left` 约 20 px）。
   引擎在 `hover.rs` 里把位移的右界从"托盘左边缘"改成"整排静止时的右边缘"
   （仅当 `slot::tile_reserved()` 为真），代价是贴近托盘的那几个图标放大时铺开得少一点。
   配套：收纳盒在**预留带还没生效**（刚注入、壳刚重启）时宁可这一帧不画按钮，也不把
   按钮往左夹到图标上——那正是"图标被盖住"的另一半原因。

### 3.1 幽灵窗体（开始菜单 / 时钟弹层后面的空矩形）

现象：打开开始菜单或点任务栏时钟后，弹层后面出现一个比卡片大一圈的空矩形，1 px 灰线、圆角、内部透明——即「幽冥窗体」。

成因（已实测确认）：
- `dcx-effect-corners` 对**每个前台窗口**写 `DWMWA_WINDOW_CORNER_PREFERENCE`（默认 `DWMWCP_ROUNDSMALL`）。
- DWM 对**本来没有边框**的窗口（XAML island：`WS_EX_NOREDIRECTIONBITMAP`）会因圆角请求**合成一条 1 px 边框**，并沿**窗口矩形**绘制。开始菜单/搜索/通知日历这些窗口矩形远大于可见卡片（含阴影留白），于是只剩一个空描边。
- `chrome.rs` 只清 `DWMWA_BORDER_COLOR`，清不掉圆角带来的边框；而且 `EnumWindows` 在本机 10.0.26200 上**枚举不到**这些 shell island（`GetForegroundWindow` / `WindowFromPoint` 却能拿到），所以靠定时扫描补不回来。
- 该属性**粘在窗口上**：设置它的进程退出后边框仍在，直到窗口所属进程结束。所以宿主没运行也会看到。

实测数据（`SearchHost.exe` 的 `Windows.UI.Core.CoreWindow`，1000×938 @ (460,82)）：

| corner preference | 窗口最外两行/列亮度 | 结果 |
|---|---|---|
| 3 (`ROUNDSMALL`) | 168 / 147 / 149 / 121 | 有 2 px 描边 |
| 1 (`DONOTROUND`) | 204 / 173 / 175 / 139 | 与内部一致，无边框 |

修复：
- **效果侧**：`dcx-effect-corners` 只处理常规应用窗口——`Windows.UI.Core.CoreWindow` 等 shell 类、`WS_EX_NOREDIRECTIONBITMAP`、`WS_EX_TOOLWINDOW`、被 cloak、以及没有 `WS_CAPTION` 的窗口一律跳过（`skip()`）。
- **清扫侧**：`chrome.rs` 在清边框色的同时把圆角偏好复位成 `DONOTROUND`，修掉旧版本已经写脏的窗口。
- **一次性修复**：`python tools/clear_shell_frames.py`（`--dry-run` 只看不改）把当前 shell 表面上的两个帧属性复位；只动 shell 类，应用窗口的圆角保留。
- 已经粘上的属性也可以靠重启 shell 宿主清掉：`ShellExperienceHost` / `StartMenuExperienceHost` / `SearchHost` 被杀掉后按需自动重建，窗口一死属性就没。

排查工具：`tools/diag_windows.py`（窗口几何 / 样式 / DWM 属性一览）。

## 4. 加一个新效果

```bash
cargo new --lib crates/dcx-effect-dragsnap
```

```rust
use dcx_core::{Effect, EffectCtx};
use dcx_core::event::WindowEvent;

pub struct DragSnap;

impl Effect for DragSnap {
    fn name(&self) -> &'static str { "dragsnap" }
    fn on_event(&mut self, ev: WindowEvent, _ctx: &EffectCtx) -> bool {
        if let WindowEvent::Foreground(hwnd) = ev {
            // ...
        }
        false
    }
}
```

然后在 `dcx-host/Cargo.toml` 加依赖、`main.rs` 加一行 `.register(...)`。

## 5. 验证

### 5.0 构建环境（Windows / MSVC）

链接和 `cc` 编译 C/C++ 只依赖两块东西：MSVC 工具集与 Windows SDK。

| 组件 | 本机路径 |
|---|---|
| MSVC 14.44 | `C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207` |
| Windows SDK 10.0.26100.0 | `C:\Program Files (x86)\Windows Kits\10`（`Include\` / `Lib\` / `bin\`） |

已在**用户环境变量**里写好 `INCLUDE`、`LIB`，并把 MSVC 与 SDK 的 `x64` 目录前置到 `PATH`：

```
INCLUDE = <msvc>\include;<kits>\Include\10.0.26100.0\{ucrt,shared,um,winrt,cppwinrt}
LIB     = <msvc>\lib\x64;<kits>\Lib\10.0.26100.0\{ucrt,um}\x64
PATH    = <msvc>\bin\Hostx64\x64;<kits>\bin\10.0.26100.0\x64;...
```

有了这些，普通终端直接 `cargo build` 即可，**不需要**先跑 `vcvars64.bat`。

> `PATH` 里必须让 MSVC 的 `link.exe` 排在 Git 自带 `usr/bin/link.exe`（coreutils 的 `link`）之前，否则 rustc 会调到错误的链接器并报 `link: extra operand`。

### 5.1 命令

```bash
# 全量编译（产物：dcx-host.exe / dcx-box.exe / dcx_engine.dll）
cargo build --release

# 只重生成收纳盒独立图标（不动平台主图标）
python tools/gen_brand.py --box-only

# 宿主烟测：跑 1.5s 后干净退出
cargo run --release -p dcx-host -- --selftest

# 任务栏主题：重启 Explorer、注入、输出日志
pwsh -File tools/run.ps1

# 只注入当前 Explorer
pwsh -File tools/run.ps1 -NoBuild -NoRestart

# 任务栏截图
pwsh -File tools/shot.ps1 -Out out/taskbar.png
```

### 5.2 分发包清单（`D:\dcx-dist`）

拆分后分发目录必须放齐四个 exe：少一个，设置中心里对应应用会一直显示
「已停止」；缺 `dcx-search.exe` / `dcx-island.exe` 时按全局热键也没有窗口可唤起。

| 文件 | 说明 |
|---|---|
| `dcx-host.exe` | 平台：注入、托盘、设置中心、应用生命周期、全局热键 |
| `dcx-box.exe` | 独立应用：桌面收纳盒，**必须与 host 同目录**（`apps.rs` 按 `current_exe()` 的父目录找） |
| `dcx-search.exe` | 独立应用：文件检索，同样必须与 host 同目录；默认关闭，热键按需拉起 |
| `dcx-island.exe` | 独立应用：桌面信息岛，必须与 host 同目录；默认关闭，由设置中心显式开启 |
| `dcx_engine.dll` | 注入 Explorer；**线协议未变，新增应用不需要替换它** |
| `wh_injector.exe` | 注入器 |
| `start-dcx.bat` / `stop-dcx.bat` | 启停脚本；`stop` 必须逐个 `taskkill` 三个独立应用，因为强杀宿主不会走 `apps::stop_all()`。两个脚本 2026-11-07 才纳入 `tools/`，与分发目录同源；此前 `stop-dcx.bat` 只存在于 `D:\dcx-dist` 且漏掉 `dcx-island.exe`，强杀宿主后信息岛会留成孤儿进程 |
| `stop-dcx-all.bat` | 手动替换文件前一键退出全部 DCX 进程并检查占用；`dcx_engine.dll` 仍被 Explorer 加载时按提示重启 Explorer（`/y` 免询问） |
| `说明.txt` | 便携包用户说明（构建信息 / 目录内容 / 功能清单 / 排障）。每次替换二进制都要同步；2026-11-07 补齐信息岛、把收纳盒改为物理移动模型、设置中心改五页、设置窗口改独立进程 |
| `SHA256SUMS.txt` | 每次替换二进制后重算；新增 `dcx-search.exe` / `dcx-island.exe` 时也要补一行。**用 LF 行尾**：写成 CRLF 会让 `sha256sum -c` 把 `\r` 当成文件名的一部分而全部报 `FAILED open or read` |

升级注意（本机实测）：

- 引擎 DLL 被 Explorer 映射时不能直接覆盖：先 `mv` 旧文件再 `cp`；替换后用
  `grep -a "box_slot.txt" dcx_engine.dll` 确认新引擎确实带 slot 代码，并重算 `SHA256SUMS.txt`。
- 开机自启仍只有一条 `HKCU\...\Run\DCX` → `"D:\dcx-dist\dcx-host.exe" --no-settings`，
  由宿主按 `apps.toml` 拉起各应用，不要给每个应用单独写启动项。
- 单独调试收纳盒时直接运行 `dcx-box.exe`（不注入、不托盘）；它靠命名互斥体保证单实例，
  设置仍从 `organizer.toml` 读。

## 6. 任务栏设置（Settings）

用户可调的任务栏外观：**形状（胶囊/卡片）**、**图标缩放（50–125 % 连续）**、**图标间距（−6…+6 px）**、**悬停效果**，以及可选的**任务栏高度 / 最大宽度**覆盖。

### 6.1 数据流

宿主（写）与引擎（读）之间**只通过文件通信**，不共享内存、不建管道：

```
dcx-host (egui 设置窗口)
      │  写入 %LOCALAPPDATA%\dcx\settings.toml
      ▼
settings.toml  ←—— 这就是 IPC 边界
      │  引擎每 1s 轮询 mtime
      ▼
dcx-engine (注入 Explorer)
      │  mtime 变化 → 重建规则引擎（build_runtime_theme）+ 强制重放全部规则
      ▼
任务栏 XAML 热更新（无需重新注入）
```

- 宿主崩溃/退出不影响已注入的引擎；引擎退出（Explorer 重启）由宿主看门狗重新注入。
- 文件缺失或解析失败时，引擎与宿主都回退到默认值，互不致命。

### 6.2 设置文件

路径：`%LOCALAPPDATA%\dcx\settings.toml`，示例：

```toml
taskbar_shape = "Capsule"      # Capsule | Card
icon_scale = 100               # 50..125（百分比）；旧版 icon_size 会自动迁移
ui_theme = "Light"             # Light | Dark — 只影响设置窗口本身，引擎忽略
icon_spacing = { between = 0 }
hover_effect = "Dock"         # Background | Dock
taskbar_height = 56           # 整条任务栏高度（DIP），36..72，默认 48
# max_width = 1895             # 可选；缺省用主题基准值
```

映射关系（`dcx-settings` → `dcx-theme::ThemeParams`）：

| 设置 | 取值 | 效果 |
|---|---|---|
| `taskbar_shape` | `Capsule` | `RootGrid` 圆角 `25,5,5,25`；托盘 `5,25,25,5`；不写描边 |
| | `Card` | `RootGrid` 圆角 `8`；不写描边（`Border#BackgroundBorder` 在树里被托盘每个图标共用，写描边会把边框画到每个托盘图标上） |
| `icon_scale` | `50..125`（百分比） | 只改渲染缩放（绕中心 `ScaleTransform`），不隐含任务栏高度；旧版 `icon_size = "Small/Medium/Large"` 在加载时映射为 `85/100/125` |
| `icon_spacing.between` | `-6..6` | 每个任务按钮左右 `Margin`；负数收紧，小图标比例下用得上 |
| `taskbar_height` | `36..72`（DIP） | 整条任务栏（dock + 托盘）的高度：引擎用 `SetWindowPos` 改 `Shell_TrayWnd` 高度，两段 XAML 随之伸缩，不做裁剪 |
| `max_width` | 可选 px | 注入 `TaskbarFrameMaxWidth` 常量；缺省 `1895` |

### 6.3 宿主（设置 UI + 自动注入）

- **系统托盘**：`dcx-host` 启动后在通知区放一个图标。**左键单击只打开设置窗口，右键才弹菜单**（「设置 / 桌面整理 / 退出并恢复任务栏 / 退出」）。窗口已打开时再次左键会把窗口提到前台，不会开第二个。
- **应用生命周期**（`dcx-host/src/apps.rs`）：`%LOCALAPPDATA%\dcx\apps.toml` 记录每个独立应用的启用状态；宿主启动时按它拉起应用 exe，5 s 看门狗负责重启，退出时统一停止。「桌面整理」页顶部的「独立应用」卡片列出所有登记应用（运行中/已停止、启停开关），开关立即起停进程；新增应用只需往注册表加一条 `AppEntry`。开机自启仍只有一条 `HKCU\...\Run\DCX` → `dcx-host.exe --no-settings`，不为每个应用各写一条。
- **设置窗口**：**改完即生效**——每次改动防抖 220 ms 后写文件，引擎约 1 秒内热重载，没有需要记得点的「保存」。
  - 写入采用「临时文件 + 重命名」的原子替换：引擎每秒轮询，直接原地写可能被读到半截文本而回退默认值，任务栏会闪一下。
  - 底部状态行如实显示三态：`已写入 · 引擎约 1 秒内重载`（引擎已注入）/ `已写入配置 · 引擎未注入` / 失败原因。
  - **`重新应用` = 重写文件 + 补注入**：先原地重写 `settings.toml`（内容没变也会 bump mtime，逼引擎重载），再跑一次看门狗式的注入扫描，把掉了引擎的 Explorer 救回来——只改文件在引擎没注入时是什么都不会发生的。注入失败会把原因显示在状态行（最常见的是宿主目录里没有 `wh_injector.exe`）。
  - `最大宽度跟随默认` 是开关：关掉即回到主题基准值 1895 px。
  - 只作用于主任务栏，自动隐藏模式未适配；引擎卸载或 Explorer 重启后工作区会被 Explorer 复位，重新注入时会重新采样基线。
  - `自动隐藏任务栏`（「行为」卡片）走的是 shell 自己的状态：`SHAppBarMessage(ABM_SETSTATE)`，只翻 `ABS_AUTOHIDE` 位，其余位（`ABS_ALWAYSONTOP` 等）原样写回，点下去即时生效。**它不是 `settings.toml` 键**——Windows 本身就保存这个值，写一份镜像只会在用户从系统设置里改过之后对不上；窗口每次重绘回读一次，所以系统那边改了这里也不会停在旧状态。实测 10.0.26200：`Shell_TrayWnd` 矩形在 `(0,1020)-(1920,1080)` 与 `(0,1078)-(1920,1138)` 之间来回移动，不需要重启 Explorer。
  - `开机自启`（同一张卡片）写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 的 `DCX` 值，值为 `"<exe 绝对路径>" --no-settings`：登录后只进托盘、不弹设置窗口。用注册表而不是 `shell:startup` 的 .lnk，是为了不依赖 COM/`IShellLink`，也不往用户启动文件夹里塞一个需要解释的文件。关掉即删值；便携目录被搬走时，宿主启动会顺手把指向旧路径的值改成当前路径（`autostart::refresh_path`），免得开关显示"已开启"却从来没自启过。
  - UI 采用 Windows 11 设置的双栏版式：左侧导航（**任务栏样式 / 桌面整理 / 文件检索 / 桌面信息岛 / 关于**，无品牌行，选中项带 3 px 强调条），右侧固定页标题 + 卡片。「任务栏样式」含「外观」「尺寸与间距」「行为」；「桌面整理」只放收纳盒（「常规」「任务栏按钮」「收纳库与桌面」「分类规则」，收纳库目录可点「更改…」换位置并迁移文件）；「文件检索」只放检索（「常规」「检索范围」「内容检索」）；「桌面信息岛」放「常规 / 系统性能 / 任务与剪贴板 / 通知渠道·入站 / 出站与凭据 / 能力边界」；「关于」放系统信息、平台行为与退出。行式控件为 4 px 圆角、品牌深青 `#0E7C7E` 强调色、Win11 开关/滑块/单选。没有预览区；引擎状态与「已写入」状态放在左栏底部。
  - 每个独立应用在自己的页面顶部显示运行状态与唯一的启停开关：「桌面整理」页的「启用收纳盒」同时写 `organizer.toml` 与 `apps.toml`；「文件检索」页的「启用文件检索（常驻）」同时写 `search.toml` 与 `apps.toml`；「桌面信息岛」页的「启用桌面信息岛」同时写 `island.toml` 与 `apps.toml`。应用之间不共享设置卡片，平台级的「统一启动与看门狗」说明放在「关于」页。信息岛保存时会把 `island.toml` 的 `ui` 表（上次页签、面板尺寸）从磁盘重新读回再写，避免覆盖应用自己写入的几何值；三条凭据走显式「保存 / 清除」按钮，不参与 220 ms 防抖。
- **自动注入**：宿主启动时起一个看门狗线程，把 `dcx_engine.dll` 注入所有 `explorer.exe`；每 5s 轮询，Explorer 重启后自动重新注入（注入前用 toolhelp 快照确认该进程尚未加载 DLL，避免重复注入）。另有一个 3s 轮询的状态采样线程，只供设置窗口显示「引擎已注入 / 未注入」。
- **退出与恢复**（`shutdown.rs`）：两件事要分开看。
  - *关闭设置窗口*（右上 ×）只关窗口：宿主、托盘、看门狗与已注入的引擎都还在，任务栏保持当前外观。
  - *「退出并恢复任务栏」*（托盘菜单，或设置 → 关于 → 退出）才把桌面还回去：暂停看门狗 → `taskkill /F /IM explorer.exe` → 等 shell 自行重启（`AutoRestartShell`，最多 8s；没起来就自己 `start explorer.exe`）→ 宿主退出。
  - 走 shell 重启而不是 `FreeLibrary`：XAML 持有我们在 `InitializeXamlDiagnosticsEx` 注册的 TAP 对象、引擎缓存了 8192 条元素句柄，单独卸载影像会让 Explorer 回调到已释放的页（0xc000027b）。进程一起消失才是干净的。
  - 这也是唯一能让任务栏**外观**回到系统默认的办法：引擎写进去的是 XAML 属性（`Height`/`Margin`/`CornerRadius`/`ScaleTransform`/`Visibility`）和 `Shell_TrayWnd` 的 DWM 边框色，都活在 Explorer 里，宿主退出不会带走；重置或删除 `settings.toml` 也不行（缺省值本身就是一套主题参数）。
  - 设置窗口里的按钮是行内二次确认（点一次变红「确认恢复」，5s 内再点才执行）；托盘菜单项不需要确认。失败时注入会 resume，原因写在窗口底部状态行。

开发时只调 UI，可直接打开设置窗口（无托盘、不注入）：

```bash
cargo run -p dcx-host -- --settings
```

要「宿主 + 设置」一起起（托盘、注入、圆角效果都在，同时弹设置窗口）：

```bash
cargo run -p dcx-host -- --with-settings
```


注入依赖外部预编译的 `wh_injector.exe`（经典 `VirtualAllocEx → WriteProcessMemory → CreateRemoteThread(LoadLibraryW)`）。定位顺序：

1. 环境变量 `DCX_INJECTOR` / `DCX_ENGINE_DLL`；
2. 否则取宿主自身所在目录下的 `wh_injector.exe` / `dcx_engine.dll`。

> 开发约定见 `tools/run.ps1`：引擎 DLL 与注入器部署在 `D:\cargo-dbx-target\release\`，可用上述环境变量指过去。

### 6.4 手动验证清单（需 Windows 实机）

本仓库 CI 仅做编译校验；以下行为需在 Win11 实机核对：

- [ ] `cargo run --release -p dcx-host`（不加 `--selftest`）→ 通知区出现 DCX 托盘图标。
- [ ] 托盘右键 → 设置 → egui 窗口打开；形状切到「卡片」→ 约 1s 内任务栏变为带描边卡片（**无需保存、无需重新注入**）。
- [ ] 图标大小拖到 50 % / 125 % → 任务图标连续缩放，两端不裁切；任务栏高度不受影响。
- [ ] 任务栏高度 36/48/72 → 整条栏（含托盘）同高伸缩，图标始终垂直居中且不裁切。
- [ ] 设置窗口底部状态行为 `已写入 · 引擎约 1 秒内重载`；把 Explorer 杀掉重建、注入失败时应显示 `已写入配置 · 引擎未注入`。
- [ ] `cargo run -p dcx-host -- --settings` → 只弹设置窗口，无托盘、不注入（开发 UI 用）。
- [ ] 杀掉 `explorer.exe` → 宿主看门狗约 5s 内重新注入并恢复主题。
- [ ] 直接手改 `settings.toml` → 引擎下次轮询（≤1s）后热重载。
- [ ] 引擎依赖 XAML 仅在任务栏 UI 线程可写：形状/缩放/间距选择器是否命中真实元素（`RootGrid` / `TaskListButton` / `SystemTrayFrameGrid`）需对照 `docs/evidence/taskbar-tree.log`。
- [ ] 设置 → 关于 → 「恢复并退出」：第一次点击按钮变红为「确认恢复」，5s 内再点 → 资源管理器重启、任务栏恢复系统默认（高度/间距/圆角/工作区）、宿主与托盘图标消失；期间行标题显示「正在恢复任务栏」。

## 7. 已知限制

- `dcx-engine` 仅支持当前 Win11 任务栏视觉树；Windows 大版本更新后需重新核对元素路径。
- 主题引擎与 `dcx-host` 独立：Explorer 重启后必须重新注入。
- 32 位进程不加载 64 位 `dcx-engine.dll`；任务栏本身是 64 位 Explorer，不受影响。
- 动画相关代码（`dcx-effect-genie`、`dcx-genie-hook`）已移除，不再保留兼容层。
- 设置热重载在引擎已注入、且文件被宿主/手动改写时触发；`dcx-host` 退出后引擎不再被重新注入（需重启宿主）。
- 图标缩放锚点（绕中心 `ScaleTransform`）在超大/超小预览下可能与相邻元素重叠，属视觉调参项，需实机微调。
- `dcx-host.exe` / `dcx-box.exe` 的 exe 图标分别由各自的 `build.rs` 调 Windows SDK 的 `rc.exe` 生成；机器上没有完整 SDK 时会跳过（只有警告，不影响其他平台功能的构建），产物就不带图标。
- 独立应用必须与 `dcx-host.exe` 放在同一目录（`apps.rs` 按 `current_exe()` 的父目录找 exe）。分发包里少放 `dcx-box.exe` 时，设置 →「桌面整理」会显示"已停止"，日志给出"找不到 …\dcx-box.exe"，平台其余功能不受影响。

## 8. 品牌资源与图标

品牌资源全部由 `tools/gen_brand.py` 生成，**不要手改 `brand/` 下的产物**——改脚本里的参数再重跑，否则下一轮生成会把改动覆盖掉。

```bash
python tools/gen_brand.py --all     # 生成全部资源（含三套配色备选）
```

- **主色深青**：渐变 `#1F9490 → #0A6E72 → #04363F`，强调色 `#0E7C7E`。
- **标记几何**：两条 45° 胶囊交叉成 X，交叉处挖半径 1.07 倍管径的圆形成月牙缺口；胶囊长 0.620T、粗 0.173T（T 为图标块边长）。比例是从初稿逐像素反解出来的，别按感觉调。
- **尺寸阶梯**：≤ 56 px 用实心 X（月牙在这一档会碎成两个点），16–24 px 再把管径收到 0.196T、拉长到 0.700T。
- **托盘图标不是裸字形**：通知区实际只有 16–24 物理像素，裸 X 会被读成「关闭/无图标」。所以托盘画的是缩小版的 app 图标——深青圆角块 + 白色实心 X，16 px 仍是一个完整的"块"，和任务栏里其它彩色图标同一套语言；深色任务栏用亮一档的青（`TRAY_PAL["dark"]`），否则整块会沉进背景。

产物与消费方：

| 文件 | 用途 |
|---|---|
| `brand/dcx.ico` | 平台 exe 图标（9 帧：16–64 DIB、128/256 PNG），由 `crates/dcx-host/build.rs` 编译进 PE 资源 |
| `brand/box.ico` | **独立应用** `dcx-box.exe` 的图标（靛蓝底 + 白色带盖分格盒），由 `crates/dcx-box/build.rs` 编进它自己的 PE 资源 |
| `brand/embed/box-*.rgba` | 收纳盒窗口图标（32/64/256），供 `dcx-box/src/ui.rs` 内嵌 |
| `brand/icons/dcx-*.png`、`brand/icons/box-*.png` | 文档、商店素材、展示页 |
| `brand/tray/tray-*-on-{light,dark}.png` | 设计稿与实景模拟 |
| `brand/embed/*.rgba` | 裸 RGBA（无文件头），供 Rust `include_bytes!` 直接内嵌 |
| `brand/mockup.html` | 效果图（锁版、尺寸链、任务栏实景） |

Rust 侧接入点：

- `crates/dcx-host/src/tray.rs` —— 托盘图标，按 `AppsUseLightTheme` 在亮档/常规青之间切换（两档都是品牌色块 + 白 X，不再随主题反色成黑白字形）；
- `crates/dcx-host/src/settings_app.rs` —— 窗口图标（`icon-64.rgba`）与界面强调色；
- `crates/dcx-host/build.rs` —— 把 `dcx.ico` 写成 `.rc`、用 `rc.exe` 编译成 `.res`，再作为输入文件交给 link.exe；
- `crates/dcx-box/src/ui.rs` + `crates/dcx-box/build.rs` —— 收纳盒窗口图标走 `box-64.rgba`，exe 图标走 `box.ico`，与平台主图标完全分开。

内嵌而非读磁盘，是为了让单文件分发也能带图标：托盘和窗口图标都随二进制走，移动安装目录不会丢。
