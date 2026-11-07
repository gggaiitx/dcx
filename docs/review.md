# DCX 代码审查报告

| 项目 | 内容 |
|---|---|
| 审查日期 | 2026-11-06 |
| 审查对象 | `dcx/` 工作区 `main` @ `314c108`（工作区干净）；`dcx - 副本/` 未纳入 |
| 审查范围 | 12 个 crate、74 个 `.rs`、41,337 行 Rust |
| 审查方式 | 分 4 个子系统并行逐文件静态审查 + 编译/测试/clippy 实跑 + 主审交叉复核 |
| 源码改动 | **无**（本报告只审查，未修改任何 `.rs`） |
| 产品口径 | **已确认：回归移动文件**（见第 1 节） |
| GUI 实机验证 | 未做。除第 2 节外，均为静态结论，需按第 8 节验收清单实机复核 |

> 证据约定：代码块为审查时从工作树直接读取的原文；表格中只给 `文件:行号` 的条目为静态审查结论，可按行号复核。所有行号基于 `314c108`。

---

## 0. 结论摘要

架构分层、进程隔离、注释与工程笔记质量高于一般个人项目；`dcx-win32` 手写 FFI、检索查询解析、DPI 换算、句柄释放是质量最扎实的部分。**但当前状态不建议直接对外发布**：存在可永久丢失用户数据的配置竞态、explorer 进程内的 COM 引用计数错误、可劫持全系统键盘的热键校验缺失，以及产品口径确认后必须直面的一批文件移动安全问题。

| 严重度 | 数量 | 含义 |
|---|---|---|
| P0 必修 | 6 组 | 发布阻断：数据丢失、explorer 崩溃、全系统键盘劫持、孤儿进程 |
| P1 高危 | 9 组 | 上线前应修：界面冻结、泄漏累积、线程不可取消、看门狗失控 |
| P2 中危 | 14 项 | 行为偏差与性能退化 |
| P3 低危 / 清理 | 9 项 | 死代码、告警、兼容性、健壮性 |
| 已核实无误 | 8 类 | **不要按常见直觉去"修"**，见第 7 节 |

**必修清单（按修复顺序）**

| 序号 | 问题 | 主位置 |
|---|---|---|
| P0-1 | `cargo test --workspace` 编译失败 | `dcx-theme/src/rules.rs:925` |
| P0-2 | 4 个 TOML 无跨进程锁 + 原子写自毁（经引擎放大为任务栏全量重写） | `dcx-organizer/src/lib.rs:479`、`dcx-settings/src/lib.rs:247` |
| P0-3 | 引擎 COM 双重 Release + DLL 无 DETACH 清理 | `dcx-engine/src/reapply.rs:156`、`src/lib.rs:706` |
| P0-4 | 热键可注册裸键、冲突后永不重试 | `dcx-host/src/hotkey.rs:173`、`:220` |
| P0-5 | 文件移动路径校验缺口（口径确认后升为 P0） | `dcx-organizer/src/lib.rs:946`、`:1314` |
| P0-6 | 退出编排：孤儿设置窗口 / 硬杀退出 / 看门狗无停止标志 | `dcx-host/src/settings_app.rs:140`、`apps.rs:246` |

---

## 1. 产品口径（已拍板，不再是待定项）

**2026-11-06 飞总确认：回归移动文件。** 收纳 = 把桌面文件**物理移动**到独立收纳库目录（默认 `文档\dcx收纳`，设置中心可改路径并迁移），放回时移回桌面根目录；桌面仍由 Explorer 原生显示，文件不删除。分类是库内虚拟元数据（扩展名规则 + `organizer.toml` 的 `overrides` 手动覆盖）。

证据链（`crates/dcx-organizer/src/lib.rs`）：

- `Config::library_dir()` `:345-365` → `docs.join("dcx收纳")`；
- 两套扫描面：`items()` `:708-710` 扫库 root、`desktop_items()` `:714-725` 扫真实桌面；
- `Organizer::absorb()` `:946-970` → `move_into_dir()` `:1314-1336`（`std::fs::rename`，跨卷回退复制+删源）；
- `move_to_desktop()` `:1061-1099`、`move_to_folder()` `:1014-1056`、`migrate_library()` `:1363-1418`；
- 单测 `collection_is_separate_from_the_real_desktop`（`:1727-1753`）断言收纳后 `!file.exists()` 且库内存在；`folder_move_is_physical_and_children_are_listed`（`:1697`）。

文档 `README.md:45/62`、`docs/engineering-notes.md:194-207` 与代码一致。**作废**：项目备忘 2026-11-04 记的「不移动任何文件、只做分类展示」约束，以及 2026-11-06 日志第 10 行把 README 判为"文档错误"的记录。

