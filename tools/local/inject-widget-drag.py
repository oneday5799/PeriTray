"""可证伪性注入：把「拖拽位置判据」故意写反，确认对应用例**确实转红**。

为什么必须做这一步（而不是「跑一遍绿了就算数」）：
  这些判据的失效方式**全是静默的** —— 钳制丢掉一端、优先级写反、圆角挖错，
  界面上不报错、不崩溃，只表现为「窗口跑到别处」「底衬是个方块」。
  而「用例通过」有两种可能：① 实现正确；② **用例对这条判据没有区分力**。
  只有注入后转红，才能把 ② 排除掉。

每条注入 = (说明, 原文, 替换, 期望转红的用例名)。
全部跑完后**无条件还原**原文件（哪怕中途异常），并在最后打印校验和比对。
"""
import hashlib
import subprocess
import sys
from pathlib import Path

SRC = Path(r"D:\Code\PeriTray\src-tauri\src\taskbar_widget.rs")
CARGO = r"C:\Users\Oneday\.cargo\bin\cargo.exe"

INJECTIONS = [
    (
        "clamp_rel_x 丢掉右端钳制（只 max(0)，不 clamp 上界）",
        "    let max = (taskbar_w - widget_w).max(0);\n    x.clamp(0, max)",
        "    let max = (taskbar_w - widget_w).max(0);\n    let _ = max;\n    x.max(0)",
        "clamp_rel_x_pins_both_ends",
    ),
    (
        "resolve_rel_x 的 locked 分支被削弱（有 custom_x 时不再走贴靠）",
        "    let wanted = if locked {\n        aligned\n    } else {",
        "    let wanted = if locked && custom_x.is_none() {\n        aligned\n    } else {",
        "resolve_rel_x_priority_is_locked_then_custom_then_last_then_aligned",
    ),
    (
        "resolve_rel_x 的「还没画过」哨兵判据写松（`!= 0` 写成 `>= 0`）",
        "            None if last != 0 => last,",
        "            None if last >= 0 => last,",
        "resolve_rel_x_priority_is_locked_then_custom_then_last_then_aligned",
    ),
    (
        "resolve_rel_x 末尾不再钳制持久化的拖拽落点",
        "    clamp_rel_x(wanted, widget_w, taskbar_w)\n}",
        "    wanted\n}",
        "resolve_rel_x_clamps_the_persisted_drag_position",
    ),
    (
        "inside_rounded_rect 恒 true（底衬退化为直角方块）",
        "    if x < 0 || y < 0 || x >= w || y >= h {\n        return false;\n    }\n    let r = r.min(w / 2).min(h / 2).max(0);",
        "    if x < 0 || y < 0 || x >= w || y >= h {\n        return false;\n    }\n    return true;\n    #[allow(unreachable_code)]\n    let r = r.min(w / 2).min(h / 2).max(0);",
        "inside_rounded_rect_excludes_corners",
    ),
]


def digest(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()[:16]


def run_test(name: str) -> bool:
    """跑单个用例，返回是否**通过**。"""
    p = subprocess.run(
        [CARGO, "test", "--quiet", name],
        cwd=str(SRC.parent.parent),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    out = (p.stdout or "") + (p.stderr or "")
    # `--quiet` 下：通过打印 "test result: ok."，失败打印 "test result: FAILED."
    return "test result: ok." in out


def main() -> int:
    original = SRC.read_text(encoding="utf-8")
    base = digest(original)
    print(f"原始文件 sha256[:16] = {base}  (共 {len(original)} 字符)\n")

    failures = []
    try:
        for i, (desc, old, new, test_name) in enumerate(INJECTIONS, 1):
            if old not in original:
                print(f"[{i}] ⛔ 注入锚点未命中，跳过：{desc}")
                failures.append((i, desc, "锚点未命中"))
                continue
            if original.count(old) != 1:
                print(f"[{i}] ⛔ 锚点不唯一（{original.count(old)} 处），跳过：{desc}")
                failures.append((i, desc, "锚点不唯一"))
                continue

            SRC.write_text(original.replace(old, new, 1), encoding="utf-8")
            passed = run_test(test_name)
            SRC.write_text(original, encoding="utf-8")

            if passed:
                print(f"[{i}] ❌ 注入后**仍然通过** —— 用例没有区分力：{desc}")
                print(f"      期望转红用例：{test_name}")
                failures.append((i, desc, "未转红"))
            else:
                print(f"[{i}] ✅ 转红（{test_name}）：{desc}")
    finally:
        # 无条件还原：注入脚本中途崩了也必须把源文件放回去
        SRC.write_text(original, encoding="utf-8")

    now = digest(SRC.read_text(encoding="utf-8"))
    print(f"\n还原后 sha256[:16] = {now}")
    if now != base:
        print("⛔⛔ 还原校验失败！源文件与注入前**不一致**，请手工检查。")
        return 2
    print("✅ 还原校验通过（与注入前逐字节一致）")

    if failures:
        print(f"\n❌ 有 {len(failures)} 条注入未达到预期：")
        for i, desc, why in failures:
            print(f"   [{i}] {why} —— {desc}")
        return 1
    print(f"\n✅ 全部 {len(INJECTIONS)} 条注入均按预期转红 ⇒ 用例有区分力")
    return 0


if __name__ == "__main__":
    sys.exit(main())
