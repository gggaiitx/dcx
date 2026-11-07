# 第二轮 review · 灵动岛生态接入

- 日期：2026-11-08
- 分支：`feat/island-ecosystem`
- 审查对象：`42c4262..8650355` 的全部落地提交（A2、A5、推送 API、A1、P1 B3/B4/B2/C1、P2 B1/C3），以及本轮的修复提交
- 方法：独立审查代理逐条对照 `docs/island-ecosystem-feasibility.md` §4.1–4.6 与 §五，通读全部 diff 与相关文件；全量 `cargo test` / `cargo clippy`；随后按 §六在真机部署并做端到端验证。

## 一、结论

- 契约的六组能力全部落地；端到端验证通过（明细见 §四）。
- 审查提出 5 个 P1、8 个 P2。**P1 全部修复**；P2 修复 8 项、记录并接受 3 项（§三）。
- 修复后：`cargo test` island 68 / config 7 / search 10 全绿；clippy 无新增告警（余下均为既有：dcx-win32 的 `# Safety`、dcx-search 旧代码、organizer 的比较分配等）。
- 契约对照见 §六：4.1 与 4.6 的 partial 项即由本轮 P1 修复补齐。

## 二、P1（已修复）

1. **Ctrl+Enter 永不触发**。`TextEdit` 只为无修饰的 Enter 交出焦点（egui `return_key` 匹配 `Modifiers::NONE`），原实现把打开/定位都挂在 `lost_focus()` 上，Ctrl 按下时整段不可达，而页脚还宣传这个键。修复：Enter 独立检测，`reveal` 分支不看焦点（`ui/panel.rs:1517` 起）。
2. **A2 跨应用 id 冲突吞通知**。WinRT 通知 id 按应用计数，去重却只用 `n.Id()`：应用 B 的 id 3 会被应用 A 的 id 3 永久吞掉（直到 512 条清空）。修复：去重键改为 `(AUMID, id)`（`notify/qq.rs:180`），投递键同改 `toast-{aumid}-{id}`（`notify/mod.rs`）。
3. **剪贴板行 hover 时三键位移**。删除槽原先只在 hover 时占位，指针进入行内的一瞬间「收进盒子/图钉」左移 22 pt——瞄准 collect 实际点到 pin，瞄准 pin 点到删除。修复：删除槽无条件占位、仅 hover 时绘制（`ui/panel.rs:900`）。
4. **搜索页可永久卡死**。子进程 `Command::output()` 无超时，挂住时 `search_pending` 永不清空，输入不再触发新查询。修复：子进程 5 s 超时后 kill（`search.rs:47`），app 侧 6 s 兜底并给出「文件检索超时」错误（`app.rs:205` 起）。
5. **复制令牌进剪贴板历史**。设置页「复制」把 64 位 bearer 写进系统剪贴板，岛内的剪贴板监听会把它落盘进 `island_clipboard.json`（也在 Win+V 里）。修复：收取剪贴板时与当前 token 精确比对并跳过（`app.rs:575`）。

## 三、P2

已修：

- `GET /v1/island/active` 在面板删卡/到期后仍返回：`ApiServer::forget`/`prune_expired` 同步服务端读模型（`api.rs:282`、`api.rs:288`）。
- API 重启先起新再放旧：改为先 `self.api = None` 再 `start`（`app.rs` poll_config）。
- 封面每秒深拷贝 RGBA：`Snapshot.cover` 改 `Arc<Cover>`（`media.rs:71`），每秒只复制指针。
- 搜索过滤逐帧 `to_lowercase`：`AppEntry.name_lower` 在扫描时算好（`launcher.rs:14`）。
- 胶囊标题按可用宽度换行后被裁成半行字形：改用共享的测量截断 `widgets::fit_one_line`（`ui/widgets.rs:603`、`ui/compact.rs:379`）。
- 设置页「访问令牌 / 能力边界」两行文案截断：缩短文案（`settings_app.rs`）。
- `build-island.ps1` 每条 Compiling 起一个 powershell 拖慢构建：进度卡节流到 2 s，收尾卡强制推送。
- `read_request` 不可测：抽出 `read_request_from<R: BufRead>`，新增 4 KB/8 KB 头、短 body、chunked、LF-only 四个边界测试（`api.rs:460`）。

接受（附理由）：