> 口径影响：本次审查中所有因物理移动而产生的数据风险（P0-5）**不是待讨论的设计分歧，而是必修缺陷**；修复方向是加固文件操作，不是回退到纯分类视图。

---

## 2. 验证结果（实跑，非推断）

| 命令 | 结果 |
|---|---|
| `cargo check --workspace` | **通过**。17 条 `dead_code`（主要在 `dcx-box`）、3 条 host 警告、1 条 future-incompat |
| `cargo test --workspace` | **失败**：`dcx-theme` lib test 25 处 E0422 |
| `cargo check --workspace --all-targets` | 失败，同上 |
| `cargo test -p dcx-organizer` | 14/14 通过 |
| `cargo test -p dcx-search` | 10/10 通过 |
| `cargo test -p dcx-host --bin dcx-host` | 2/2 通过 |
| `cargo test -p dcx-box --no-run` | 编译通过（未实跑） |
| `cargo clippy --workspace` | 无高危 lint；告警集中在 `dcx-win32` / `dcx-blur` 的 `# Safety` 文档缺失与风格项 |

`rustc` future-incompat 警告：`dcx-host/src/settings_app.rs:3097` `Stroke::new(2.0, p.secondary)` → 未来版本将变硬错误，改 `2.0_f32`。

---

## 3. P0 必修

### P0-1 · 全量测试目标无法编译

