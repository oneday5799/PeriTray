# -*- coding: utf-8 -*-
"""验收（前端接线）：设置页「任务栏」标签页里新增的下拉**真的存在且能落盘**。

⛔ 为什么单独验这一层：`verify-content-scale.py` 走的是 CDP 直接调 `update_config`
   ⇒ 它证明的是**后端**；若 HTML 里 `data-value` 拼错、或 `initTaskbarTab()` 忘了调
   `initTaskbarContentScale()`，后端照样全绿，而**用户在界面上根本选不到那一档**。
   （`tools/check.mjs` 的「前端完整性检查」是静态的，覆盖不到这一层。）
"""
import os
import subprocess
import sys
import time

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
EXE = os.path.join(WD, "PeriTray.exe")
CFG = os.path.join(WD, "config.toml")
CFG_BAK = os.path.join(WD, "config.toml.uibak")
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, ok, detail))
    print(f"  {'[OK]  ' if ok else '[FAIL]'} {name}" + (f"  —— {detail}" if detail else ""))
    sys.stdout.flush()


def cdp(expr, timeout=90):
    r = subprocess.run([NODE, CDP, "settings.html", expr],
                       capture_output=True, text=True, timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout).strip())
    return r.stdout.strip()


DOM = ("(async () => {"
       "  const el = document.getElementById('combo-taskbar-content-scale');"
       "  if (!el) return JSON.stringify({ found: false });"
       "  const nav = document.querySelector('.win-nav-item[data-tab=\"taskbar\"]');"
       "  if (nav) nav.click();"
       "  await new Promise(r => setTimeout(r, 300));"
       "  const card = el.closest('.card');"
       "  return JSON.stringify({"
       "    found: true,"
       "    values: [...el.querySelectorAll('.win-combo-item')].map(i => i.dataset.value),"
       "    labels: [...el.querySelectorAll('.win-combo-item-content')].map(i => i.textContent),"
       "    current: el.querySelector('.win-combo-content').textContent,"
       "    cardTitle: (card ? card.querySelector('.card-title') : null)"
       "      ? card.querySelector('.card-title').innerText.replace(/\\s+/g, ' ').slice(0, 40) : '',"
       "    cardVisible: card ? card.offsetParent !== null : false,"
       "  }); })()")


def pick(value):
    """模拟用户在真实下拉里点选某一档，然后读回后端配置。

    ⛔⛔ **flyout 引用必须「第一次就从 `el` 取好」并缓存在 `window` 上**：
       `initComboBox` 打开 flyout 时会把该节点**移到 `document.body`**，而 `closeFlyout`
       **不移回来** ⇒ 一旦打开过，之后 `el.querySelectorAll` 永远查不到
       （返回空 ⇒ `NO_ITEM`，实测第一次成功后第二次必失败）。
       ⛔ 也不能退化成 `document.querySelector('.win-combo-item[data-value=…]')` ——
       页面里 `default` 这个值在「窗口材质 / 弹窗尺寸 / 主题模式」里**都有**，
       全局查会点到**别的控件**上（假绿）。这是 `initComboBox` 的既有契约。
    """
    return cdp("(async () => {"
               "  const el = document.getElementById('combo-taskbar-content-scale');"
               "  const inv = window.__TAURI__.core.invoke;"
               "  if (!window.__scaleFlyout)"
               "    window.__scaleFlyout = el.querySelector('.win-combo-flyout');"
               "  const items = [...window.__scaleFlyout.querySelectorAll('.win-combo-item')];"
               "  const before = await inv('get_config');"
               "  el.querySelector('.win-combo-btn').click();"
               "  await new Promise(r => setTimeout(r, 350));"
               f"  const item = items.find(i => i.dataset.value === '{value}');"
               "  if (!item) return JSON.stringify({ err: 'NO_ITEM' });"
               "  item.click();"
               "  await new Promise(r => setTimeout(r, 1200));"
               "  const after = await inv('get_config');"
               "  return JSON.stringify({"
               "    before: before.taskbar_content_scale,"
               "    after: after.taskbar_content_scale,"
               "    label: el.querySelector('.win-combo-content').textContent,"
               "  }); })()")


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

        import json
        dom = json.loads(cdp(DOM))
        print("  DOM:", dom)
        check("控件存在于设置页 DOM", dom.get("found") is True)
        if not dom.get("found"):
            return 2
        check("选项与后端字面逐字一致",
              dom["values"] == ["default", "follow_system"],
              f"{dom['values']}")
        check("选项文案符合需求（默认大小 / 跟随系统）",
              dom["labels"] == ["默认大小", "跟随系统"], f"{dom['labels']}")
        check("控件在「任务栏」标签页内且可见", dom.get("cardVisible") is True,
              f"card = {dom.get('cardTitle')!r}")

        r1 = json.loads(pick("follow_system"))
        print("  pick(follow_system):", r1)
        check("点选「跟随系统缩放」后落盘生效",
              r1.get("after") == "follow_system",
              f"{r1.get('before')} → {r1.get('after')}，按钮文案 = {r1.get('label')!r}")

        r2 = json.loads(pick("default"))
        print("  pick(default):", r2)
        check("点选「默认缩放大小」后落盘生效",
              r2.get("after") == "default",
              f"{r1.get('after')} → {r2.get('after')}，按钮文案 = {r2.get('label')!r}")

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
