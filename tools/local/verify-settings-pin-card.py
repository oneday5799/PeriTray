# -*- coding: utf-8 -*-
"""验收：设置页「任务栏组件」折叠卡片（开关 / 展开 / 已添加清单 / 移除）。

⛔ 为什么必须单独验：`cargo test` 覆盖的是后端判据，CDP 直调 `update_config`
   覆盖的是**后端**；而本轮真正新增的是**前端接线**——`bindToggle` 绑没绑对键、
   `refreshList` 有没有渲染行、「移除」按钮点了会不会真删。若这三处漏了，
   后端与静态闸门全绿，用户打开设置页却什么都点不动。
   ⇒ 走**真实事件**（`input.click()` 派发原生 change），而非直接调 handler。

判据全部锚在「本批真正改过的机制」上：
   · 关闭后**设备不丢**（用户明确要求）→ 关时后端清单长度必须不变
   · 展开区含**已添加设备** + 每行「移除」
   · 「移除」点一下后端清单与 DOM 行数**同步 -1**
"""
import json
import os
import subprocess
import sys
import time

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
EXE = os.path.join(WD, "PeriTray.exe")
CFG = os.path.join(WD, "config.toml")
CFG_BAK = CFG + ".pinbak"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, ok, detail))
    print(f"  {'[OK]  ' if ok else '[FAIL]'} {name}" + (f"  —— {detail}" if detail else ""))
    sys.stdout.flush()


def cdp(expr, timeout=90):
    # ⚠️ 必须显式给 encoding="utf-8"：`text=True` 在本机会用 locale 的 GBK 解码，
    # 而 CDP 回显里含中文设备名（小爱音箱…）⇒ 直接 UnicodeDecodeError 崩在 subprocess 里。
    r = subprocess.run([NODE, CDP, "settings.html", expr],
                       capture_output=True, text=True, encoding="utf-8",
                       errors="replace", timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout).strip())
    return r.stdout.strip()


# 切到「任务栏」tab + 汇总卡片 DOM 状态。
DOM = ("(async () => {"
       "  const nav = document.querySelector('.win-nav-item[data-tab=\"taskbar\"]');"
       "  if (nav) nav.click();"
       "  await new Promise(r => setTimeout(r, 400));"
       "  const card = document.getElementById('taskbar-widget-card');"
       "  const tg = document.getElementById('toggle-taskbar-widget');"
       "  const items = document.getElementById('taskbar-widget-items');"
       "  const dev = document.getElementById('taskbar-widget-devices');"
       "  if (!card || !tg || !items || !dev) return JSON.stringify({found:false});"
       "  const rows = [...dev.querySelectorAll('.card-item')];"
       "  return JSON.stringify({"
       "    found: true,"
       "    cardVisible: card.offsetParent !== null,"
       "    checked: tg.checked,"
       "    expanded: items.classList.contains('show'),"
       "    maxH: items.style.maxHeight,"
       "    oldPickerGone: !document.getElementById('btn-taskbar-devices'),"
       "    helpTipGone: !document.querySelector('#taskbar-widget-card .help-dot, "
       "                            #taskbar-widget-card [data-help]'),"
       "    rowCount: rows.length,"
       "    names: rows.map(r => (r.querySelector('.card-item-name')||{}).textContent),"
       "    dimmed: rows.map(r => !!r.querySelector('.card-item-name.dimmed')),"
       "    btnLabels: rows.map(r => { const b = r.querySelector('button');"
       "                              return b ? b.textContent : null; }),"
       "  }); })()")

# 点开关 → 读后端 config + 清单长度。
TOGGLE = ("(async () => {"
          "  const inv = window.__TAURI__.core.invoke;"
          "  const tg = document.getElementById('toggle-taskbar-widget');"
          "  const before = await inv('get_config');"
          "  const beforeList = await inv('get_pinned_taskbar_list');"
          "  tg.click();"                       # 真实事件 → 派发 change → bindToggle + 联动展开
          "  await new Promise(r => setTimeout(r, 1500));"
          "  const after = await inv('get_config');"
          "  const afterList = await inv('get_pinned_taskbar_list');"
          "  const items = document.getElementById('taskbar-widget-items');"
          "  return JSON.stringify({"
          "    before: before.taskbar_widget_enabled,"
          "    after: after.taskbar_widget_enabled,"
          "    beforeN: beforeList.length, afterN: afterList.length,"
          "    domExpanded: items.classList.contains('show'),"
          "  }); })()")

# 点卡片标题 → 折叠/展开（守卫排除 .toggle/input，故点标题是安全路径）。
HEADER = ("(async () => {"
          "  const items = document.getElementById('taskbar-widget-items');"
          "  const before = items.classList.contains('show');"
          "  document.querySelector('#taskbar-widget-card .card-title').click();"
          "  await new Promise(r => setTimeout(r, 600));"
          "  return JSON.stringify({ before: before,"
          "    after: items.classList.contains('show'),"
          "    maxH: items.style.maxHeight }); })()")