**位置**：`crates/dcx-theme/src/rules.rs:925`（`#[cfg(test)] mod tests`）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
```

`TargetStyles` 由 `crate` 公开再导出，测试模块只用 `use super::*`，导致 `:1572`、`:1609`、`:1640`、`:1722`、`:1745`、`:1776`、`:1914` 等共 25 处 `E0422: cannot find struct ... TargetStyles`。

**后果**：`cargo test --workspace` 与 `cargo check --all-targets` 全挂。README:222-227 只引导跑 `-p dcx-organizer` / `-p dcx-search`，仓库无 CI，坏目标一直未被发现。

**修复**：在 `:925` 后补 `use crate::TargetStyles;`（一行）。修完把 `cargo test --workspace` 纳入发布门禁。

---

### P0-2 · 配置「文件即 IPC」无跨进程锁，且原子写会自毁

README:142 把「文件即 IPC」列为架构优点，但 4 个 TOML 都是**多进程读-改-写、无锁、固定临时名**。

**证据 A：原子写被自己的回退破坏**（`dcx-organizer/src/lib.rs:479-495`，`dcx-settings/src/lib.rs:247-268` 同构）

```rust
let tmp = p.with_extension("toml.tmp");
std::fs::write(&tmp, text.as_bytes())?;
match std::fs::rename(&tmp, &p) {
    Ok(()) => Ok(()),
    Err(e) => {
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(&p, text.as_bytes()).map_err(|_| e)   // 非原子截断覆盖
    }
}
```

`dcx-settings/src/lib.rs:243-246` 的注释明确写着"引擎每秒轮询、半写会让任务栏瞬间重置"——回退分支恰好制造半写。

**证据 B：裸读-改-写**（`dcx-organizer/src/lib.rs:530-534`）

```rust
pub fn patch<F: FnOnce(&mut Config)>(f: F) -> io::Result<()> {
    let mut cfg = Config::load();
    f(&mut cfg);
    cfg.save()
}
```

**证据 C：陈旧快照整份覆盖**。`dcx-host/src/settings_app.rs:905` 只在 `new()` 读一次 `BoxConfig`；`persist_box()` `:1804-1820` 仅合并 11 个 UI 键，`overrides` 始终是启动时的旧副本：

```rust
let disk = BoxConfig::load();
self.bx.sort_by = disk.sort_by; /* … 11 个 UI 键 … */
match self.bx.save() { ... }        // 整份写回，overrides 是旧值
```

`dcx-box` 侧 `save_keep_gone()`（`lib.rs:1139-1152`）与 `new_category()` 也写同一文件。用户手动归类 → 设置窗口改任意一项 → 220 ms 防抖 → 手动分类被抹掉。

**证据 D：解析失败静默回退默认值**（`lib.rs:378-391`）

```rust
Err(e) => { eprintln!("... parse failed ..."); Config::default() },
Err(_) => Config::default(),
```

半写 TOML → 内存变成默认值（含 `library_root: None`、空 `overrides`/`categories`）→ 下一次保存把默认值写回磁盘 → **手动分类与自定义库路径永久丢失**。

**引擎侧放大链**（host 与 engine 两侧交叉核实）：

| 环节 | 位置 | 行为 |
|---|---|---|
| 半写改 mtime | `dcx-engine/src/bootstrap.rs:410-429` | 每 1000 ms 轮询，`post_to_taskbar(..., 2)` |
| 触发全量重载 | `bootstrap.rs:237-245` | `build_engine_from_settings()` + `reload_and_apply()` + `hover::on_settings_changed()` |
| 默认规则覆盖 | `dcx-engine/src/lib.rs:604-619` | **无条件** `Settings::load()` 并 `*g = Some(engine)`，无保留旧值分支 |
| 强制重写 | `lib.rs:561-601` | `reset_caches()` 后对每个元素 `apply_element(record, true)`，`force=true` 绕过 `mark_written` |
| 单调恶化 | `apply.rs:227-257` / `reapply.rs:169-174` / `lib.rs:367-377` | 每次误 reload 再累积一批永不注销的 property-changed callback 与 `Box::leak` 字符串 |

即：**一次杀软/编辑器占用导致的 rename 失败，不是"这次不应用"，而是按默认主题强制重写整棵任务栏，并累积常驻泄漏。**

**涉及文件**：`settings.toml`（host 设置进程写 / engine 读）、`organizer.toml`（设置进程 + box 双写）、`apps.toml`（`apps.rs:114-120`）、`search.toml`（`dcx-search/src/config.rs:195-200` 的 `unwrap_or_default()` + `persist_ui()` RMW；host 侧 `settings_app.rs:650/660/726` 读 scope/exclude 失败回 `default_excludes()`，两边对坏文件口径不一致，会互相重置对方配置）。

**修复**：一次性统一处理 4 个文件，不要逐个打补丁——① 命名互斥体（或文件锁）保护整个 load-modify-save；② tmp 文件名带 PID/随机后缀；③ 删除非原子直写回退，rename 失败即返回错误并保留旧文件；④ 解析失败时禁止自动落盘（只读 + 提示）；⑤ 读侧缓存加版本/时间戳校验，避免陈旧快照覆盖。

---

### P0-3 · 注入 DLL 的 COM 引用计数错误 + 无卸载清理

**双重 Release**（`dcx-engine/src/reapply.rs:156-183`，`cb_release` 在 `:113-121`）

```rust
ComPtr(dp).add_ref();
ComPtr(value).add_ref();
let registered = ... register_property_changed_callback(...) ...;
ComPtr(cb_ptr).release();     // 失败时 refs 1→0 → cb_release 已 release(dp) release(value) + drop(Box)
if !registered {
    ComPtr(dp).release();     // 第二次
    ComPtr(value).release();  // 第二次
}
```

注册失败时 `add_ref` 一次、`release` 两次 → `DependencyProperty` 与 boxed value（如 `ColumnDefinitionCollection`）引用计数下溢 → 对象提前析构、悬垂指针，**发生在 explorer.exe 进程内**。触发条件是 `RegisterPropertyChangedCallback` 失败（QI 失败 / HRESULT 失败 / SEH 捕获），正是 Windows 大版本更新后 XAML 视觉树不匹配时的高发场景（README:231-233 自列为已知限制）。

**无 DETACH 清理**（`dcx-engine/src/lib.rs:706-720`）

```rust
if reason == DLL_PROCESS_ATTACH && !STARTED.swap(true, Ordering::SeqCst) {
    diag::install();
    CreateThread(..., Some(attach_thread), ...);
}
1
```

只处理 ATTACH。`hover.rs:1001-1017` 的 `shutdown()`（`KillTimer` / `timeEndPeriod` / 释放 COM 引用）全仓库无调用点，是死代码；`slot.rs` 无对应 `KillTimer`；`diag.rs:118` 的 `AddVectoredExceptionHandler` 无 `Remove`；后台线程（`lib.rs:688` 最长 10 分钟、`bootstrap.rs:372` 7 个延迟线程、`:412` 每秒 watch）均无停止标志与 join。当前靠 `taskkill /F explorer.exe` 卸载是**有意取舍**（`shutdown.rs:9-13` 与 `docs/engineering-notes.md:512` 说明 FreeLibrary 会导致 0xc000027b），但 DLL 自身不具备安全卸载能力：一旦有路径触发 FreeLibrary，残留的定时器/VEH/TAP 回调会跳进已释放代码页。

**修复**：① `watch()` 失败分支去掉 `:179-180` 的重复 release（或改为成功路径才移交所有权），并补一条失败路径单测；② 至少补 `DLL_PROCESS_DETACH` 的 `KillTimer` + `RemoveVectoredExceptionHandler` + `Unadvise` + 停止标志；若确因 XAML 生命周期无法安全卸载，应在文档与代码注释里把"禁止 FreeLibrary"写成硬约束而非隐含假设。

---

### P0-4 · 全局热键校验缺失与失败不重试

- `dcx-host/src/settings_app.rs:1441-1443`：快捷键是自由文本 `TextEdit`，无任何格式校验。
- `hotkey.rs:173-201`：`parse_hotkey()` 不要求任何修饰键，多主键静默覆盖，`mods` 仅 `MOD_NOREPEAT`；`bind()` `:232` 直接 `RegisterHotKey(hwnd, id, 0x4000, 'S')`。
  → 用户填 `"s"`，**全系统 S 键被独占，所有程序打字失效**，直到改键或退出 host。
- `hotkey.rs:220-245`：注册失败（被其它程序占用）仍无条件 `*state = Some((key, enabled))`；1 s 后的 `rebind()` 因状态相同直接 return → 占用解除后热键**永久失效**。
- `hotkey.rs:268-293`：`WM_HOTKEY` 回调在唯一 `GetMessageW` 循环里同步读 TOML、`FindWindowW`、`Command::spawn`、写 marker 文件；spawn 阻塞会让 `WM_TIMER` 与后续热键全部排队。

**修复**：① 解析层强制要求至少一个修饰键（Alt/Ctrl/Shift/Win）、拒绝多主键、白名单 VK；② 注册失败不写 `state`，允许 1 s 后重试并把错误显示到设置页；③ 热键回调只发信号，IO 移出窗口过程。

---

### P0-5 · 文件移动安全（产品口径确认后升为 P0）

| 子项 | 位置 | 问题与后果 |
|---|---|---|
| 拖放零校验 + 缺祖先保护 | `dcx-box/src/app.rs:904-937`、`dcx-organizer/src/lib.rs:946-970` | `dropped_files` 路径不经归一化/白名单/祖先校验直达 `move_into_dir`；`absorb()` 只判「不在 root 内」，而 `:982` 的 `absorb_auto()` 才有 `same_path(root) \|\| is_same_or_under(root, p)` 保护。把桌面文件夹拖到分类标签可 `rename(desktop, root\Desktop)` **整体搬走桌面** |
| 跨卷删源失败被吞 | `lib.rs:1314-1336` | `Ok(()) => { let _ = remove_recursive(src); Ok(dst) }`；被占用文件常能复制成功但删不掉 → 桌面留副本却返回 `Ok`、计 `moved+1`、提示「已收纳」，产生静默重复。错误分支还把真实 copy 错误换成误导性的 `rename_err` |
| 跟随 junction 复制 | `lib.rs:1482-1502` | `copy_recursive` 用 `metadata`（跟随链接）判目录、`read_dir().filter_map(|e| e.ok())` 吞枚举错误；`remove_recursive` 用 `symlink_metadata` 只删链接 → 外部整棵目录树被复制进库，源文件以副本形式滞留 |
| TOCTOU 覆盖 | `lib.rs:1440-1461`、`:1183-1191` | `unique_path` 后缀耗尽返回已存在名；`dst.exists()` 与 `std::fs::rename` 之间是 check-then-act，而 Windows `std::fs::rename` 带 `MOVEFILE_REPLACE_EXISTING` → 可静默覆盖同名文件 |
| `..` 绕过包含校验 | `lib.rs:1504-1513`、`:1304-1311` | `norm()` 不折叠 `..`，`is_same_or_under()` 是字符串 `strip_prefix` → `migrate_library:1369`「新旧库不能互相包含」、`absorb_auto:982` 祖先保护、`Organizer::new:653`「库不能放桌面内」三道校验均可用含 `..\` 的路径穿透 |
| 迁移回滚不完整 | `lib.rs:1379-1414`、`settings_app.rs:1915-1929` | `read_dir().filter_map(|e| e.ok())` 吞枚举失败；回滚 `let _ = move_into_dir(...)` 忽略失败；`report.pairs` 含 leftover 项，`remap_moved_key` 把 `overrides` 改写到新路径而文件仍在旧路径 |
| 静默裁掉手动分类 | `lib.rs:1155-1159`、`dcx-box/src/model.rs:567` | `prune()` 每 1.2 s 用 `Path::exists()` 裁掉 `overrides`/`excluded`/`hidden`；网络盘、OneDrive 占位符或拖拽刚落地的短暂不可达 → 下一次 `save_keep_gone()` 写盘固化丢失。这是比半写 TOML 更常见的丢数据路径 |
| 回收站删除静默 | `dcx-box/src/shell_io.rs:186-209` | `FOF_ALLOWUNDO \| NOCONFIRMATION \| SILENT \| NOERRORUI`；超大文件/网络卷/超长路径无法入站时可能永久删除且不提示；整批仅一个返回码，靠 `!p.exists()` 判定成功会误判；`CoInitializeEx` 无配对 `CoUninitialize` |

**修复方向**：统一入口做路径规范化（`GetFullPathNameW` / 折叠 `..`）+ 白名单（只允许桌面、库内、外部拖入的普通文件，拒绝桌面祖先、库自身、系统目录、UNC、重解析点）；移动用「先 copy+校验，再删源，任一失败即整体回滚并如实上报」；删除源失败不得计入成功；`prune()` 改为"连续 N 次不可达才裁剪"或只裁剪显式删除项。

---

### P0-6 · 退出编排：孤儿进程与硬杀

| 位置 | 问题 |
|---|---|
| `settings_app.rs:140-143` | 设置窗口是 spawn 出的独立 `dcx-host.exe --settings`，**不保存 Child** |
| `tray.rs:59-64` / `shutdown.rs:63-69` | 退出只 `apps::stop_all()`（仅遍历 `apps.rs` REGISTRY）+ `std::process::exit(0)` → 宿主退出后设置窗口继续运行、每帧读写配置与注册表、两个进程同时 append `host.log` |
| `main.rs:58,193-195` | `QUIT` 全仓库无读取点、`dcx_core::RUNNING/stop()` 无人调用、`platform.run()` 不返回 → `timeEndPeriod(1)`、WinEvent 反注册、effect shutdown 均不执行 |
| `apps.rs:246-253` | 5 s 看门狗无停止标志、无退避、无条件保活：用户主动关掉 box 会被拉回，应用崩溃即每 5 s 重启；`stop_all()` 到 `exit(0)` 之间仍可能 launch 出孤儿进程 |
| `injector.rs:215-235` | 注入失败不入 `injected`，每 5 s 重新 spawn `wh_injector.exe` + 刷日志，无次数上限；注入器挂起则 `Command::status()` 连看门狗线程一起挂住 |
| `dcx-core/src/platform.rs:136,162` | `SetTimer(0, 0, ...)` 由系统分配 ID，收尾却写死 `KillTimer(0, 0)`，定时器未销毁 |

**修复**：保存设置窗口 `Child` 并在退出时终止；所有长生命周期线程读同一个 `QUIT`；`stop_all()` 之前先置停止标志并 join；看门狗加指数退避与最大重试；`platform.run()` 改为可退出循环并走 `Drop` 清理。

---

## 4. P1 高危

| # | 位置 | 问题与后果 |
|---|---|---|
| P1-1 | `settings_app.rs:2317-2320` → `auto_hide.rs:56-58` | **每一帧**在 egui 线程调 `SHAppBarMessage`（向 explorer 发同步 shell 消息、无超时）+ 反复开关 HKCU 注册表键；explorer 忙/重启时设置窗口整个「未响应」 |
| P1-2 | `settings_app.rs:2259-2267` → `injector.rs:166-170` | 「重新应用」在 UI 线程跑 `explorer_pids()` + `Command::status()` 等注入器，无超时；`wh_injector` 卡在 `CreateRemoteThread` 则窗口永久冻结 |
| P1-3 | `settings_app.rs:1894-1909`、`lib.rs:1363` | 「迁移收纳库」在帧内 `read_dir` + 模态 `MessageBoxW` + 逐文件移动 + `persist_box()`，文件多或网络盘时长时间冻结且不可取消 |
| P1-4 | `settings_app.rs:2181` | box 页每帧 `BoxOrganizer::new(...).counts()` → `items()` → `scan_top(root)` 全量目录扫描 + 元数据读取，并每帧深拷贝配置/分配 Vec |
| P1-5 | `apply.rs:227-257`、`reapply.rs:169-174`、`dcx-xaml/src/xaml.rs:1014` | 每次 `SetValue` 注册新 property-changed callback，`token` 直接丢弃，`unregister_property_changed_callback` 全仓库无调用者；`apply_all` 被调度 7 次、reload 清 `WRITTEN` 后全量重写 → delegate 与注册项单调累积、回调风暴 |
| P1-6 | `apply.rs:53`、`:301-341`、`lib.rs:593-601` | `RESOLVED` 每个 (dp,value) `add_ref_raw` 后永久持有；`BRUSHES` 以 COM 对象**地址**为键且 `reset_caches()` 不清 → 元素销毁后地址复用会把绑定到旧元素的 brush 写回新元素，并永久持有 COM 引用 |
| P1-7 | `hover.rs:241-370` | `tick()` 全程持非重入 `STATE` Mutex，锁内调用会 pump 消息的 XAML `SetValue` / 回调注册；重入链 `cb_on_change → record_element → hover::mark_dirty → STATE.lock()` 会**同 UI 线程二次加锁自死锁**。作者已在 `lib.rs:555-560` 记录同类重入曾导致 0xc000027b，但 hover 侧无等价保护 |
| P1-8 | `dcx-search/src/query.rs:662-669,730-748`、`app.rs:192-200,974,1029-1040` | 内容检索单文件 `search_path` 无超时，取消查询后仍无条件 `for h in handles { h.join(); }` → 快速改词时新查询要等旧全盘扫描跑完；索引重建无节流、不 join 旧线程，反复 rebuild 叠加多组 2–8 个全盘遍历线程 |
| P1-9 | `query.rs:247,296,415-449`、`config.rs:56-62` | 持 `Arc<RwLock<Index>>` 读锁对最多 2000 个命中逐个 `metadata()`；后台 `save_snapshot` 在读锁内写大快照 → 索引 builder/watcher 写锁被阻塞数秒；`max_results` / `max_file_size_mb` / `time_budget_ms` 无 clamp，手工编辑配置即可放大到 OOM |

其它 P1：`lib.rs:63-75` `mark_written` 在 16,384 条后停止 push 但仍返回 `true`，去重失效，按 `:49-56` 注释会触发「重写 → 新 Add → 再 apply」反馈循环；`tap.rs:85-100` 自实现 COM 对象的 `refs` 用非原子 `u32 +=/-=`（对比 `dcx-blur/src/ffi.rs:777` 与 `reapply.rs` 用 `AtomicU32`，属疏漏）；`diag.rs:72-97` VEH handler 在异常上下文里加锁、分配、做文件 IO；`shutdown.rs:103,125` 用裸命令名 `explorer.exe` / `taskkill`（便携目录可写，存在 CreateProcess 搜索顺序劫持面）；`apps.rs:89` `default_enabled()` 恒 `true` 与 `:41` 的 `AppEntry.default_on`（search=false）语义冲突，手工编辑 `apps.toml` 漏 `enabled` 会让默认关闭的应用自启并全盘索引。

---

## 5. P2 中危

| # | 位置 | 问题 |
|---|---|---|
| P2-1 | `dcx-search/src/config.rs:195-200`、`settings_app.rs:650/660/726` | `search.toml` 双进程写，坏文件回退口径不一致，互相重置排除规则/检索范围（并入 P0-2 统一修） |
| P2-2 | `query.rs:30-59,580-587` | `regex:` token 数无上限、`parse()` 不受 `time_budget`/取消约束、`grep_regex` 二次编译无 `size_limit` → **编译型** CPU/内存 DoS（非回溯型，见第 7 节）；失败分支把正则源码塞进 `terms`，使名称匹配退化为 O(term 数) |
| P2-3 | `query.rs:529-575` | 内容检索候选集 `Vec<String>` 无上限、不受 `max_results` 约束，宽泛查询内存峰值数百 MB |
| P2-4 | `index.rs:310-337,315` | `remove()` 只标 `dead`，`pool`/`by_path` 单调增长；`pool.len() > u32::MAX` 后直接 return 丢弃新条目且无日志 → 常驻进程最终静默停止收录 |
| P2-5 | `dcx-search/src/app.rs:293-299` | `save_blocking()` 先 `dirty.swap(false)` 再判断 600 s 节流提前 return，dirty 已清却不保存，本次会话变更丢失 |
| P2-6 | `dcx-box/src/model.rs:410-423`、`lib.rs:650-665` | 库路径变更时 `reload()` 不迁移；库在桌面内时静默改用 `%LOCALAPPDATA%\dcx\library` 但 `Config.library_root` 原值不变 → 设置页显示路径与实际 root 不一致，`create_dir_all` 失败被吞 |
| P2-7 | `lib.rs:612,1275`、`dcx-box/src/desk_place.rs:389`、`dcx-win32/src/lib.rs:1160` | `to_string_lossy()` 把非法 UTF-16 路径变成 U+FFFD，属性判断/去隐藏/SHChangeNotify/回收站作用到错误路径；线程 `.expect` 在 release(`panic=abort`) 直接终止进程 |
| P2-8 | `dcx-box/src/file_icon.rs:28,50-57`、`view.rs:1763` | 图标缓存 192 条满则**整体清空**；`lnk/exe/ico` 按全路径缓存且每帧同步 `SHGetFileInfoW` + `load_texture` → 大列表掉帧（有上限，未见无限增长；GDI 释放完整） |
| P2-9 | `dcx-box/src/win.rs:84-88,111-119`、`tile.rs:374-403`、`main.rs:58` | `hit_test_proc` 已安装但 `HIT.active` 永为 false（见第 7 节）；`OLD_PROC` 是单个全局 `AtomicIsize`，窗口重建后可能调用失效过程指针；`CreateDIBSection` 成功但 `bits` 为空时 bitmap 未释放；单实例互斥体句柄无 `CloseHandle` |
| P2-10 | `dcx-win32/src/lib.rs:724-747` | `apps_use_light_theme()` / `reg_*` 被 `Skin::system()` 每帧调用，每帧 3 次注册表访问 |
| P2-11 | `query.rs:72-75,94-102,280` | 通配符 `?` 按字节消费，UTF-8 中文/emoji 错配；大小写折叠与排除规则只做 ASCII，Unicode 等价形式漏匹配 |
| P2-12 | `index.rs:97-115`、`query.rs:133-144` | `custom_roots` 无 exists/规范化/本地卷校验即可交给 `ignore::WalkBuilder` 与 `notify` 递归 watch；`under()` 是纯字节前缀，不处理 `..`/8.3/Unicode（不构成路径穿越） |
| P2-13 | `autostart.rs:31-36,62-71` | `points_at_us()` 用 `contains` 子串匹配，注册表值任意位置含当前 exe 路径即误判「已开启」，`refresh_path()` 不修复；写入侧引号与 REG_SZ 写法正确、无命令注入 |
| P2-14 | `main.rs:103-122`、`boot.rs:169-181` | `--settings` 不走 `already_running()`，只靠 `FindWindowW` 去重；两个进程几乎同时启动且窗口未创建时会开出双窗口并发写同一 TOML（叠加 P0-2） |

---

## 6. P3 低危 / 清理项

| 位置 | 说明 |
|---|---|
| `dcx-box/src/win.rs:602,628` 等 17 处 | `set_hit` / `set_region` / `CreateRectRgn` 等 dead_code，属 11-05 窗口模型重构残留，可删不可"修" |
| `dcx-host/src/settings_app.rs:3097` | `Stroke::new(2.0, ...)` future-incompat 警告，改 `2.0_f32` |
| `dcx-win32/src/lib.rs:1200-1306`、`dcx-blur/src/ffi.rs` 等 | clippy `missing # Safety` 文档，风格类，无高危 lint |
| `dcx-engine/src/lib.rs:367-377` | 动态规则展开每次 `Box::leak`，explorer 常驻进程内单调增长 |
| `dcx-engine/src/bootstrap.rs:150-154` | `LoadLibraryExW("Windows.UI.Xaml.dll")` 结果从不 `FreeLibrary` |
| `dcx-engine/src/tap.rs:205-221,102-140` | `fac_qi` 命中不 AddRef；`SetSite` 重连时旧 callback `Box::into_raw` 只 unadvise 不释放 |
| `dcx-box/src/shell_io.rs:113`、`settings_app.rs:2638`、`dcx-search/src/win.rs:292-294` | `CoInitializeEx` 无配对 `CoUninitialize`（`desk_place.rs:364,387` 是正确对照） |
| `dcx-core/src/platform.rs:136,162` | `KillTimer(0, 0)` 配对错误（并入 P0-6） |
| `dcx-theme/src/expr.rs:404-413,263-287` | 递归下降解析器无深度上限，恶意主题表达式可栈溢出 |

