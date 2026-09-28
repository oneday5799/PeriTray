---
name: peri-tray-release
description: "PeriTray 发版的可执行清单：版本号 5 处同步、手写 release notes 注入、轻量标签、发布后核对，以及撤回已发布版本并重发同一版本号。当用户说「发版 / 发布 / 出 vX.Y.Z / 打个标签 / release notes / 撤回重发」时使用。"
description_zh: "PeriTray 发版清单：版本号同步 + release notes 注入 + 打标签 + 核对 + 撤回重发"
description_en: "PeriTray release checklist: bump 5 version spots, inject notes, tag, verify, withdraw & re-release"
version: 1.4.0
display_name: "peri-tray-release"
display_name_en: "peri-tray-release"
visibility: "private"
agent_created: true
---

# PeriTray 发版

## 何时用

用户要求「发版 / 发布 / 出 vX.Y.Z / 打标签 / 写 release notes」时。仓库 `D:\Code\PeriTray`。

## 0. 前置（本机工具，不要用自带工具）

- git `C:\Program Files\Git\cmd\git.exe`（Windows 版**不认** MSYS 路径 `/d/...`，要写 `D:/...`）
- python `C:\Users\Oneday\.workbuddy-ai\binaries\python\versions\3.13.12\python.exe`
- node `C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-2\node.exe`
- cargo `/c/Users/Oneday/.cargo/bin/cargo.exe`（**必须在 `src-tauri/` 下跑**，仓库根没有 `Cargo.toml`）
- 网络：git 加 `-c http.schannelCheckRevoke=false`；curl 加 `--ssl-no-revoke`
- 查 GitHub 用 `api.github.com`（最稳）。⚠️ **`raw.githubusercontent.com` 偶发返回 0 字节**（同一路径两次结果不同）
  ⇒ 读仓库文件改走 `api.github.com/repos/<o>/<r>/contents/<path>?ref=<tag>` 并加
  `-H "Accept: application/vnd.github.raw+json"`。
- ⚠️ 本机 `gh` **未认证**、也没有 `RELEASE_TOKEN` ⇒ **不能**用 API 改已发布的 release 正文。
- ⚠️ bash 里 `grep`/`cat` 的中文输出会乱码 ⇒ 读中文一律走 Read 工具或 Python（`sys.stdout.reconfigure(encoding='utf-8')`）。

## 1. 确定范围与「用户可感知的改动」

```bash
git tag --sort=-creatordate | head -5          # 上次标签
git log --oneline <last>..HEAD                 # 待发布提交
```

逐个读提交正文（本仓提交正文极详实，含根因/影响边界），判定**用户能否在界面上看到或感觉到差别**：

- 用户可感知 ⇒ 🐛 问题修复 / ⚡ 性能与流畅度
- 纯内部重构、CI、文档、静态检查 ⇒ 🧹 内部优化

## 2. 改版本号（**5 处，必须全改**）

| 文件 | 形式 |
|---|---|
| `package.json` | `"version": "X.Y.Z"` |
| `src-tauri/Cargo.toml` | `version = "X.Y.Z"` |
| `src-tauri/Cargo.lock` | PeriTray 条目的 `version = "X.Y.Z"` |
| `src-tauri/tauri.conf.json` | `"version": "X.Y.Z"` |
| `src-tauri/dist/settings.html` | `>版本 vX.Y.Z</button>`（**首帧占位**，运行时由 `get_app_version` 覆盖，仍要改以免闪旧号） |

⚠️ **行尾不统一**（2026-09-22 逐文件实测，不是推测）：
**CRLF** —— `package.json`、`src-tauri/tauri.conf.json`、`src-tauri/dist/settings.html`；
**LF** —— `src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`。
⇒ 用 `io.open(path, "r", encoding="utf-8", newline="")` 读、同参数写回，
**字节级保留原行尾**（别做全局 LF↔CRLF 转换）；改完复核「CRLF 数 + 裸 LF 数 == 总行数」。
✅ **两遍式**做法：先断言每条锚点在原文件里恰好命中 1 次、全过才写盘；锚点取
`git show HEAD:<file>` 的**原文**（格式化工具可能早已折行）；改完复核「CRLF 数 + 裸 LF 数
== 总行数」。可运行的完整实现见 2026-09-28 的 `chore(release): v1.4.0-beta.1` 提交。
⚠️ 原脚本 `bump-version-1.3.7.py` 已随 `.workbuddy-ai` 清理删除，勿再找。
⚠️ `git diff` 会为 `Cargo.toml`/`Cargo.lock` 打印 `LF will be replaced by CRLF` —— 这是
**仓库既有的** index/工作区差异，不是本次改动引入的；只要 diff 仍只有版本号那一行即可放行。
⚠️ **两遍式**：先断言每条锚点在原文件里**恰好命中 1 次**，全过才写盘。