- **收件箱渠道 chip 在 320 pt 最小宽度下会挤掉最后一个**：横向空间是既有约束，收窄 chip 会牺牲 12 pt 图标的可辨性；默认 358 pt 完整显示。记为已知限制。
- **启动时端口被占用会在 UI 线程重试最多 1 s**：只在启动路径发生一次，随后日志降级为 "API disabled"；把 bind 异步化需要改写 ApiServer 的生命周期，收益不抵复杂度。
- **封面读取瞬时失败会隐藏整首曲目的封面**（`cover_key` 已推进）：每秒重试解码的成本高于收益，切歌自愈。

审查中其余观察（时延、分配量级）未构成缺陷：媒体快照每秒一条、卡片上限 16、库扫描 5 s 且只扫顶层。

## 四、实机验证（2026-11-08 03:37–03:47，D:\dcx-dist）

| 项 | 证据 |
|---|---|
| 部署 | 三个 exe 备份为 `*.bak-20261108-0337`；停 host/island/search（box 未动）；`start-dcx.bat` 拉起 |
| API 鉴权 | `/v1/health` 200（免 token）；无 token push 401；带 token push 200；DELETE 200；非法 id 400 带中文原因 |
| `--push` | 中文卡片退出码 0，`island_show` 标记始终不存在（未触碰面板） |
| 动态页 | 卡片标题/正文/进度/剩余时间；到期自动消失；胶囊显示「标题 55%」并脉冲 |
| 媒体会话 | 隔离 Chrome 播放本地 wav（带 MediaSession 元数据）：面板顶部出现封面/标题/艺术家/时间/三键；点暂停→图标变播放、时间冻结、胶囊曲名隐藏；再点恢复。无会话时媒体行消失 |
| B3 | Perf：「桌面待整理 0 项 · 收纳库 82 项」，与磁盘一致；收纳后 83/84 跟随 |
| B4 | 行内「收进盒子」→ toast「已收进盒子」；库根出现 `剪辑 2026-11-08 03-45-52.md`；原剪贴板条目仍在 |
| B2 | 资源管理器拖 `DCX drag test.txt` 到胶囊 → 桌面文件消失、库根出现同名文件 |
| C1 | `build-island.ps1` 跑通；API 可查到「dcx-island 构建成功」卡（progress 1.0，ttl 12） |
| 设置页 | 推送 API 卡片：开关/端口/访问令牌（复制·重置·已生成）/能力边界 |
| 面板 | 六个 Tab 在默认宽度排布正常；搜索页应用+文件结果、底部键位提示可见 |

## 五、验证缺口（诚实记录）

- **A2 系统通知**：本机没有可构造的真实 Toast 源（QQ 未登录、通知访问为 Unspecified），只验证了默认关闭与 QQ-only 行为；需在有真实通知的机器上复验。
- **搜索 Enter / Ctrl+Enter**：修复后未做真机点击（会真的启动应用/打开资源管理器），以焦点语义修正 + 代码审查为准；点击行打开已由 B4/媒体行点击间接覆盖。
- **拖放高亮**：`hovered_files` 时胶囊染色未单独截帧；投放落库已验证。

## 六、契约对照

| 条目 | 状态 | 说明 |
|---|---|---|
| 4.1 A2 系统通知历史 | met | 默认关、开启后进消息页（含应用名）、关闭恢复 QQ-only、内存不落盘；去重已修 |
| 4.2 A5 全屏勿扰 | met | 全屏期间不调 `take_due`（不丢提醒），退出后按 STALE_AFTER 补发/丢弃；与隐藏开关独立 |
| 4.3 推送 API | met | 仅 127.0.0.1、除 health 外 Bearer、四端点、TTL/续期/删除、动作白名单无执行命令、动态页、`--push` 绕过单例 |
| 4.4 A1 媒体会话 | met | 事件+1 s 轮询、线程内解码封面、UI 建纹理、三键、本地插值、无窗口标题兜底 |
| 4.5 B3/B4/B2/C1 | met | 5 s+mtime 计数、复制式暂存 + `save_keep_gone`、OLE 拖放工作线程、两个脚本 |
| 4.6 B1/C3 | met | 无头查询在互斥前、快照缺失退出码 2、300 ms 防抖、.lnk 缓存、六 Tab、Ctrl+Enter 已修 |
| 五 回归 | met | 既有四 Tab/提醒/剪贴板/消息/深色主题不变；测试全绿；clippy 无新增 |
