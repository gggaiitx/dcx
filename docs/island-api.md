# 信息岛本地推送 API

把脚本的进度、构建状态、提醒推到信息岛「动态」页，胶囊会显示最新卡片的标题（带进度百分比）并脉冲一次；卡片到期自动消失。

- 只监听 `127.0.0.1`，局域网/公网不可达。
- 除健康检查外，所有请求都要 `Authorization: Bearer <token>`。
- 令牌在 `%LOCALAPPDATA%\dcx\island_api_token.txt`（信息岛首次启动时生成；设置页「桌面信息岛 → 推送 API」可复制/重置）。
- `dcx-island.exe` 在 `--push` 模式下**先于单实例检查**运行：岛已经在跑也能用，且不会打开/收起面板。

## 最快接入

### Windows PowerShell（含 5.1）——推荐用文件

PowerShell 5.1 在把带内嵌双引号的参数交给原生程序时会重新加引号，且**不同写法结果不一致**：`--push '{"id":"test"}'` 会被吃成 `{id:test}`（报 `JSON 解析失败：key must be a string at line 1 column 2`），改成 `\"` 转义后有时能过、有时又变成 `EOF while parsing a string`。可靠做法是让 PowerShell 写文件、程序读文件（路径作为参数没有引号问题，UTF-8 BOM 会被自动跳过）：

```powershell
$j = '{"id":"test","title":"测试推送","progress":0.4,"ttl_secs":30}'
[IO.File]::WriteAllText("$env:TEMP\dcx-island-push.json", $j, [Text.Encoding]::UTF8)
& "D:\dcx-dist\dcx-island.exe" --push-file "$env:TEMP\dcx-island-push.json" | Out-Host
```

`| Out-Host` 让 PowerShell 等待这个 GUI 子系统的程序并显示返回的 JSON；不加的话提示符会先回来，输出稍后才出现（卡片本身仍会正常推送）。

`--push` 在 PowerShell 7 里可以直接用单引号：`& exe --push '{"id":"test","title":"x"}'`。

### cmd.exe

```bat
echo {"id":"test","title":"测试推送","progress":0.4} > "%TEMP%\p.json"
"D:\dcx-dist\dcx-island.exe" --push-file "%TEMP%\p.json"
```

`--push` 也可以，但要把内部引号写成 `\"`：`--push "{\"id\":\"test\",\"title\":\"测试推送\"}"`。

### Git Bash / WSL

```bash
D:/dcx-dist/dcx-island.exe --push '{"id":"test","title":"测试推送","progress":0.4,"ttl_secs":30}'
```

两种模式都先于单实例检查运行：岛已经在跑也能用，且不会打开/收起面板。退出码 0 = HTTP 200；失败时 stderr 给出原因。`id` 取固定值即可「同一张卡持续更新」，每次推送续期 TTL，不会堆一屏卡片。

## 测试数据

```json
{
  "id": "build-42",
  "title": "构建中",
  "body": "正在编译 dcx-island（12/40）",
  "progress": 0.3,
  "ttl_secs": 600,
  "action": { "kind": "show_panel", "tab": "live" }
}
```

最小卡片：`{"id":"ping","title":"完成"}`（TTL 默认 12 秒）。

## HTTP 接入

### PowerShell

Windows PowerShell 5.1 会用 ANSI 编码发送字符串 body，中文会变 `?`；显式发送 UTF-8 字节即可：

```powershell
$token = (Get-Content "$env:LOCALAPPDATA\dcx\island_api_token.txt" -Raw).Trim()
$json  = '{"id":"build-42","title":"构建中","progress":0.3,"ttl_secs":600}'
Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:9841/v1/island/push" `
  -Headers @{ Authorization = "Bearer $token" } `
  -ContentType "application/json; charset=utf-8" `
  -Body ([Text.Encoding]::UTF8.GetBytes($json))