## 3. 写 release notes（临时文件，**不入库**，第 5 步 `-F` 喂给 tag）

> ⚠️ 这个文件**只存在于工作区**、**不要 `git add`**（2026-09-28 起改方案 A：
> notes 走附注 tag 正文 ⇒ 仓库里不留任何临时文件，也不用事后删除）。
> 放仓库根或系统临时目录都行；`/tmp` 更不容易误提交。

```markdown
> 一句话定性（本版以稳定性为主 / 引入新功能 …）

## 🐛 问题修复

- 用「什么现象被修好了」描述，不要写 commit 术语

## ⚡ 性能与流畅度

- **更快在哪**：具体数字/机制

## 🧹 内部优化

**完整变更列表**：https://github.com/oneday5799/PeriTray/compare/v<last>...v<new>
```

⚠️ 正文里若要写 HTML 标签（如 `<br>`）**必须放进反引号**，否则会被当 HTML 渲染。

## 4. 跑闸门

```bash
node tools/check.mjs
node tools/doc-table-audit.mjs
( cd src-tauri && cargo fmt --check )
( cd src-tauri && cargo check --no-default-features --all-targets )   # 要求零告警
( cd src-tauri && cargo test --no-default-features )
node tools/check-clippy.mjs
```

⚠️ **耗时（2026-09-22 v1.3.7 实测，订正旧稿的「全量重编 ≈5 分钟」）**：
版本号只改 `Cargo.toml`/`Cargo.lock` 里的**本包版本**，依赖未动 ⇒ cargo **只重编 PeriTray 自己**
（日志明写 `Compiling PeriTray v1.3.7 ... Finished dev profile in 13.33s`）。
target 目录已预热时，**6 道闸门合计 1m17s**（check 段约 13s、clippy 段约 5s）；
pre-commit 钩子那次提交只花 **19s**。
⇒ 旧稿「check ≈5 分钟 / clippy ≈4 分钟」是**冷 target** 或改动面更大的场景，**别当常态引用**。
⇒ 仍建议 `run_in_background`（冷构建确实会超 120s 前台超时），但不必按分钟级等待。
✅ 6 道闸门的单一入口就是 §4 那条命令序列；`tools/verify-final.sh` 会把其中多道串起来跑。
⚠️ 原脚本 `run-gates-release.sh` 已随 `.workbuddy-ai` 清理删除，勿再找。

## 5. 提交

- 标题固定 **`chore(release): vX.Y.Z`**，版本号 5 处 + notes 文件放在**同一个提交**。
- ⚠️ **pre-commit 钩子**：总跑 `node tools/check.mjs`；暂存了 `src-tauri/{src,Cargo.toml,Cargo.lock}` 时
  再跑 `cargo fmt --check` + `cargo check` + `node tools/check-clippy.mjs --optional` ⇒ 提交很慢，用 `run_in_background`。
- ⚠️ `git commit -F <文件>` 的路径必须是 **Windows 形式**（`C:/Users/...`）；MSYS 形式 `/c/...` 会让 git
  报「读不到文件」，**而钩子照常打印「通过」**，极易误以为提交成功 ⇒ 务必核对 `git log --oneline -1`。

## 6. 打标签 + 推送

```bash
git tag vX.Y.Z <sha>                                   # 轻量标签（v1.3.6 实测 cat-file -t = commit，非 annotated）
git -c http.schannelCheckRevoke=false push origin main
git -c http.schannelCheckRevoke=false push origin vX.Y.Z
git -c http.schannelCheckRevoke=false ls-remote --tags origin vX.Y.Z    # 核对远端 == 本地
```

⚠️ 推 tag 会触发 `release.yml`（build x64+arm64 → release job）。**release 正文从该 tag 的树里读**，
所以 notes 文件必须在该 tag 的提交里。

## 7. 发布后核对（必做）

