# tools/local —— 值得复用的自包含脚本

> 2026-09-28 从 `.workbuddy-ai/scratch/keep/` 迁入 `tools/`（原 `.workbuddy-ai` 已整体删除）。
> 更早一次清理（2026-09-27）时这里已剔除过一次：一次性探针、Edge 浏览器 profile、
> commit-msg 草稿、旧 dist 快照等。留下的都是**能复现结论**的自包含脚本。
>
> ⚠️ **产物落盘**：截图与临时文件写到 `_out/`（已列进 `.gitignore`）；
> 少数脚本的 `OUT` 变量直接指向本目录，`*.png` 同样已忽略。
>
> **配套知识库**（结论、纪律、判据都在那里，脚本只是复现手段）：
> - 任务栏内嵌与自绘 → Wiki **15-任务栏内嵌与自绘**
> - 物理设备身份 → Wiki **16-物理设备身份**

## 通用注意事项

- ⚠️ **全部是「本机口径」脚本**：写死了 DPI 125%、任务栏 2560 宽、具体设备名等实测值。
  换机器 / 换分辨率 / 换 DPI 时**必须重跑并复核期望值**，不要直接相信脚本里的数字。
- ⚠️ 多数脚本**只读**（`probe_*` 完全只读）；少数会改 `config.toml`
  ⇒ 运行前**先备份**，且**先 `kill()` 进程再改文件**（应用运行期间改文件可能被退出时的内存态写回覆盖）。
- ⚠️ 本机**沙箱用作业对象管理进程树** ⇒ 从脚本拉起的**常驻程序活不过该脚本所在的那条命令**。
  涉及真实进程的验证必须把「启动 → 读 hwnd → 截图 → 统计」放进**同一个脚本**。
- ⚠️ 任务栏相关的截图/像素分析脚本：观测进程须先
  `SetProcessDpiAwarenessContext(-4)`；`BitBlt` 抓分层窗须带 `CAPTUREBLT`。
  **先怀疑观测口径（DPI 缩放），再怀疑代码。**

## 设备身份取证探针（只读，5 个）

用于重新验证 Wiki 16 里的**平台事实**（容器 GUID 版本位、`BaseContainers` 反向索引、
占位容器规模、HID 集合与容器的关系、音频端点容器解析失败率）。

| 脚本 | 验证的结论（Wiki 16 章节） |
|---|---|
| `probe_usb_container_port.py` | 容器形态分类 + 同型号多实例的容器一致性（§4.1） |
| `probe_container_derivation.py` | 版本位交叉表 + UUIDv5 复现尝试 + 幽灵设备检测（§4.1） |
| `probe_base_containers.py` | `BaseContainers` 反向索引结构（§4.4） |
| `probe_audio_no_container.py` | 音频端点无容器原因 + 占位容器规模（§4.2） |
| `probe_hid_intra_device_containers.py` | HID 多集合是否跨容器（§4.5） |

## 任务栏验收脚本（9 个）

| 脚本 | 验什么 | Wiki 15 对应坑 |
|---|---|---|
| `verify-widget.py` | **自包含全流程**：启动 + 日志解析 + DPI 感知 + 截图 + 像素统计 | §9.1 观测口径两坑 |
| `verify-widget-m2.py` | 里程碑 2 的内容接入 | — |
| `verify-widget-m3.py` | 里程碑 3（真实内容） | — |
| `verify-widget-drag.py` | 拖拽落位与记忆 | §5.3 坐标口径 |
| `verify-zorder.py` | Z 序重申，含**注入三阶段** | §6.4 判据坑 |
| `verify-hover-content.py` | hover **不侵蚀内容**（6 条判据） | §9.2 像素判据六坑 |
| `verify-hover-backdrop.py` | 底衬本体与尺寸（15 条判据） | §9.2 / §9.3 |
| `verify-content-scale.py` | 内容缩放两档 | §4 两条 DPI 口径 |
| `verify-settings-ui.py` | 前端接线（**后端全绿 ≠ 用户能选得到**） | Wiki 04 §5.3 |

### tooltip 专用（2026-09-28 新增）

| 脚本 | 验什么 |
|---|---|
| `verify-tooltip-struct.py` | 结构判据：tip 窗存在 + `TTM_GETTOOLCOUNT` = 设备数 + 逐条文本非空 |
| `verify-tooltip-pixels.py` | **像素判据**：`TTM_ACTIVATE` 强制显示后立刻抓屏 ⇒ 证明「文本真渲染」且「**没被任务栏盖住**」 |

⚠️ **两个坑（本次实测踩到，探针侧的错不是实现的错）**：
① `TTM_GETTOOLCOUNT` 的值是 **1037**（1083 是 `TTM_GETCURRENTTOOLW`）——凭印象写会读出假的 0；
② `TTM_ENUMTOOLS` 的语义是「lParam 指向数组、wParam = 容量」，**不是**「查某个 id」⇒ 逐 id 探测无效。
⛔ 另：**激活后要立刻抓屏**（tooltip 约 5s 自动隐藏，隔几秒再截必然扑空）。
⭐ `TTM_ACTIVATE` 是公开 API ⇒ **不需要注入鼠标**即可验收（本机无法注入鼠标）。


## 截图脚本（2 个）

`shot-widget-layout.py`（整条任务栏 + 4× 放大）· `shot-widget-m3.py`

## 注入 harness（3 个）

**反向注入自检**的范式实现：patch → 跑指定用例 → **无条件还原** + 校验和比对。
目的只有一个——**证明判据在未修复的代码上会转红**。

| 脚本 | 注入目标 |
|---|---|
| `inject-widget-layout.py` | 布局/文本格式化的 5 条判据 |
| `inject-widget-drag.py` | 拖拽相关判据 |
| `inject-pin-write-tests.py` | pin 写入侧判据 |

⭐ 两个必踩（脚本本身的设计缺陷，不是被测代码的）：
① `cpSync(src, dst)` 当 `dst` 已存在时会复制成 `dst/<basename>/`（**子目录**）⇒ 要递归逐文件覆盖；
② **用例之间必须 reset** —— 忘了会得到「上一条残留的叠加」，一度误判为「闸门判据不稳」。