---

## 7. 已核实无误 —— 不要按常见直觉去「修」

1. **`dcx-win32/src/lib.rs`（1,367 行手写 FFI）**：`wide()` 追加结尾 NUL；`SHFILEINFOW`/`ICONINFO`/`BITMAP`/`MSG`/`WNDCLASSW`/`APPBARDATA` 的 `#[repr(C)]` 布局与 SDK 一致（`APPBARDATA` 还有偏移断言）；`WPARAM=usize`/`LPARAM=isize`/`HWND=isize`/`BOOL=i32` 位宽正确；无 `GetProcAddress` 动态转型。**未发现**签名、对齐、字符串终止或生命周期错误。
2. **ReDoS 不成立**：`regex:` 走 `regex::RegexBuilder`、内容检索走 `grep_regex::RegexMatcherBuilder`，均为 Rust `regex` 线性时间自动机，不支持回溯/反向引用；无效正则降级为字面量（`query.rs:44-55`，单测 `invalid_regex_does_not_match_everything` 覆盖）；内容检索字面量用 `regex::escape`（`:587`）。真实风险是 P2-2 的**编译型** DoS，两者不要混淆。
3. **通配符无元字符注入**：`query.rs:66-92` 是自研双指针 glob，不编译成正则。
4. **`set_hit` / `set_region` 死代码不是「透明窗口吞点击」回归**：`dcx-box/src/app.rs:1504-1517` 已把窗口模型改成「面板窗口只等于面板矩形（+阴影环）、关闭时移到 (-32000,-32000)」，任务栏按钮是独立 layered 窗口（`tile.rs`），不再需要 `SetWindowRgn` 裁剪或 `HTTRANSPARENT`。2026-11-04 备忘里那条约束已过时。
5. **DPI 换算自洽**：`app.rs:1413-1414` `ctx.set_pixels_per_point(self.scale)`，`win.rs:224-231` `to_px()` 与 `:217-222` `pt_rect()` 成对换算，cursor/origin 一致（仅 `win.rs:29` 注释「ppp==1.0」失准）。
6. **无命令注入、无明显句柄泄漏、无缓冲区溢出/消息解码错误**：`CreateProcess` 全用 PathBuf / 编译期静态字面量、不经 shell；Toolhelp 快照与 `OpenProcess` 配对 `CloseHandle`；定长缓冲按返回长度切片；`WM_HOTKEY` 只用 wparam 的 id。
7. **GDI / COM 句柄释放成体系正确**：`file_icon.rs` 的 HICON/HBITMAP/HDC、`desk_place.rs` 的 Release/CoUninitialize、`dcx-blur` 的 `Count(AtomicU32)`/`Box::from_raw` 均配对；例外仅限 P2-9、P3 列出的条件分支。
8. **测试与边界处理**：`organizer` 14/14、`search` 10/10、`host` 2/2 实跑通过；`format_double`、`GetDpiForWindow().max(96)`、`n==0/1`、除零保护、`record_element` 同 handle 覆盖等边界处理完整；`panic=abort` + 一进程一 winit 事件循环的拆分设计自洽。