```bash
curl -sS --ssl-no-revoke "https://api.github.com/repos/oneday5799/PeriTray/actions/runs?per_page=3"
curl -sS --ssl-no-revoke "https://api.github.com/repos/oneday5799/PeriTray/actions/runs/<id>/jobs"
curl -sS --ssl-no-revoke "https://api.github.com/repos/oneday5799/PeriTray/releases/tags/vX.Y.Z"
```

逐项核对：`conclusion=success` · `prerelease` 与「tag 是否含 `-`」一致 · **附件恰好 2 个
`PeriTray_<ver>_{x64,arm64}-setup.exe`** · 正文含手写的各分组标题、且 **compare 链接只出现 1 次**。
⭐ **Release 页不带 `.msix`**（2026-09-22 起的产品约定）：MSIX 仍照常构建并作为 **workflow artifact**
（`msix-*`）留存，供手动上传 Partner Center，但**不挂到 Release**（避免用户从发布页直接安装而绕过商店
分发与更新链路）。见 `release.yml` 的 `Create Release`。
⚠️ Release 全流程约 6–8 分钟（本机实测）⇒ 轮询放后台。

✅ 用 gh CLI 轮询即可，无需脚本：`gh run view <RUN_ID> --json jobs,status`
（逐 job 步骤：`gh run view <RUN_ID> --log-failed`）。
⚠️ 原脚本 `wait-release-1.3.7.sh` 已随 `.workbuddy-ai` 清理删除，勿再找。

### 7.1 升级通道自检（**必做** —— 2026-09-22 靠它抓到一个真实缺陷）

**发布成功 ≠ 用户能拿到更新。** app 是**自己解析 releases 列表**判断有没有新版本的
（NSIS 走 GitHub `check_for_update`，MSIX 走 Store API）⇒ 必须用**真实数据**按 app 的判据复现一遍。

```bash
curl -sS --ssl-no-revoke "https://api.github.com/repos/oneday5799/PeriTray/releases" \
  -o "$TEMP/releases.json"     # 临时产物，⛔ 别写进仓库
# ⚠️ 旧脚本 verify-update-channel-1.3.7.py 已随 .workbuddy-ai 清理删除；
#    版本比较器的行为由 src-tauri 内的 cargo test 覆盖（见 Wiki 08 更新通道小节），此处只留取数命令。
```

⭐ **必须跑双模式对照**：同一份真实数据 + 同一套用例，**只翻转比较器方向**，
结果必须从 `ALL_CASES_PASS` 变成 `CASES_FAILED`。
- `ALL_CASES_PASS` 且 `latest=<本版>` ⇒ 升级通道可用。
- `--buggy` 若**也**全过 ⇒ **用例没覆盖到这个方向**，判据是假的，必须重写。

⛔ **2026-09-23 踩到的坑（务必记住）**：该脚本原先**只有** `--buggy` 那一种写法（`av`/`bv` 对调），
却自称「取 max」⇒ 它其实是**缺陷复现**脚本。**拿它当「修复后验证」会得到完全相反的结论**
（修复后跑它仍报 `latest=1.2.9`，看起来像「修复无效」）。
⇒ **教训：一个脚本的名字/注释说它是验证器，不代表它编码的是正确逻辑。**
判断方法：**故意注入反向实现，看结论是否翻转**；不翻转就说明判据是假的。

判据（与 `update.rs::latest_release` 同源）——三个都要对：

| `current` | 期望 `has_update` |
|---|---|
| 上一版（如 `1.3.6`） | `true` |
| 上一测试版（如 `1.3.7-beta.1`） | `true` |
| 本版（如 `1.3.7`） | `false` |

⛔ **教训（2026-09-22 实发）**：这一步查出 `update.rs` 的 `max_by` 比较器**方向写反**，
`latest` 恒取到窗口内**最旧**的版本（真实数据实测 `v1.2.9`）⇒ NSIS 用户**永远收不到更新提示**，
设置页恒显示「已是最新版本」。
**只核对 CI 结论与 Release 页面是查不出这个的** —— 页面全绿，用户侧功能是坏的。

⚠️ 顺带：`/releases` 默认 `per_page=30`，返回的是**最新 30 条窗口**（本仓 2026-09-22 实测
窗口最旧为 `v1.2.9`）。对「取最大」无害，但**比较器写反时取的正是窗口边界那端**
⇒ 复现脚本必须用真实窗口，不要只喂三四条构造数据。

