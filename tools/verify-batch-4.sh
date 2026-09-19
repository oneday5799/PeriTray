#!/bin/sh
# 第四批验收脚本（P2-3 ~ P2-10、P3-1 ~ P3-11，共 18 条）
#
# 用法：在仓库根执行 `sh tools/verify-batch-4.sh`
# 依据：主方案 §6.3「第四批：逐条按 §六 表格『验证』列执行」+ §6.2 的
#       「验证必须可证伪」原则（凡「修复前后取值相同」的检查一律不算验收）。
#
# ⚠️ 硬约定（沿用 verify-batch-2.sh）：
#   · `grep -c` 零匹配时退出码为 1 ⇒ 一律赋值后比较，不用 `set -e` / `&&`。
#   · 判据只能锚在**会真正变化的符号**上；写不出命令的条目一律进 MANUAL，
#     不给评审类条目编造「命令」（主方案 §6.3 明文批评过这种伪装）。
#
# 本脚本把 18 条分成四类：
#   命令类（跑具名单测）   —— 逐条列出**真实存在**的测试名（已用 `cargo test -- --list` 核对）
#   结构类（grep 判据）    —— 只对能机械判真伪的条目使用
#   INFO                   —— 存量计数，**不作为闸门**（值会随正常改动漂移）
#   MANUAL                 —— 注入类 / 评审类，脚本不做（会误报）

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

# 统计「代码行」中匹配正则的行数：剔除 `//` 行注释与 `//` 之后的内容，
# 并跳过块注释续行（首个非空白字符是 `*`）。
# ⚠️ 已知局限：字符串字面量里的 `//`（如 URL）会被误当注释截断；本仓当前无此情形。
code_count() { # code_count <正则> <文件...>
  pat="$1"
  shift
  awk -v pat="$pat" '
    { line=$0; sub(/\/\/.*/, "", line); t=line; sub(/^[ \t]+/, "", t)
      if (t ~ /^\*/) next
      if (line ~ pat) n++ }
    END { print n+0 }' "$@"
}

echo "== 第四批（18 条）验收 =="
echo
echo "-- 0) 先跑一次完整测试套件，下面逐条从结果里取（避免 18 次 cargo 调用）--"
RESULTS=$(cd src-tauri && cargo test --no-default-features 2>&1)
if printf '%s\n' "$RESULTS" | grep -q "test result: ok"; then
  printf 'PASS  测试套件整体通过：%s\n' "$(printf '%s\n' "$RESULTS" | grep 'test result:' | tail -1)"
else
  printf 'FAIL  测试套件未通过 —— 后续「命令类」判据将全部失败，请先看 cargo test 输出\n'
  fail=1
fi

chk_test() { # chk_test <条目> <完整测试名>
  if printf '%s\n' "$RESULTS" | grep -q "^test $2 [.]* ok$"; then
    printf 'PASS  %s：%s\n' "$1" "$2"
  else
    printf 'FAIL  %s：单测 %s 未通过或不存在\n' "$1" "$2"
    fail=1
  fi
}

# ── 命令类：逐条对应的具名单测 ───────────────────────────────────────
# 每条按主方案 §六「验证」列改写后的口径：抽纯函数 / 边界单测 / 含正控。
echo
echo "-- 命令类：具名单测（每条对应主方案 §六 该行的『验证』列）--"

