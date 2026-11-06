# review.md 核对与修复记录（2026-11-06）

对应报告：`review.md`（基准 `main` @ `314c108`）。本记录只写「核对到了什么、改在哪、怎么验证」，不重复报告原文。

分支：`fix/review-p0`

## 1. 核对结论

报告中的 6 组 P0 全部在源码中复现，没有一条属于误报：

| 编号 | 核对方式 | 结论 |
|---|---|---|
| P0-1 | `cargo test --workspace` | 复现：`dcx-theme` 测试目标 25 处 E0422，整个 workspace 测试编译失败 |
| P0-2 | 读 `Config::save` / `Settings::save` / `apps::save` / `Config::patch` | 复现：固定 `.toml.tmp`、rename 失败回退截断直写、load-modify-save 无锁 |
| P0-3 | 读 `reapply::watch` / `DllMain` | 复现：失败分支 `ComPtr(cb_ptr).release()` 已把 refs 降到 0，随后又 release dp/value；DETACH 无分支 |
| P0-4 | 读 `parse_hotkey` / `bind` | 复现：无修饰键也返回 `Some`；失败仍写 `state`，1 s 后 `rebind` 直接 return |
| P0-5 | 读 `absorb` / `move_into_dir` / `copy_recursive` / `norm` | 复现：`absorb` 无祖先校验、跨卷删源被 `let _ =` 吞掉、`norm` 不折叠 `..` |
| P0-6 | 读 `open_settings_at` / `tray::quit` / `apps::spawn_enabled` | 复现：Child 未保存、退出走 `process::exit(0)`、看门狗无停止标志 |

修复 P0-1 后暴露出 5 个**此前从未运行过**的 theme golden 断言（`BorderThickness` 期望 `0.5,1,0.5,1`、`TargetStyles` 期望 45 条等）。核对主题数据后确认是**断言过期**、不是实现回归：`theme.rs` 里 `BorderThickness=0` 带有明确注释，且新增了 `TaskbarBackground` 折叠条目。按实际口径更新断言，并为折叠条目补了一条 golden 用例。

## 2. 改动索引

| 主题 | 入口 |
|---|---|
| 配置写入协议 | `crates/dcx-win32/src/config_file.rs`（新）、四个 `Config::save/load` |
| 文件移动安全 | `crates/dcx-organizer/src/lib.rs`：`norm`、`is_same_or_under`、`drop_refusal`、`move_into_dir`、`copy_recursive`、`unique_path`、`prune`、`migrate_library` |
| 热键 | `crates/dcx-host/src/hotkey.rs` + `settings_app.rs` 输入校验 |
| 引擎生命周期 | `crates/dcx-engine/src/{reapply,lib,hover,diag,bootstrap}.rs` |
| 退出编排 | `crates/dcx-host/src/{main,tray,apps,shutdown,settings_app}.rs`、`dcx-core/src/platform.rs` |

## 3. 验证结果

| 命令 | 结果 |
|---|---|
| `cargo check --workspace` | 通过（仅剩既有 `dead_code` 告警） |
| `cargo build --workspace` | 通过（含 cdylib 链接） |
| `cargo test --workspace` | **22 个测试目标全绿**，0 失败 |
| `cargo clippy --workspace --all-targets` | 无 error、无 future-incompat |

新增测试覆盖：

- `path_normalisation_folds_parent_segments`、`containment_cannot_be_escaped_with_dotdot`
- `absorb_refuses_the_desktop_directory_itself`（含 `..` 变形）
- `move_into_dir_never_overwrites`
- `prune_waits_for_consecutive_misses`
- `hotkey::tests::*`（裸键被拒、多主键被拒、未知 token 被拒、NOREPEAT）
- `autostart::tests::points_at_us_only_for_our_own_path`（含"路径只出现在参数里"）

## 4. 仍需实机验收（本环境无法覆盖）

报告第 8 节的实机用例仍需在目标机跑一遍，尤其是：

1. 双进程高频并发写四个 TOML 10 分钟，文件始终可解析、字段不丢。
2. 人为占用目标文件触发 `ReplaceFileW` 失败后，旧配置完好且界面有报错。
3. 文件移动五用例：跨卷、被占用、junction、拖入桌面目录、含 `..` 路径。
4. 热键：输入 `s` 被拒；占用程序退出后 1 秒内自动重绑；热键不阻塞 `WM_TIMER`。
5. 退出：托盘退出后 `dcx-host.exe --settings` / `dcx-box.exe` / `dcx-search.exe` 无残留。
6. 引擎：热重载 100 次后回调注册数与内存不单调增长（**未修**，见下）。

## 5. 明确未修项

| 编号 | 原因 |
|---|---|
| P1-5 回调/token 累积 | 需要 `unregister_property_changed_callback` 的完整所有权改造，且报告自述未复现；按报告建议先加计数器确认频率 |
| P1-6 `RESOLVED`/`BRUSHES` 永久持有 | 清缓存必须成对 Release，改错会让 explorer 崩溃；需要实机验证 |
| P1-7 hover 同线程二次加锁 | 属推断性死锁，热路径；先加日志确认再改 |
| P1-8 检索线程 join / 节流 | 涉及查询取消语义，改动面大于本次范围 |
| P2-2/P2-3 正则与候选集上限 | 与 P1-8 同一条链路，建议一并做 |
| 引擎 `Box::leak`、`LoadLibraryExW` 不释放 | 每个 explorer 进程生命周期内一次性增长，风险低于上述项 |

这些都在报告里标为 P1/P2，不影响本次 P0 的可用性；建议按报告第 8 节顺序单独立分支处理。
