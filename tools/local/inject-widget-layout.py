"""可证伪性注入：把 taskbar_widget.rs 的判据故意写反，确认对应用例转红。

每条注入 = (说明, 原文, 替换, 期望转红的用例名)。
全部注入跑完后**无条件还原**原文件（哪怕中途异常），并在最后打印最终校验和。
"""
import subprocess
import sys
import hashlib
from pathlib import Path

SRC = Path(r"D:\Code\PeriTray\src-tauri\src\taskbar_widget.rs")
CARGO = r"C:\Users\Oneday\.cargo\bin\cargo.exe"

INJECTIONS = [
    (
        "format_volume 无音频端点返回空串（而不是 N/A）",
        '        // 键鼠这类无音频端点的设备：用户明确要求显示 N/A（而不是留空）\n        return "N/A".to_string();',
        '        return String::new();',
        "missing_volume_always_shows_na",
    ),
    (
        "format_battery 读不出时返回 0%（而不是 N/A）",
        '        None => "N/A".to_string(),\n    }\n}\n\n/// 音量文本（画在图标**右下角**）',
        '        None => "0%".to_string(),\n    }\n}\n\n/// 音量文本（画在图标**右下角**）',
        "zero_battery_shows_percent_but_unreadable_shows_na",
    ),
    (
        "estimate_text_px 把中文按 ASCII 的 8px 估（低估）",
        '.map(|c| if (c as u32) < 0x80 { 8 } else { 14 })',
        '.map(|c| if (c as u32) < 0x80 { 8 } else { 8 })',
        "cjk_is_not_underestimated",
    ),
    (
        "estimate_widget_width 不取两段的最大值（只算电量段）",
        '            let bat = estimate_text_px(&format_battery(it));\n            let vol = estimate_text_px(&format_volume(it));\n            bat.max(vol)',
        '            let bat = estimate_text_px(&format_battery(it));\n            let vol = estimate_text_px(&format_volume(it));\n            bat.min(vol)',
        "per_item_width_uses_the_wider_of_the_two_texts",
    ),
    (
        "format_volume 去掉「静音优先」分支（静音设备显示百分比）",
        '    if it.is_muted == Some(true) {\n        return "静音".to_string();\n    }',
        '    if false {\n        return "静音".to_string();\n    }',
        "muted_device_shows_muted_instead_of_percent",
    ),
]


def digest(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()[:16]


def run_test(name: str) -> str:
    p = subprocess.run(
        [CARGO, "test", "--quiet", name],
        cwd=str(SRC.parent.parent),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    return (p.stdout or "") + (p.stderr or "")


def main() -> int:
    original = SRC.read_text(encoding="utf-8")
    print(f"[原文件] sha256[:16] = {digest(original)}  ({len(original)} 字符)\n")
    failures = []
    try:
        for i, (desc, old, new, test_name) in enumerate(INJECTIONS, 1):
            if original.count(old) != 1:
                print(f"❌ 注入 {i} 的锚点不唯一（命中 {original.count(old)} 次）: {desc}")
                failures.append(desc)
                continue
            SRC.write_text(original.replace(old, new), encoding="utf-8")
            out = run_test(test_name)
            # cargo test 失败时返回码非 0 且输出含 "FAILED"
            turned_red = "FAILED" in out or "test result: FAILED" in out
            marker = "✅ 转红" if turned_red else "❌ 仍绿（用例失去区分力！）"
            print(f"{marker}  注入 {i}: {desc}\n          期望转红用例 = {test_name}")
            if not turned_red:
                failures.append(desc)
                print("          ---- 输出尾部 ----")
                print("\n".join(out.strip().splitlines()[-8:]))
            print()
    finally:
        SRC.write_text(original, encoding="utf-8")
        restored = SRC.read_text(encoding="utf-8")
        ok = digest(restored) == digest(original)
        print(f"[还原] sha256[:16] = {digest(restored)}  一致={ok}")

    if failures:
        print(f"\n⚠️ {len(failures)} 条注入未能使用例转红")
        return 1
    print(f"\n✅ 全部 {len(INJECTIONS)} 条注入都使对应用例转红（用例有区分力）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