# P2-4：重启门槛判定抽成纯函数（主方案要求拆成两个独立谓词，勿合并）
chk_test P2-4 tests::should_restart_only_after_two_consecutive_timeouts
chk_test P2-4 tests::is_time_jump_is_strictly_greater_than_8s
# P2-5：HRESULT 判定抽成纯函数（含「不过度宽泛」的反向断言）
chk_test P2-5 audio::tests::apartment_mode_conflict_is_recognized_and_not_over_broad
# P2-6：图标只写一次（比对两次调用的路径与 mtime）
chk_test P2-6 windows::tests::toast_icon_is_written_once_then_cached
# P2-8：上限判定抽成只接 `&mut Config` 的纯函数后，可脱离 AppHandle 直接测
chk_test P2-8 commands::tests::tray_device_limit_rejects_without_writing
chk_test P2-8 commands::tests::tray_device_limit_allows_when_one_below
chk_test P2-8 commands::tests::tray_device_toggle_removes_existing_even_at_limit
# P2-9：只删本应用生成格式（含「用户的 debug_user.log 不得被碰」）
chk_test P2-9 process::tests::managed_log_name_accepts_only_own_formats
chk_test P2-9 process::tests::should_delete_log_respects_retention_boundary
# P2-10：「让出执行器」这条性质由该用例证伪（改回阻塞式 sleep 即转红）
chk_test P2-10 windows::tests::dpi_settle_wait_yields_the_executor
# P3-1：响应体上限的边界两侧
chk_test P3-1 update::tests::body_limit_boundary_is_exclusive_at_exact_size
chk_test P3-1 update::tests::body_limit_covers_realistic_and_degenerate_sizes
# P3-2：版本号缺失段按 0 补齐（并保留「真升级仍能被检出」的正控）
chk_test P3-2 update::tests::compare_versions_ignores_missing_trailing_segment
chk_test P3-2 update::tests::version_nums_treat_trailing_zeros_as_equal
chk_test P3-2 update::tests::compare_versions_still_detects_real_bumps
# P3-3：回差重新武装（含「抖动带内不得重复提醒」的反向断言）
chk_test P3-3 battery_notify::tests::rearm_boundary_excludes_the_jitter_band
chk_test P3-3 battery_notify::tests::recharge_then_drain_notifies_again
chk_test P3-3 battery_notify::tests::rearm_is_per_threshold
chk_test P3-3 battery_notify::tests::jitter_within_band_does_not_re_alert
# P3-5：超长路径走一遍转换（不 panic 且结果完整）
chk_test P3-5 app_icon::tests::long_path_survives_wide_round_trip
chk_test P3-5 app_icon::tests::nt_length_guard_rejects_oversized_reports
chk_test P3-5 app_icon::tests::split_device_path_splits_volume_and_rest
# P3-6：复用 P1-8 的 CAS 单飞守卫，故随 P1-8 的四个用例验收
chk_test P3-6 state::tests::animation_guard_is_exclusive_and_released_on_drop
chk_test P3-6 state::tests::animation_guard_released_when_animation_panics
chk_test P3-6 state::tests::animation_flag_is_reclaimed_after_timeout
chk_test P3-6 state::tests::animation_timeout_boundary
# P3-9：字段级容错 + 集中归一化（含「合法值不得被动」的反向断言）
chk_test P3-9 config::tests::invalid_enum_value_does_not_kill_whole_config
chk_test P3-9 config::tests::normalize_config_replaces_invalid_values_field_by_field
chk_test P3-9 config::tests::normalize_config_keeps_valid_values_untouched
chk_test P3-9 config::tests::normalize_config_battery_threshold_rules_match_frontend
chk_test P3-9 config::tests::default_config_roundtrips_unchanged
chk_test P3-9 config::tests::write_path_finalize_normalizes_before_serializing
chk_test P3-9 config::tests::missing_fields_fall_back_to_field_defaults_not_zero_values
# P3-11：注册 -> 读取 -> 等待 的顺序（含「先读后注册会丢变更」的证伪用例）
chk_test P3-11 tray::tests::theme_watch::register_is_called_before_read
chk_test P3-11 tray::tests::theme_watch::reading_before_registering_would_lose_the_change
chk_test P3-11 tray::tests::theme_watch::change_between_register_and_load_is_captured
chk_test P3-11 tray::tests::theme_watch::read_still_happens_even_if_register_fails

# ── 结构类：能机械判真伪的条目 ───────────────────────────────────────
echo
echo "-- 结构类：grep 判据 --"

# P2-3：前端惰性访问层的**边界**——`window.__TAURI__` 的代码行只允许出现在 common.js
# （其余文件仅在注释里提及它）。注意：本条的可证伪主判据仍是 MANUAL 里的注入项，
# 这条结构判据只保证「没有别处又冒出一个裸访问」。
n=$(code_count 'window[.]__TAURI__' $(find src-tauri/dist/scripts -name '*.js' | grep -v '/common.js$'))
chk_eq "P2-3：common.js 之外的代码行不得访问 window.__TAURI__" "$n" 0
n=$(grep -c "function onTauriEvent" src-tauri/dist/scripts/common.js || true)
chk_eq "P2-3：惰性事件订阅入口 onTauriEvent 存在" "$n" 1

# P2-7：锁内只收集、锁外发送——两个阶段必须拆成独立函数才可能做到
n=$(grep -c "pub fn collect_pending_notices" src-tauri/src/battery_notify.rs || true)
chk_eq "P2-7：收集阶段 collect_pending_notices 存在" "$n" 1
n=$(grep -c "pub fn emit_notifications" src-tauri/src/battery_notify.rs || true)
chk_eq "P2-7：发送阶段 emit_notifications 存在" "$n" 1

# P2-8：读与写必须物理上落在同一次加锁内 ⇒ 调用方只允许出现一次 `with_config_mut`
# ⚠️ 必须用 code_count 而非 `grep -c`：`commands.rs:333` 的**文档注释**里引用了同一调用
#    形态（用来说明改法），`grep -c` 会把它算成第 2 处 —— 首次运行即产出这个假阳性。
n=$(code_count 'with_config_mut[(][|]c[|] try_toggle_tray_device' src-tauri/src/commands.rs)
chk_eq "P2-8：toggle 的检查+写入合并在同一次 with_config_mut 内" "$n" 1
n=$(code_count 'fn try_toggle_tray_device[(]c: &mut Config' src-tauri/src/commands.rs)
chk_eq "P2-8：纯函数 try_toggle_tray_device 存在（脱离 AppHandle 可测）" "$n" 1

