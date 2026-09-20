#!/bin/sh
# 收尾验收脚本
#
# 用法：在仓库根执行 `sh tools/verify-final.sh`
# 依据：主方案 §8.3 第 1 / 5 / 9 / 15 / 18 条 + §6.3 的统一口径。
#
# ⚠️ **本文里的 `§x` 指针指向的是已移除的《修复方案》**——那 6 份审查 / 整改期过程档案
#    已于 2026-09-20 融合进 Wiki 与 AGENTS.md 后删除。**保留原指针是为了让这份历史验收
#    脚本仍可对账**；现行出处见 Wiki：12-代码审查与整改复盘 / 13-架构决策与纪律 /
#    14-工程实践与验收方法论。
#
# ⚠️ 硬约定（沿用 `verify-batch-{0,1,2,4}.sh`）：
#   · `grep -c` 零匹配时退出码为 1 ⇒ 一律**赋值后比较**，不用 `set -e` / `&&`。
#   · 判据只能锚在**会真正变化的符号**上；写不出命令的条目一律进 MANUAL，
#     **不给评审类条目编造「命令」**（主方案 §6.3 明文批评过这种伪装）。
#   · 本脚本**不接入** pre-commit 与 CI（与其它 `verify-*.sh` 同性质：一次性验收实用脚本）。
#     其中文档体检（第 2 段）**已单独接入 CI**——它是可长期承重的闸门，见 `.github/workflows/ci.yml`。
#
# 四段：
#   1) 闸门段 —— §8.3 第 1 条的四道回归闸门 + clippy（§8.3 第 5 条）
#   2) 文档段 —— §8.3 第 15 / 18 条，并**自测这两条判据是否仍然承重**
#   3) 脚本段 —— 四批验收脚本的语法机检 + 第四批实跑
#   4) MANUAL —— 评审类 / 需真实进程的端到端项（脚本不做，会误报）

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
chk_ge() { # chk_ge <名称> <实得> <下界>
  if [ "$2" -ge "$3" ] 2>/dev/null; then
    printf 'PASS  %s（实得 %s ≥ %s）\n' "$1" "$2" "$3"
  else
    printf 'FAIL  %s（下界 %s，实得 %s）\n' "$1" "$3" "$2"
    fail=1
  fi
}
gate() { # gate <名称> <工作目录> <命令...>
  name="$1"; dir="$2"; shift 2
  if out=$(cd "$dir" && "$@" 2>&1); then
    printf 'PASS  %s\n' "$name"
  else
    printf 'FAIL  %s\n' "$name"
    printf '%s\n' "$out" | tail -25
    fail=1
  fi
}

echo "== 收尾验收 =="

# ── 1) 闸门段 ────────────────────────────────────────────────────────
echo
echo "-- 1) 闸门段（§8.3 第 1 条的四道回归闸门 + §8.3 第 5 条的 clippy）--"

gate "闸门 1/5：前端完整性（node tools/check.mjs）" . node tools/check.mjs
gate "闸门 2/5：cargo fmt --check" src-tauri cargo fmt --check

# 第 3 道：cargo check 零告警。⚠️ **判据机制必须与 `tools/pre-commit` 一致**：
# 裸 `cargo check` 有告警也返回 0，故须**解析输出里的 `^warning`**（钩子用的就是这个），
# **不要改用 `RUSTFLAGS=-D warnings`**——那会因指纹变化触发**全量重编**（实测 >4 分钟），
# 而本机钩子路径只要约 3s。零告警要求见 AGENTS.md「提交自动闸门」。
out=$(cd src-tauri && cargo check --no-default-features --all-targets 2>&1)
rc=$?
if [ "$rc" != "0" ]; then
  printf 'FAIL  闸门 3/5：cargo check --no-default-features --all-targets 失败\n'
  printf '%s\n' "$out" | tail -25
  fail=1
elif printf '%s\n' "$out" | grep -q '^warning'; then
  printf 'FAIL  闸门 3/5：cargo check 存在 warning（要求零告警）\n'
  printf '%s\n' "$out" | grep -n '^warning' | head -10
  fail=1
else
  printf 'PASS  闸门 3/5：cargo check --no-default-features --all-targets（零告警）\n'
fi