---

## 8. 修复顺序与验收清单

| 阶段 | 内容 | 验收 |
|---|---|---|
| 0 | 产品口径已定（物理移动），不再讨论是否回退实现 | 文档与代码一致 |
| 1 | P0-1 补 import | `cargo test --workspace` 全绿 |
| 2 | P0-2 四个 TOML 统一加锁 / 唯一 tmp / 删直写回退 / 解析失败不落盘 | 双进程高频并发写 10 min，文件始终可解析、无字段丢失；人为占用目标文件触发 rename 失败后旧配置完好 |
| 3 | P0-5 文件移动安全 | 实机 5 用例：跨卷移动、被占用文件、目录 junction、拖入桌面目录、含 `..` 路径；每例校验无重复、无覆盖、无越界搬移、失败如实上报 |
| 4 | P0-4 热键 | 输入 `"s"` / `"Alt+s+t"` 被拒；占用程序退出后 1 s 内自动重绑；热键响应不阻塞 `WM_TIMER` |
| 5 | P0-6 退出编排 | 点托盘退出 / 恢复退出后，`dcx-host.exe --settings`、`dcx-box.exe`、`dcx-search.exe` 均无残留；`host.log` 单写者 |
| 6 | P0-3 引擎 | 失败路径单测；热重载 100 次后回调注册数与内存不单调增长；补 DETACH 清理或把「禁止 FreeLibrary」写成硬约束 |
| 7 | P1 → P2 → P3 | `cargo clippy --workspace --all-targets` 无 future-incompat；设置窗口在 explorer 挂起时仍可重绘；快速改词不堆积线程 |

**建议的发布门禁**：`cargo test --workspace` + `cargo clippy --workspace --all-targets` + 上述阶段 2/3/5 的实机用例。当前仓库无 CI，P0-1 长期未被发现正是缺门禁的直接后果。

---

## 9. 审查边界

- 未做 GUI 实机验证（真实鼠标输入、DPI 125%、任务栏自动隐藏、XAML 视觉树变化、explorer 重启），此类行为只能按 `docs/engineering-notes.md` 的记录与本报告第 8 节验收清单在目标机上复核。
- 注入器 `wh_injector.exe` 是外部二进制、不在本工作区，其 `VirtualAllocEx → WriteProcessMemory → CreateRemoteThread` 实现未审查。
- 静态审查结论（尤其 P1-7 的同线程死锁、P1-6 的地址复用）为代码路径推断，**未经崩溃复现**；修复前应先用日志/计数器确认触发频率，避免按推断重写稳定的热路径。
- 本报告不评价需求好坏，只核对「实现是否与已确认口径一致」及代码自身的正确性、安全性与资源生命周期。