# P2-10：异步上下文里的等待必须用 tokio 版；AGENTS.md 与 check.mjs 都要登记
n=$(grep -c "tokio::time::sleep(std::time::Duration::from_millis(200)).await" src-tauri/src/windows.rs || true)
chk_eq "P2-10：DPI 等待使用 tokio::time::sleep(..).await" "$n" 1
n=$(grep -c "异步上下文禁止" AGENTS.md || true)
chk_eq "P2-10：AGENTS.md 已登记「异步上下文禁止 std::thread::sleep」" "$n" 1
n=$(grep -c "异步上下文里的阻塞 sleep" tools/check.mjs || true)
chk_eq "P2-10：check.mjs 头部已登记该类别无法静态覆盖" "$n" 1

# P3-7：约定必须写进 AGENTS.md（分类表本身是评审类）
n=$(grep -c "忽略 .Result. / .let _ =. 必须写明" AGENTS.md || true)
chk_eq "P3-7：AGENTS.md 已登记「忽略 Result / let _ = 必须写明为何安全」" "$n" 1

# P3-8：check.mjs 头部必须有「已知无法覆盖的类别」一节（与 AGENTS.md 防护边界互相印证）
n=$(grep -c "已知无法覆盖的类别" tools/check.mjs || true)
chk_eq "P3-8：check.mjs 头部含「已知无法覆盖的类别」一节" "$n" 1

# P3-10：锁序登记表五节齐全 + 加锁入口唯一性（本轮 P3-10 遗留项收敛后新增的判据）
for sec in "## 一、允许的嵌套边" "## 二、禁止的反向边" "## 三、锁清单" "## 四、持锁做 I/O" "## 五、自查方法"; do
  n=$(grep -c "//! $sec" src-tauri/src/state.rs || true)
  chk_eq "P3-10：state.rs 锁序登记含「$sec」" "$n" 1
done
# `.lock()` 的代码行只允许出现在 state.rs（lock_unpoisoned 的实现 + 中毒单测）
n=$(code_count '[.]lock[(][)]' $(git ls-files 'src-tauri/src/*.rs' | grep -v '/state.rs$'))
chk_eq "P3-10：state.rs 之外的代码行不得出现裸 .lock()（加锁入口唯一）" "$n" 0

# ── INFO：存量计数（会随正常改动漂移，**不作为闸门**）─────────────────
echo
echo "-- INFO：存量计数（仅记录，不判失败）--"
printf 'INFO  P3-7：全仓 `let _ = ` 命中 %s 处（报告写作时为 115；该数字随正常改动漂移，故不作判据）\n' \
  "$(grep -rn "let _ = " src-tauri/src/ --include=*.rs | wc -l | tr -d ' ')"

# ── MANUAL：注入类 / 评审类（不给它们编造命令）───────────────────────
echo
cat <<'EOF'
MANUAL  以下条目按主方案 §六「验证」列只能靠**注入**或**评审**，脚本不做（会误报）：

  · P2-3【注入】在 DevTools 里 `delete window.__TAURI__`，随后触发一次事件监听路径，
    断言走防御式分支（invoke 返回 rejected Promise，而不是抛 `TypeError`），再刷新恢复。
    ⚠️ 主方案第二十一轮已明确：`node tools/check.mjs` 与「两页手测」都验不了本条
       （前者不查运行时时序，后者时序敏感、修复前后都可能通过）。上面那条结构判据
       只是「边界检查」，**不能替代本注入项**。
  · P2-5【评审】COM 公寓契约的文字部分（「进程内统一 STA，不做反初始化」）只能人读。
  · P2-7【注入】在 `emit_notifications` 发送通知前 `get_devices_cache().try_lock()`，
    断言**成功**（修复前在锁内发送会失败）。
    ⚠️ 主方案第二十二轮：真实符号是 `state.rs` 的 `get_devices_cache()`，
       **不是** `BT_CACHE_LOCK`（后者全仓不存在，原稿是凭空写的）。
  · P2-8【压力手测】脚本化并发发起 N 次 `toggle_device_tray`，断言
    `tray_devices.len()` 从不超过 `TRAY_DEVICE_LIMIT`（= 4）。人手的连点间隔远大于
    竞态窗口，**修复前后都可能通过**，故不算验收。
    （主方案第二十二轮已说明：纯函数单测天然复现不了 TOCTOU；上面的单测覆盖的是
      上限判定本身，不是并发窗口。）
  · P3-7【评审】`let _ =` 分类表本身。
  · P3-8【评审】check.mjs 头部与 AGENTS.md「防护边界」的一致性。
  · P3-10【评审】锁序白名单与「持锁区不得调用」清单的语义正确性（脚本只能查存在性）。
  · P3-11【注入】在「读取点」插入 `sleep(50ms)` 人为撑开窗口，切换主题，观察是否被捕获。
    （现有四个 theme_watch 单测已用可注入桩覆盖了同一机制，注入项是端到端补充。）

  另注：**P3-4 不在本批**。它是 P1-10 的硬前置条件，已在第二批执行并验收
  （见 tools/verify-batch-2.sh），主方案第二十三轮已把它的「改法+验证」正式移出 §六。
EOF

echo
if [ "$fail" = "0" ]; then
  echo "第四批验收：通过（含 MANUAL 项，需另行执行）"
else
  echo "第四批验收：**未通过**"
  exit 1
fi