# 点第一行「移除」→ 后端清单与 DOM 行数同步 -1。
REMOVE = ("(async () => {"
          "  const inv = window.__TAURI__.core.invoke;"
          "  const dev = document.getElementById('taskbar-widget-devices');"
          "  const beforeList = await inv('get_pinned_taskbar_list');"
          "  const beforeNames = beforeList.map(x => x.name);"
          "  const btn = dev.querySelector('.card-item button');"
          "  const target = (btn.closest('.card-item').querySelector('.card-item-name')||{}).textContent;"
          "  btn.click();"
          "  await new Promise(r => setTimeout(r, 2500));"
          "  const afterList = await inv('get_pinned_taskbar_list');"
          "  return JSON.stringify({ beforeN: beforeList.length, afterN: afterList.length,"
          "    beforeNames: beforeNames, target: target,"
          "    domRows: dev.querySelectorAll('.card-item').length,"
          "    remaining: afterList.map(x => x.name),"
          "    targetGone: !afterList.some(x => x.name === target) }); })()")


def main():
    if not os.path.exists(CFG):
        print("!! 找不到 config.toml")
        return 1
    open(CFG_BAK, "w", encoding="utf-8").write(open(CFG, encoding="utf-8").read())
    try:
        subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
        time.sleep(1)
        env = dict(os.environ)
        env["PM_DEV_OPEN_SETTINGS"] = "1"
        env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
            "--disable-gpu-sandbox --remote-debugging-port=9222"
        subprocess.Popen([EXE], cwd=WD, env=env)
        time.sleep(9)

        # ── 1. 卡片存在 + 旧选择器已退役 ──
        d = json.loads(cdp(DOM))
        print("  DOM:", json.dumps(d, ensure_ascii=False))
        check("新卡片存在于「任务栏」tab 且可见", d.get("found") is True and d.get("cardVisible") is True)
        check("旧的「选择设备」按钮已退役", d.get("oldPickerGone") is True)
        check("卡片旁的 ? 说明已移除", d.get("helpTipGone") is True)
        check("开关初值来自 config（当前 true ⇒ 勾选）", d.get("checked") is True)
        check("初始展开态跟随开关（开 ⇒ 展开）", d.get("expanded") is True, f"maxH={d.get('maxH')}")

        n0 = d.get("rowCount", -1)
        check("展开区渲染出已添加设备行", isinstance(n0, int) and n0 >= 1, f"{n0} 行：{d.get('names')}")
        check("每行都有「移除」按钮",
              bool(n0) and all(x == "移除" for x in d.get("btnLabels", [])),
              f"{d.get('btnLabels')}")

        # ── 2. 关闭后设备必须保留（用户明确要求；本批核心语义）──
        t1 = json.loads(cdp(TOGGLE))
        print("  toggle(off):", json.dumps(t1, ensure_ascii=False))
        check("点开关后落盘为 false", t1.get("after") is False, f"{t1.get('before')} → {t1.get('after')}")
        check("⛔ 关闭后已添加设备**未被清空**",
              t1.get("afterN") == t1.get("beforeN") and t1.get("beforeN", 0) >= 1,
              f"{t1.get('beforeN')} → {t1.get('afterN')}")
        check("关闭时展开区收起（联动）", t1.get("domExpanded") is False)

        # ── 3. 重新开启 ──
        t2 = json.loads(cdp(TOGGLE))
        print("  toggle(on):", json.dumps(t2, ensure_ascii=False))
        check("再点开关后落盘回 true", t2.get("after") is True, f"{t2.get('before')} → {t2.get('after')}")
        check("重新开启后设备仍在（未误删）",
              t2.get("afterN") == t1.get("afterN"), f"清单 {t2.get('afterN')} 条")

        # ── 4. 折叠卡头部点击 ──
        h = json.loads(cdp(HEADER))
        print("  header:", json.dumps(h, ensure_ascii=False))
        check("点卡片标题可折叠/展开", h.get("before") != h.get("after"),
              f"{h.get('before')} → {h.get('after')}，maxH={h.get('maxH')}")
        cdp(HEADER)  # 复位

        # ── 5. 「移除」按钮端到端 ──
        r = json.loads(cdp(REMOVE))
        print("  remove:", json.dumps(r, ensure_ascii=False))
        check("点「移除」后端清单 -1", r.get("afterN") == r.get("beforeN", 0) - 1,
              f"{r.get('beforeN')} → {r.get('afterN')}，移除的是 {r.get('target')!r}")
        check("移除的是点中的那一台", r.get("targetGone") is True)
        check("DOM 行数与后端同步", r.get("domRows") == r.get("afterN"),
              f"DOM {r.get('domRows')} 行 vs 后端 {r.get('afterN')} 条")

        ok = sum(1 for _, o, _ in RESULTS if o)
        print(f"\n=== {ok}/{len(RESULTS)} 通过 ===")
        return 0 if ok == len(RESULTS) else 2
    finally:
        subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
        time.sleep(1)
        if os.path.exists(CFG_BAK):
            open(CFG, "w", encoding="utf-8").write(open(CFG_BAK, encoding="utf-8").read())
            os.remove(CFG_BAK)
            print("  (config.toml 已还原)")


if __name__ == "__main__":
    sys.exit(main())