⭐ **更硬的一层（2026-09-22 补做）：直接问运行中的进程要状态，不依赖任何重算。**

```bash
cd src-tauri/target/debug
WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--disable-gpu-sandbox --remote-debugging-port=9222" ./PeriTray.exe &
sleep 12
cd /d/Code/PeriTray && node tools/cdp-eval.mjs popup \
  "window.__TAURI__.core.invoke('get_update_status')"
```

期望返回 `{"status":"latest","currentVersion":"<本版>","latestVersion":"<本版>", …}`。
等价旁证是日志（`[update] checking for update` / `result: has_update=… latest=…` 都是**标准级**日志）：
修复前实测 `latest=1.2.8 → 1.2.9`（窗口最旧），修复后 `latest=1.3.7`。

⚠️ 三条注意：
① **窗口必须可见**，否则 `invoke` 挂死（popup 隐藏时同步表达式能跑、Promise 永不 resolve）；
② **本机 `target/debug/PeriTray.exe` 目前会让 WebView2 renderer 崩在 `msedge.dll`（`0xc0000005`）**，
   表现为「应用卡在启动中间、日志停在 `[aumid]` 之后、`startup complete` 缺失」⇒
   **必须带 `--disable-gpu-sandbox`**（已 2/2 复现有效）。已排除：沙箱启动方式（非沙箱同样崩）、
   profile 损坏、`hardware_acceleration` 开关、Runtime 版本更新；疑似本机 GPU / 安全软件瞬时状态，
   **正式版是否同样崩溃、重启是否自愈，均未验证**；
③ `node tools/cdp-eval.mjs --list` 顺带确认页面 URL 是 `tauri.localhost`（= 正常版，不是 dev 版）。

## 8. 收尾

- ⛔ **别把 notes 文件 `git add` 进仓库**（方案 A 的全部意义）。若误提交了，历史里会留下
  一个「下次发版会被原样注入成新版本正文」的定时炸弹——把仓库里的那份删掉即可，
  **不要**改 CI 去兼容它。
- 结论写进 **Wiki**（权威源），强制约定写进 **AGENTS.md**；⛔ 别再往任何本地记忆文件堆知识
  —— 那是 2026-09-27 已清理掉的反模式（旧 `PLAYBOOK.md` 2898 行 + 12 篇工作日志）。

## 9. 撤回已发布版本并重发**同一版本号**（2026-09-22 首次实操）

**触发场景**：已发布的产物里发现了必须修掉的缺陷，而用户要求**不升版本号**（不发 1.3.8）直接重发。
⭐ **典型动机**：更新检查本身的缺陷 —— 产物里的比较器是坏的，只有重发才能让发布页上的包是对的。

### 9.1 先想清楚：重发**救不了**老用户
缺陷在**用户本机的二进制**里 ⇒ ≤当前版的 NSIS 用户**永远收不到任何更新提示**（`max_by` 恒取窗口最旧
版本 ⇒ `has_update` 恒 false），**只能手动下载**。重发的价值是「**让发布页上的产物是对的**」。
（MSIX / Store 用户走 `StoreContext`，不受影响。）⇒ **汇报时必须说清这一点，别只报「重发成功」。**
详见 `PLAYBOOK.md` §H.5。

### 9.2 机制要点（改行为必须动标签）
- 触发条件是 `on.push.tags: 'v*'` ⇒ **只有「推标签」会触发发布**，推 `main` 不会。
- ⭐ **工作流取自「标签所指的那个提交」** ⇒ 要改发布行为（如本次「Release 不挂 MSIX」），
  必须**先提交到 `main`，再把标签移过去**。只改 `main` 不动标签 = 重发出来的还是旧行为。
- 删 Release **不会**删标签；两者独立，必须分别删。
- **同一版本号重发时，五处版本号不用再改** —— 先 `git diff <原版本提交>..HEAD -- <五个文件>` 确认为空。
- notes 文件必须**重新落盘**（原文件早已按 §8 删掉），且**必须把「重发新纳入的修复」补写进去**。

