#!/bin/sh
# 第二批验收脚本（P0-3、P1-1~P1-5、P1-10、P3-12、P3-4）
#
# 用法：在仓库根执行 `sh tools/verify-batch-2.sh`
# 依据：主方案 §6.3 的实操建议 + 该表「第二批」行的命令。
#
# ⚠️ 硬约定：`grep -c` 零匹配时退出码为 1 ⇒ 一律赋值后比较，不用 `set -e` / `&&`。

cd "$(dirname "$0")/.." || exit 2

fail=0
chk_eq() { # chk_eq <名称> <实得> <期望>
  if [ "$2" = "$3" ]; then
    printf 'PASS  %s（实得 %s）\n' "$1" "$2"
  else
    printf 'FAIL  %s（期望 %s，实得 %s）\n' "$1" "$3" "$2"
    fail=1
  fi
}
chk_ge() { # chk_ge <名称> <实得> <下限>
  if [ "$2" -ge "$3" ] 2>/dev/null; then
    printf 'PASS  %s（实得 %s ≥ %s）\n' "$1" "$2" "$3"
  else
    printf 'FAIL  %s（期望 ≥ %s，实得 %s）\n' "$1" "$3" "$2"
    fail=1
  fi
}

echo "== 第二批（P0-3 / P1-1~P1-5 / P1-10 / P3-12 / P3-4）验收 =="

# ── P3-12：真正的判据是「行为」，而实施时已把注入点做进代码 ─────────────
# 实施把 4 个 COM 回调统一收敛到 `request_sync_callbacks(hwnd)`，并为单测留了
# 可注入版本 `request_sync_callbacks_with(hwnd, post)`（主方案 §4.7 的「三选一」之③）。
# 故**行为判据直接跑那个单测**——它注入必然失败的投递桩，断言失败后标志回到 false，
# 并带**正控**（注入成功的桩，断言标志保持 true）。
echo "-- P3-12 行为判据（单测，含正控）--"
if (cd src-tauri && cargo test --no-default-features sync_pending_flag_rolls_back_when_post_fails 2>&1) \
  | grep -q "test result: ok. 1 passed"; then
  printf 'PASS  P3-12：sync_pending_flag_rolls_back_when_post_fails 通过（1 passed）\n'
else
  printf 'FAIL  P3-12：单测未通过（或未找到）——请人工看 cargo test 输出\n'
  fail=1
fi

# 结构判据①：**WM_SYNC_CALLBACKS 的投递点**已收敛为 1 处（即 `post_sync_callbacks` 这一层），
# 且它**没有被 `let _ =` 吞掉**——这是 P3-12 的失效模式本身。
#
# ⚠️ 判据必须锚在 **WM_SYNC_CALLBACKS** 上。第一版写成 `grep -c "let _ = PostMessageW("`
#    （不带消息名）**实测得 3**，全是**假阳性**：
#      · `:37` / `:680` 是**文档注释**里引用旧写法（说明缺陷用的），不是代码；
#      · `:602` 是 `request_shutdown()` 投递 **WM_CLOSE**，其上方注释明写
#        「**best-effort**：仅覆盖正常退出路径」——失败**没有状态后果**
#        （不像 P3-12 的 `SYNC_CALLBACKS_PENDING` 会永久停在 true），
#        属决策 9「只梳理**写入类** `let _ =`」的**有意豁免**。
#    ⇒ 又一次印证「**写得出命令 ≠ 命令是对的**」：判据要么锚在会真正变化的符号上，
#       要么先把假阳性逐条看过再定阈值。
n=$(grep -c "PostMessageW(Some(hwnd), WM_SYNC_CALLBACKS" src-tauri/src/audio_notify.rs || true)
chk_eq "P3-12：WM_SYNC_CALLBACKS 投递点已收敛为 1 处（可注入层）" "$n" 1
n=$(grep -c "let _ = PostMessageW(Some(hwnd), WM_SYNC_CALLBACKS" src-tauri/src/audio_notify.rs || true)
chk_eq "P3-12：该投递点未被 let _ = 吞错" "$n" 0
printf 'INFO  P3-12：`:602` 的 `let _ = PostMessageW(…, WM_CLOSE, …)` 是**有意豁免**\n'
printf '      （request_shutdown 为 best-effort，失败无状态后果），不计入本判据。\n'

