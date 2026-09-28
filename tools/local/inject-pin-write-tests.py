# -*- coding: utf-8 -*-
"""可证伪注入：证明本次新增的 8 条 pin 写入路径单测不是「永远绿」。

每个注入只改一处，跑 `cargo test --no-default-features pinned` 记录哪些用例转红，
最后**无条件**还原并校验 sha256（二进制读写，不做换行转换）。

判据是「注入必须只让该红的红」：
  · 该红的没红 ⇒ 用例是假绿（上一批就踩过：两条路功能重叠，只断言共同结果）；
  · 不该红的红了 ⇒ 用例断言过宽，会被无关改动打红。
"""
import hashlib
import re
import shutil
import subprocess

SRC = r"D:\Code\PeriTray\src-tauri\src\commands.rs"
BAK = r"D:\Code\PeriTray\tools\local\_out\_cmd_backup.rs"
CARGO = r"C:\Users\Oneday\.cargo\bin\cargo.exe"
CWD = r"D:\Code\PeriTray\src-tauri"


def digest(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


orig_bytes = open(SRC, "rb").read()
orig_text = orig_bytes.decode("utf-8")
orig_hash = digest(SRC)
shutil.copy2(SRC, BAK)

# ⚠️ 本仓的 .rs 是 **CRLF** 行尾（rustfmt 的 newline_style = Auto 会原样保留）。
# 多行锚点若按 LF 写，count() 恒为 0 —— 表现为「锚点命中 0 次 ⇒ 跳过」，
# 看着像脚本问题、实则是「整个注入根本没生效」，是最容易蒙混过关的假验收形态。
NL = "\r\n" if "\r\n" in orig_text else "\n"


def nl(text):
    """把锚点里的 LF 统一成文件实际的换行符。"""
    return text.replace("\n", NL)

RETAIN_BLOCK = """    let before = c.pinned_taskbar_devices.len();
    c.pinned_taskbar_devices
        .retain(|p| !config::pinned_device_matches(p, key, fallback));
    if c.pinned_taskbar_devices.len() < before {
        return Ok(());
    }"""

INJECTIONS = [
    (
        "1",
        "切换判据与显示侧分叉（只比 key，不看 fallback）",
        "        .retain(|p| !config::pinned_device_matches(p, key, fallback));",
        "        .retain(|p| p.key != key);",
        ["pinned_toggle_is_the_inverse_of_the_display_predicate",
         "pinned_toggle_removes_every_matching_entry",
         "pinned_toggle_flips_shared_fallback_entry"],
    ),
    (
        "2",
        "取消时只删第一条（自然写法：position + remove + return）",
        RETAIN_BLOCK,
        """    if let Some(pos) = c
        .pinned_taskbar_devices
        .iter()
        .position(|p| config::pinned_device_matches(p, key, fallback))
    {
        c.pinned_taskbar_devices.remove(pos);
        return Ok(());
    }""",
        ["pinned_toggle_removes_every_matching_entry",
         "pinned_toggle_is_the_inverse_of_the_display_predicate"],
    ),
    (
        "3",
        "上限守卫失效",
        "if c.pinned_taskbar_devices.len() >= PINNED_TASKBAR_LIMIT {",
        "if c.pinned_taskbar_devices.len() >= usize::MAX {",
        ["pinned_limit_rejects_without_writing"],
    ),
    (
        "4",
        "空键守卫失效（幽灵条目得以写入）",
        "if key.trim().is_empty() {",
        "if false {",
        ["pinned_toggle_normalizes_blank_fields_and_rejects_blank_key"],
    ),
    (
        "5",
        "归一化失效（存下 Some(\"\")）",
        """    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)""",
        "    s.map(str::to_string)",
        ["pinned_toggle_normalizes_blank_fields_and_rejects_blank_key"],
    ),
    (
        "6",
        "fallback 丢失（换机/重装驱动后 pin 静默失联）",
        "        fallback: normalized_opt(fallback),",
        "        fallback: None,",
        # 额外打红 `flips_shared_fallback_entry` 是**正确**的：该用例正是靠 fallback
        # 命中才走取消分支，fallback 一丢它必然改走新增 ⇒ 条目数不减反增。
        ["pinned_toggle_adds_then_removes", "pinned_toggle_flips_shared_fallback_entry"],
    ),
    (
        "7",
        "alias 丢失（占位条目显示不出用户自定义名）",
        "        alias: normalized_opt(alias),",
        "        alias: None,",
        # 额外打红归一化用例同理：它断言的就是 `alias: Some("耳机")`。
        ["pinned_toggle_adds_then_removes",
         "pinned_toggle_normalizes_blank_fields_and_rejects_blank_key"],
    ),
]


def run_tests():
    r = subprocess.run(
        [CARGO, "test", "--no-default-features", "pinned"],
        cwd=CWD,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    out = (r.stdout or "") + (r.stderr or "")
    if "error[" in out or "error: could not compile" in out:
        return None, "编译失败 ❌"
    failed = re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)
    m = re.search(r"test result: .*", out)
    return [f.split("::")[-1] for f in failed], (
        m.group(0).strip() if m else "（无 test result 行）"
    )


def expected_verdict(tag, desc, expected, short, result_line):
    exp = set(expected)
    got = set(short)
    if exp == got:
        print("    判定：✅ 该红的都红了，且未波及无关用例")
    elif not got:
        print("    判定：❌ 该红的**没红** ⇒ 对应用例是假绿")
    else:
        print(f"    判定：⚠️ 该红的没全红 / 多红了：期望 {sorted(exp)}，实得 {sorted(got)}")


try:
    for tag, desc, old, new, expected in INJECTIONS:
        old, new = nl(old), nl(new)
        hits = orig_text.count(old)
        print(f"### 注入 {tag}：{desc}")
        if hits != 1:
            print(f"    锚点命中 {hits} 次（应为 1）⇒ 跳过，无法判定\n")
            continue
        open(SRC, "wb").write(orig_text.replace(old, new, 1).encode("utf-8"))
        short, result_line = run_tests()
        print(f"    {result_line}")
        if short is None:
            print("    判定：❌ 注入未编译通过，本次注入无效\n")
            continue
        print(f"    转红 {len(short)} 条：" + (", ".join(sorted(short)) if short else "（无）"))
        expected_verdict(tag, desc, expected, short, result_line)
        print()
finally:
    open(SRC, "wb").write(orig_bytes)
    same = digest(SRC) == orig_hash
    print(f"### 还原校验：sha256 {'一致 ✅' if same else '不一致 ❌'}")
    if not same:
        shutil.copy2(BAK, SRC)
        print("    已从备份强制还原")