```

### curl（Git Bash / WSL）

```bash
TOKEN=$(cat "$LOCALAPPDATA/dcx/island_api_token.txt")
curl -sS -X POST http://127.0.0.1:9841/v1/island/push \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  --data-binary '{"id":"build-42","title":"构建中","progress":0.3}'
```

用 `--data-binary`（或 `--data-binary @file.json`），不要用 `-d` 传中文——Git Bash 的 argv 编码会破坏多字节字符。

### Python

```python
import json, pathlib, urllib.request
token = pathlib.Path.home().joinpath("AppData/Local/dcx/island_api_token.txt").read_text(encoding="utf-8").strip()
body = json.dumps({"id": "build-42", "title": "构建中", "progress": 0.3}, ensure_ascii=False).encode("utf-8")
req = urllib.request.Request(
    "http://127.0.0.1:9841/v1/island/push", data=body,
    headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
)
print(urllib.request.urlopen(req, timeout=5).read().decode("utf-8"))
```

## 端点

| 方法 | 路径 | 鉴权 | 说明 |
|---|---|---|---|
| GET | `/v1/health` | 否 | `{"ok":true,"app":"dcx-island","version":"…"}` |
| POST | `/v1/island/push` | Bearer | 推送/更新卡片，返回 `{"ok":true,"id":"…","expires_at":…}` |
| GET | `/v1/island/active` | Bearer | 当前未过期卡片，最新在前 |
| DELETE | `/v1/island/push/{id}` | Bearer | 撤回卡片，幂等 |

一次请求只处理一个 JSON 卡片；body 上限 64 KB，头部上限 8 KB，不支持 chunked 编码。

## 卡片字段

| 字段 | 必填 | 说明 |
|---|---|---|
| `id` | 是 | 1–64 位 `[A-Za-z0-9._-]`；同 id 再推 = 更新并续期 |
| `title` | 是 | ≤200 字符；胶囊显示它（有进度时带百分比） |
| `body` | 否 | ≤2000 字符；面板「动态」页卡片正文 |
| `progress` | 否 | 0–1 的小数；显示进度条与百分比（`0.4` = 40%） |
| `ttl_secs` | 否 | 存活秒数，默认 12，上限 3600；`0` 视为默认 |
| `action` | 否 | 见下表；没有动作就只有文字 |

动作白名单（没有执行命令一类动作）：

| kind | 参数 | 点击卡片按钮时 |
|---|---|---|
| `open_url` | `url`（http/https） | 用默认浏览器打开 |
| `open_folder` | `path` | 打开文件夹（不存在则报错提示） |
| `show_panel` | `tab`：`tasks`/`clip`/`inbox`/`perf`/`live`/`search` | 打开信息岛并切到该页 |

## 排错

| 现象 | 原因 |
|---|---|
| `JSON 解析失败：key must be a string at line 1 column 2` | PowerShell 5.1 把 JSON 里的双引号吃掉了（`{id:test}`）；改用 `--push-file` |
| `JSON 解析失败：EOF while parsing a string…` | 同上，另一种引号变形；改用 `--push-file` |
| 401 | 没带 token / token 已重置（设置页「重置」后立即生效，无需重启） |
| 400 + `id 需为…` | id 字符不合法或超过 64 位 |
| 400 + `open_url 仅支持…` | 动作链接不是 http/https |
| `无法连接 127.0.0.1:9841` | 信息岛未运行，或设置里关掉了推送 API / 改了端口 |
| 卡片不出现 | TTL 太短（先被下一条顶掉）；卡片最多 16 张，新的顶旧的 |

## 边界

- 令牌文件是**刻意的例外**：脚本需要一行就能读到它，因此不放 Windows 凭据管理器；它只授予「往岛上推卡片」这一项能力。
- 从设置页「复制」令牌后，岛内剪贴板历史会跳过与当前 token 完全相同的文本，不会落盘。
- 推送 API 默认开启；关掉后 `--push` 会直接报「推送 API 未启用」。