# 结构判据②：注入缝存在（定义 1 处 + 至少 1 个调用点/单测）。
n=$(grep -c "fn post_sync_callbacks" src-tauri/src/audio_notify.rs || true)
chk_eq "P3-12：可注入投递层 fn post_sync_callbacks 存在" "$n" 1
n=$(grep -c "request_sync_callbacks_with" src-tauri/src/audio_notify.rs || true)
chk_ge "P3-12：request_sync_callbacks_with 有调用点（定义 + 单测）" "$n" 2

# ⚠️ **主方案 §6.3 的 P3-12 结构命令①已失效，此处显式登记**（不执行，只记录）：
#   原命令：grep -A3 "PostMessageW(Some(self.hwnd), WM_SYNC_CALLBACKS" src-tauri/src/audio_notify.rs \
#             | grep -c "SYNC_CALLBACKS_PENDING.store(false"     期望输出 4
#   2026-09-18 实测：**输出 0**。原因不是修复退步，而是**实施把 4 个调用点收敛成了
#   1 个 helper**（`post_sync_callbacks`），于是 `PostMessageW(Some(self.hwnd), …)` 这个
#   **字面形态在文件里已不存在**（现在是 `PostMessageW(Some(hwnd), …)`，参数名从
#   `self.hwnd` 变成 `hwnd`）。回滚逻辑也随之**从 4 处收敛到 1 处**（在
#   `request_sync_callbacks_with` 的失败分支里），所以「期望 4」这个数字本身也失去了意义。
#   ⇒ 判据已改为上面的「行为单测 + 吞错写法归零 + 注入缝存在」三条。
printf 'INFO  P3-12：主方案 §6.3 的「PostMessageW 后跟回滚，期望 4」命令已失效（实测 0），\n'
printf '      因实施把 4 个调用点收敛为 1 个 helper；已改用行为单测 + 注入缝判据。\n'

# ── P3-4：内联 oncontextmenu 已清零（P1-10 收紧 CSP 的硬前置）────────────
for f in src-tauri/dist/popup.html src-tauri/dist/settings.html; do
  n=$(grep -c "oncontextmenu" "$f" || true)
  chk_eq "P3-4：$f 的内联 oncontextmenu 已清零" "$n" 0
done

# ── 人工/注入类（不给它们编造命令）──────────────────────────────────
cat <<'EOF'
MANUAL  以下按主方案 §6.3 只能靠**注入**验证，脚本不做（会误报）：
  · P1-1：在 `std::thread::spawn` 的**闭包体内**（或 `update_audio_devices_menu()` 开头）
    插 `sleep(500ms)`，断言 UI 不卡。
    ⚠️ 注入点必须在**被移动的那段工作内部**——§4.2 的改法只把重操作 spawn 到子线程，
       轻量部分（config::with_config / AUTO_START.store / update_auto_text）**有意留在
       回调里**（该回调由 tao 主循环在主线程上执行，见 AGENTS.md）；插在「回调入口」等于插在
       留下的那部分，**修复前后都会卡**，不可证伪
       （主方案 §6.3 第二十三轮已修正）。
  · P0-3 / P1-2 / P1-3 / P1-4 / P1-5 / P1-10：本表未列，按其各自小节的「验证」执行
    （口径见 §6.3 的「统一口径」注）。
EOF

echo
if [ "$fail" = "0" ]; then
  echo "第二批验收：通过（含人工/注入项，需另行执行）"
else
  echo "第二批验收：**未通过**"
  exit 1
fi