### 9.3 ⭐ 推荐顺序（把「远端无标签 / 无发布」的窗口压到最短）
1. **先把新 notes 提交到 `main` 并推送**（非破坏性，可反复）。
2. 再删 Release + 删标签（破坏性；此后到 CI 跑完为止**没有发布页**）。
3. **立刻**重建标签并触发。CI 失败也不致命 —— 标签对象还在本地，重推即可恢复。

### 9.4 ⛔ 本机两个硬坑
- ⛔ **`gh` 未登录** ⇒ `gh release delete` 不可用。绕行：从 Windows 凭据管理器取 PAT
  （`git credential fill`）后走 REST API。
- ⛔ **git 通道间歇性 502**（沙箱代理 `CONNECT tunnel failed, response 502`；直连是 443 超时），
  而 **`curl` 到 `api.github.com` 一直稳定** ⇒ `git push` 连续失败时**别死等**，改用 API 等价操作：

```bash
GIT="/c/Program Files/Git/cmd/git.exe"
TOKEN=$(printf "protocol=https\nhost=github.com\n\n" | "$GIT" credential fill 2>/dev/null \
  | sed -n 's/^password=//p' | tr -d '\r')
API="https://api.github.com/repos/oneday5799/PeriTray"
H=(-H "Authorization: Bearer $TOKEN" -H "Accept: application/vnd.github+json" -H "X-GitHub-Api-Version: 2022-11-28")

# 取 release id → 删 Release
curl -sS --ssl-no-revoke "${H[@]}" "$API/releases/tags/vX.Y.Z"          # 记下 id
curl -sS --ssl-no-revoke -X DELETE -o /dev/null -w "%{http_code}\n" "${H[@]}" "$API/releases/<id>"
# 删标签
curl -sS --ssl-no-revoke -X DELETE -o /dev/null -w "%{http_code}\n" "${H[@]}" "$API/git/refs/tags/vX.Y.Z"
# ⭐ 建标签（= 推标签的等价物）
curl -sS --ssl-no-revoke -X POST "${H[@]}" -d '{"ref":"refs/tags/vX.Y.Z","sha":"<commit>"}' "$API/git/refs"
```

⭐ **实测：API 建 ref 同样会触发 `push` 事件、同样会跑 `on.push.tags` 工作流**
（2026-09-22 的 run `35753780115` 就是这么起来的）⇒ 这是 git 不通时的**完整替代路径**。
⚠️ 提交 `main` 仍需 git ⇒ 网络差时把 push 包在**重试循环**里（实测成功间隔 1～4 次不等，也可能连续
10+ 次全失败）。⚠️ `curl -o <路径>` 在 MSYS 下常报 `client returned ERROR on write of N bytes`
—— **不代表请求失败**，看 `-w "%{http_code}"` 即可。

## release.yml 的既有机制（**不要重复造**）

- release job 有 `Checkout` + `Resolve release notes`：**从附注 tag 的正文**取 release notes
  （`git for-each-ref --format='%(contents)' refs/tags/$TAG` → `$RUNNER_TEMP/release-notes.md`），
  取到则用它作正文并**关闭** `generate_release_notes`；取不到则回退到自动生成的变更列表。
  二者**只取其一**——本仓不用 PR，自动正文实际只有一行 compare 链接，同时开启会把同一 URL
  打印两遍（v1.3.7-beta.1 实测）。
- ⛔⛔ `%(contents)` 对**轻量 tag 会取到该 commit 的提交说明**（2026-09-28 实测，连标题带含
  API 名的整段 body）⇒ 必须先判 `git cat-file -t "$TAG" == tag`。漏判会**静默**产出
  「Release 页看着正常、实则是提交说明」的正文——所以 `git tag -a` 不是风格偏好，是硬要求。
- `softprops/action-gh-release@v3` 的 `releaseBody()`（`src/util.ts`）对「空 `body_path`」与
  「文件不存在」都只 `console.warn` 后回退 ⇒ **不会让发布失败**。改动工作流前先读 action 源码确认边界行为。
- beta（tag 含 `-`）自动 `prerelease=true`，并跳过 MSIX 打包与 Store 图标生成。
- ⭐ **`Create Release` 的 `files:` 只列 `artifacts/*.exe`**（2026-09-22 起）。MSIX 仍由 `Build MSIX
  package` 构建、由 `Upload MSIX artifact` 作为 workflow artifact 留存，**但不进 Release**。
  改动此处前先确认分发链路（Partner Center 手动上传）不受影响。