# 第 4 道：clippy（§8.3 第 5 条，单一来源在 tools/check-clippy.mjs；本地带 --optional）
gate "闸门 4/5：cargo clippy（node tools/check-clippy.mjs --optional）" . node tools/check-clippy.mjs --optional

# 第 5 道：测试套件。逐行取 `test result: ok. N passed` 求和（套件含多个测试二进制）。
RESULTS=$(cd src-tauri && cargo test --no-default-features 2>&1)
if printf '%s\n' "$RESULTS" | grep -q 'FAILED'; then
  printf 'FAIL  闸门 5/5：测试套件有失败用例\n'
  printf '%s\n' "$RESULTS" | grep -E 'FAILED|test result:' | tail -10
  fail=1
else
  total=0
  for n in $(printf '%s\n' "$RESULTS" | sed -n 's/.*result: ok\. \([0-9]*\) passed.*/\1/p'); do
    total=$((total + n))
  done
  chk_ge "闸门 5/5：cargo test --no-default-features 通过数" "$total" 176
fi

# ── 2) 文档段 ────────────────────────────────────────────────────────
echo
echo "-- 2) 文档段（§8.3 第 15 条列数体检 + 第 18 条自指型断言体检）--"

gate "文档体检：主仓交付稿（AGENTS.md / README.md）的表格列数 + 自指型指针（node tools/doc-table-audit.mjs）" . \
  node tools/doc-table-audit.mjs

# 元判据：**判据自身是否仍然承重**。自测夹具故意含 4 种缺陷，
# 期望「退出码 1 且恰好报 4 个问题」——少于 4 即某条判据已失效（例如正则被写窄）。
out=$(node tools/doc-table-audit.mjs tools/doc-table-audit.selftest.md 2>&1)
rc=$?
n=$(printf '%s\n' "$out" | grep -c '✗' || true)
chk_eq "文档段：自测夹具仍能转红（退出码）" "$rc" 1
chk_eq "文档段：自测夹具的问题数（4 条判据各自承重）" "$n" 4

# §8.3 第 5 条落地形态：clippy 与文档体检都必须真的在 CI 里
n=$(grep -c 'node tools/check-clippy.mjs' .github/workflows/ci.yml || true)
chk_eq "§8.3 第 5 条：CI 已接入 check-clippy.mjs" "$n" 1
n=$(grep -c 'node tools/doc-table-audit.mjs' .github/workflows/ci.yml || true)
chk_eq "§8.3 第 15/18 条：CI 已接入 doc-table-audit.mjs" "$n" 1

# ── 3) 脚本段 ────────────────────────────────────────────────────────
echo
echo "-- 3) 脚本段（四批验收脚本的语法机检 + 第四批实跑）--"

for s in tools/verify-batch-0.sh tools/verify-batch-1.sh tools/verify-batch-2.sh \
         tools/verify-batch-4.sh tools/verify-final.sh; do
  if sh -n "$s" 2>/dev/null; then
    printf 'PASS  语法机检：%s\n' "$s"
  else
    printf 'FAIL  语法机检：%s\n' "$s"
    fail=1
  fi
done

gate "第四批验收：sh tools/verify-batch-4.sh" . sh tools/verify-batch-4.sh

