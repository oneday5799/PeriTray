#!/bin/sh
# 第一批验收脚本（P0-1、P0-2、P3-13：单飞守卫的绑定与上移）
#
# 用法：在仓库根执行 `sh tools/verify-batch-1.sh`
# 依据：主方案 §6.3 的实操建议 + 该表「第一批」行的两条命令（**其中一条已按实测订正**）。
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

echo "== 第一批（P0-1 / P0-2 / P3-13）验收 =="

# ── 命令类 ────────────────────────────────────────────────────────
# P0-1/P3-13：外层守卫不得带下划线（下划线会消掉 unused_variables，
# 而「未被引用」正是 move 闭包捕获失败的直接原因）。
n=$(grep -c "let Some(_guard)" src-tauri/src/bluetooth.rs || true)
chk_eq "P0-1：bluetooth.rs 的 let Some(_guard) 已清零" "$n" 0

# P3-13：SingleFlightGuard 的**定义**已上移到 state.rs，wireless_24g/mod.rs 只剩 use + 调用。
# ⚠️ 判据必须落在 `struct`（定义）上，不能写成 `grep -c "SingleFlightGuard"`——
#    改法是「改为 use crate::state::SingleFlightGuard; 复用」，加了 use 之后
#    该文件至少仍有 3 处命中，**永远不可能为 0**（主方案 §6.3 第二十二轮已修正）。
n=$(grep -c "struct SingleFlightGuard" src-tauri/src/wireless_24g/mod.rs || true)
chk_eq "P3-13：wireless_24g/mod.rs 已无 SingleFlightGuard 定义" "$n" 0
n=$(grep -c "struct SingleFlightGuard" src-tauri/src/state.rs || true)
chk_eq "P3-13：定义已落在 state.rs（正控）" "$n" 1

# ── ⚠️ 已按实测订正的判据 ──────────────────────────────────────────
# 主方案 §6.3 原写：`grep -c "let Some(_guard)" src-tauri/src/{bluetooth.rs,wireless_24g/mod.rs}`
# **应为 0**（当时 1+2=3）。
# 2026-09-18 实测：bluetooth.rs = 0 ✅，但 **wireless_24g/mod.rs = 1**。
# 人工核对后确认该处**是有就地说明的正当例外**（`snapshot_fresh`：函数同步跑完即返回、
# 不存在 move 闭包，守卫绑到函数结束承担 Drop 职责；其上方注释明写「请勿按 AGENTS.md 的
# 规则改写它——那条规则只针对需要被闭包捕获的绑定」）。
# ⇒ **原命令过宽**（它把「内层 Drop-only 守卫」也算成违规）。改为**判别式**判据：
#    「每个残留的 let Some(_guard) 都必须有就地说明」——既承认正当例外，
#    又能抓住「新增的、没有说明的 `_guard`」。
total=0
documented=0
for f in src-tauri/src/bluetooth.rs src-tauri/src/wireless_24g/mod.rs; do
  for ln in $(grep -n "let Some(_guard)" "$f" 2>/dev/null | cut -d: -f1); do
    total=$((total + 1))
    if sed -n "$((ln - 3)),$((ln - 1))p" "$f" | grep -q "正确写法"; then
      documented=$((documented + 1))
      printf 'INFO  正当例外：%s:%s（其上方 3 行内有「正确写法」说明）\n' "$f" "$ln"
    else
      printf 'FAIL  未说明的 let Some(_guard)：%s:%s（要么去掉下划线，要么补就地说明）\n' "$f" "$ln"
      fail=1
    fi
  done
done
printf 'INFO  let Some(_guard) 合计 %s 处，其中带说明 %s 处\n' "$total" "$documented"

# ── 人工/注入类（不给它们编造命令）──────────────────────────────────
cat <<'EOF'
MANUAL  以下两条按主方案 §6.3 只能靠**注入**验证，脚本不做（会误报）：
  · P0-1：在 `enqueue_bt_refresh` 的**线程体内**插 `sleep(2s)` 撑开窗口，**并发调用两次**，
    断言第二次被拒（只跑 1 个 worker）。
    ⚠️ 注入点必须在**线程体内**——插在回调入口等于插在「没被移动的那部分」，
       修复前后都会「卡」，测试不可证伪（主方案 §6.3 第二十二/二十三轮已修正）。
    ⚠️ 不能改成「把 CAS 改为恒失败」——那样 `SingleFlightGuard::new` 返回 None、
       直接 return，**未修复的代码同样不会启动线程**，同样不可证伪。
  · P0-2：同族（失败路径回滚），按 §3.2 的「验证」块执行。
EOF

echo
if [ "$fail" = "0" ]; then
  echo "第一批验收：通过（含人工/注入项，需另行执行）"
else
  echo "第一批验收：**未通过**"
  exit 1
fi
