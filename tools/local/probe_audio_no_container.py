"""只读探针 v4：用注册表反向索引独立验证「哪些音频端点无可用容器，为什么」。

背景（设计稿 §12.2 实测）：
  当前在场的 `SWD\\MMDEVAPI` devnode 共 6 个，`CM_Locate_DevNodeW` **6/6 成功**，
  但只有 2 个能拿到可用容器。需要判明另外 4 个是：
    (a) 落在**占位容器**上（⇒ 被 `usable_container` 正确过滤），还是
    (b) 压根**没有容器属性**。

方法：`Control\\DeviceContainers\\<guid>\\BaseContainers\\<guid>` 下的**子键名**
就是该容器的成员 devnode 实例路径（值都是 REG_NONE 占位，**键名才是数据**）。
⇒ 这是一张「容器 → 成员」的反向索引表，可独立于 CFGMGR2 交叉验证。

顺带回答：注册表 `Enum\\SWD\\MMDEVAPI` 里的**全部**端点（含不在场的）有多少 ——
设计稿 §12.2 原话是「全部 30 个端点键」，需核实这个数字的来源。

全部只读。
"""
import sys
from collections import defaultdict

import winreg

sys.stdout.reconfigure(encoding="utf-8")

DC = r"SYSTEM\CurrentControlSet\Control\DeviceContainers"
ENUM_MM = r"SYSTEM\CurrentControlSet\Enum\SWD\MMDEVAPI"
NULL_CONTAINERS = {
    "00000000-0000-0000-0000-000000000000",
    "00000000-0000-0000-ffff-ffffffffffff",
}


def subkeys(path):
    out = []
    try:
        k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, path)
    except OSError:
        return out
    i = 0
    while True:
        try:
            out.append(winreg.EnumKey(k, i))
        except OSError:
            break
        i += 1
    return out


def values(path):
    out = {}
    try:
        k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, path)
    except OSError:
        return out
    i = 0
    while True:
        try:
            name, data, ty = winreg.EnumValue(k, i)
        except OSError:
            break
        out[name] = (ty, data)
        i += 1
    return out


def as_str(raw):
    if isinstance(raw, (bytes, bytearray)):
        return raw.decode("utf-16-le", "replace").rstrip("\x00")
    return str(raw)


def build_reverse_index():
    """容器 → 成员实例路径列表。"""
    idx = defaultdict(list)
    for c in subkeys(DC):
        guid = c.strip("{}").lower()
        # ⚠️ BaseContainers 下的子键名是**小写**花括号 GUID，与顶层大写键名不同 ——
        # 直接拿顶层键名去拼路径会**静默返回空表**（本探针第一版就踩了）。
        child = f"{DC}\\{c}\\BaseContainers\\" + "{" + guid + "}"
        # ⛔ 成员实例路径是**值名**（数据一律 REG_NONE），**不是子键** ——
        # 用 subkeys() 会得到空表且不报错（本探针第二版就栽在这）。
        for member in values(child):
            idx[member.upper()].append(guid)
    return idx


def main():
    print("=" * 78)
    print("§12.2 音频端点无容器的原因 —— 反向索引交叉验证")
    print("=" * 78)

    rev = build_reverse_index()
    print(f"\n[1] 反向索引规模: {len(rev)} 个成员实例路径（来自 {len(subkeys(DC))} 个容器）")

    # ── 2. 注册表里 SWD\MMDEVAPI 的全部端点 ────────────────
    all_endpoints = subkeys(ENUM_MM)
    print(f"\n[2] 注册表 Enum\\SWD\\MMDEVAPI 顶层键数: {len(all_endpoints)}")
    for e in all_endpoints:
        p = f"{ENUM_MM}\\{e}"
        v = values(p)
        cfg = v.get("ConfigFlags")
        cfg = cfg[1] if cfg else 0
        cid = None
        if "ContainerID" in v:
            cid = as_str(v["ContainerID"][1]).strip("{}").lower()
        kind = "⛔占位" if cid in NULL_CONTAINERS else ("✅真实" if cid else "无属性")
        print(f"      {kind}  {cid}   <- SWD\\MMDEVAPI\\{e}")
    print("    ⚠️ 注意：这里**没有**靠 ConfigFlags 判在场 —— 对 SWD\\MMDEVAPI 它恒为 0x0，"
          "判不出来。在场与否必须靠运行时 CM_Get_Device_ID_ListW。")

    # ── 3. 与 Rust 侧实测的 6 个逐一对照 ────────────────────
    print("\n[3] Rust 侧实测的 6 个在场实例 → 反向索引里的容器")
    observed = [
        r"SWD\MMDEVAPI\{0.0.1.00000000}.{7333ea13-d2cf-4f1c-a75c-979c510107d3}",
        r"SWD\MMDEVAPI\{0.0.0.00000000}.{c5c40f0a-a935-48a4-bc19-06ff1d09e504}",
        r"SWD\MMDEVAPI\{0.0.0.00000000}.{91ac81af-b442-4a90-92d4-ffff1c01190e}",
        r"SWD\MMDEVAPI\MicrosoftGSWavetableSynth",
        r"SWD\MMDEVAPI\{0.0.0.00000000}.{7e42f2f4-6a35-4f5f-9de8-f8c57b82e68a}",
        r"SWD\MMDEVAPI\{0.0.0.00000000}.{c5fc3377-e930-4fd1-9cc1-8ee6b0486f1a}",
    ]
    for inst in observed:
        cids = rev.get(inst.upper(), [])
        if not cids:
            print(f"      {inst}")
            print("          反向索引里**查不到** ⇒ 未归属任何容器")
            continue
        for c in cids:
            kind = "⛔ 占位容器（应被过滤）" if c in NULL_CONTAINERS else "✅ 真实容器"
            print(f"      {inst}")
            print(f"          → {c}  {kind}")

    # ── 4. 占位容器的成员规模（复核「107 个互不相关节点」）──
    print("\n[4] 占位容器的成员规模")
    for c in subkeys(DC):
        guid = c.strip("{}").lower()
        if guid in NULL_CONTAINERS:
            members = list(values(f"{DC}\\{c}\\BaseContainers\\{c}"))
            print(f"      {guid}: {len(members)} 个成员")
            pref = defaultdict(int)
            for m in members:
                pref[m.split("\\")[0]] += 1
            print(f"      按枚举器前缀: {dict(sorted(pref.items(), key=lambda kv: -kv[1]))}")
            mm = [m for m in members if m.upper().startswith("SWD\\MMDEVAPI")]
            print(f"      其中 SWD\\MMDEVAPI 成员: {mm}")


if __name__ == "__main__":
    main()