# ── 4) MANUAL ────────────────────────────────────────────────────────
echo
cat <<'EOF'
MANUAL  以下条目脚本不做（会误报），须按主方案指定方式另行执行：

  · §8.3 第 9 条【回归轮】重点覆盖配置读写（P1-3/P1-7/P1-11）、快捷键（P1-5/P2-12）、
    弹窗动画（P1-8/P3-6，**两条路径都要**）、CSP 生效后的前端全功能（P1-10，
    6 个下拉框各点一次）。**已在 ⑧-e 用真实进程 + CDP 执行完毕**
    （提交 `86ea8f9`；10 项判据的实得值见该提交 body 与当日工作日志）。
  · §8.3 第 14 条【评审】「回归风险 → 验证」覆盖关系：凡「回归风险」出现
    需确认 / 需扫一遍 / 必须核对 / 需逐处确认 / 需前后端同步改 的，落地前必须
    二者其一（提升为「验证」/ 显式写「接受该风险，不验证」）。**「什么算确认动作」有边界模糊，
    故本条判据是评审，不要给它编造命令。**
    ✅ **已执行（2026-09-20，B1）**：§6.4 清点出的 7 条缺口**逐条处置完毕** ——
    §4.3（首启图标缺失）与 §4.6（前端提示文案）**提升为验证**；§5.1（扫全部 showToast
    调用点，实得 10 文件 / 45 处 / 仅 4 处刻意的 \n）与 §5.5（12 个静态量逐处语义确认，
    全部纯内存、无 OS 句柄）**已实测执行**；§4.2（图标延迟）/ §5.6（factory 跨次复用，
    该改法已 A/B 否决）/ §5.7（日志噪声，5 个调用点全为事件驱动、无放大面）
    **显式接受并给出理由**。逐条实得见主方案 §6.4 的处置表与 §十一 勘误 #15。
  · §8.3 第 18 条【评审】`见 §…` 指针的**第二、三级**（「指对」+「内容相符」）：
    脚本只能做第一级（存在性）与「裸占位符」的机械检出；**「目标章节是否真的含有所指内容」
    只能人读**。本仓实测：引号内的 `§N.x` 一律是元文本（引用旧文本），脚本按
    「提及 ≠ 使用」归入 INFO，**这些仍需人工确认没有真断指针混在里面**。
    ✅ **已执行（2026-09-20，B4）**：主方案 `见 §…` **78 处 / 33 个唯一目标**、本计划文件
    **45 处 / 18 个**，第二级**全部指对**；第三级**逐条核实内容相符**（§6.1~§6.4 / §1.2 /
    §1.3 / §1.5④ / §4.1 / §4.7 / §5.3 / §9.1 / §9.2 / §9.4 / §十一 硬提醒）；
    doc-table-audit 的全部 INFO 项**逐条判定为元文本**，无真断指针混入。
    **顺带抓到 2 处失效**：避坑清单表头上界陈旧（写「一~二十七轮」而第 30 条
    是第二十八轮实测）与 §9.4 结果行的占位符计数过期（13 → 实得 31），
    根因均为 §9.4 的判据**只核下界 / 未重 derive**，已就地订正。
  · §8.3 第 16 / 17 条【评审】专题 / 批次 / 未闭合项三套编排语言的对账；
    「建议」抽共用产物者必须给设计或显式标注「尚未设计」。
    ✅ **已执行（2026-09-20，B2 / B3）**：§1.4 的三套对账表**已存在且自洽**（含末尾硬规则
    「同一条目的执行约束只以本表为准」）；§1.5④ **已撤回**「抽一个共用代码小工具」这一承诺
    并给出替代物（`AGENTS.md` 规范条目 + 评审检查项；若代码化则唯一合规形态是自由函数
    `fn rollback_on_fail(flag: &AtomicBool, ok: bool)`）。**抓到 2 处未同步的残留措辞**
    （§1.2 与 §4.7 仍写「抽出共用工具 / 复用回滚工具」）并就地订正。
  · 注入类三项的最终结论（2026-09-19，详见 tools/verify-batch-4.sh 的 MANUAL 段）：
    · P2-3 —— **已由等价判据覆盖**：`node tools/verify-l1l2.mjs` 的 H 组在**部分注入**
      （`event` 在、`core` 不在）下加载真实 popup.html；H0 是前置断言、H0b 是对照。
      可证伪性实跑：`--inject-broken=l1` ⇒ H2 转红。
    · P2-7 —— **已固化为常驻单测** `battery_notify::tests::cache_lock_is_released_before_emit`
      （两个探针 + 三段式断言）；对照判据实跑：guard 活到发送处 ⇒ `left: 0, right: 1` 转红。
      同时 `emit_notifications` 已降为私有，杜绝第二处持锁调用入口。**已移出 MANUAL**。
    · P3-11 —— **机制层已由等价判据覆盖**：`register_is_called_before_read` 断言真实调用序列，
      实跑交换两行 ⇒ 2 条用例转红。端到端「插 sleep + 真实切主题」的增量只剩
      「注册表通知链路可用」（已由 `real_state_wrapper_does_not_panic` 覆盖到「不 panic」），
      而真实切主题会闪烁用户桌面 ⇒ **不再执行，显式接受残余风险**（§8.3 第 14 条允许的第二种方式）。
    · P0-4 类锁序：见 `tools/verify-b14.mjs`。
EOF

echo
if [ "$fail" = "0" ]; then
  echo "收尾验收：通过（MANUAL 项需另行执行）"
else
  echo "收尾验收：**未通过**"
  exit 1
fi
