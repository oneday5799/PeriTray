"""只读探针：回答「同一台物理设备的不同 HID 集合，会不会被当成不同设备？」

背景：批次 A 把 2.4G 电量缓存键由 `(VID, PID)`（型号级）改为**设备身份键**
（容器优先 `c:<容器>`，降级 `i:<实例路径>` / `n:<名称>`，最后 `m:<VID>:<PID>`）。

`device_data::is_wireless_24g` 只看 **(VID, PID)** ⇒ 同一个 2.4G 接收器的
**每个** HID 集合（`&Col01` / `&Col02` / `&MI_0x` …）都会被判为 2.4G、
都会各推一个查询目标。于是必须回答：

    这些「同一台设备」的目标，拿到的是**同一个身份键**（⇒ 去重成一条）
    还是**不同身份键**（⇒ 一台设备被拆成多台、各查各的）？

判据（可证伪）：
  A. 容器优先路径：同一物理设备的全部 HID 集合应落在**同一容器**
     ⇒ 身份键相同 ⇒ `snapshot` 去重成一条。若跨多个容器 ⇒ **身份键被拆开**（缺陷）。
  B. 降级路径：`i:<实例路径>` 是**逐 HID 集合**的 ⇒ 一旦容器解析失败，
     同一台设备必然被拆成多份（降级方向的固有代价，需如实标注）。

「同一物理设备」的判据 = HID 实例路径的**实例段**（末段 `8&xxxxxxx&0&0000`），
它在同一台设备的各集合间是**共享**的（复合接口的 `&MI_0x` 会不同，另计）。

全部只读。
"""
import sys
from collections import defaultdict

import winreg

sys.stdout.reconfigure(encoding="utf-8")

DC = r"SYSTEM\CurrentControlSet\Control\DeviceContainers"
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


def value_names(path):
    out = []
    try:
        k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, path)
    except OSError:
        return out
    i = 0
    while True:
        try:
            n, _d, _t = winreg.EnumValue(k, i)
        except OSError:
            break
        out.append(n)
        i += 1
    return out


def build_index():
    """容器 → 成员实例路径（成员是**值名**，不是子键；子键名是**小写**花括号 GUID）。"""
    idx = {}
    for c in subkeys(DC):
        guid = c.strip("{}").lower()
        members = value_names(f"{DC}\\{c}\\BaseContainers\\" + "{" + guid + "}")
        idx[guid] = members
    return idx


def hid_instance_of(path):
    """HID 实例路径 → (deviceID, instanceID)。

    `HID\\VID_25A7&PID_FA70&MI_00&COL01\\8&33C1AD1D&0&0000`
      → deviceID   = `VID_25A7&PID_FA70&MI_00&COL01`
        instanceID = `8&33C1AD1D&0&0000`
    """
    parts = path.split("\\")
    if len(parts) < 3:
        return None, None
    return parts[1], parts[2]


def hid_phys_of(path):
    """HID 实例路径 → **物理设备前缀**。

    本机实测形态：同一台设备的多个 HID 集合是
        HID\\VID_1532&PID_0094\\8&b16f3a&0&0000 … &0&0007
    即集合序号在**末段**的 `&0&NNNN`，而 `8&b16f3a` 是设备实例哈希
    ⇒ 物理设备前缀 = 末段去掉最后两个 `&` 分量。
    """
    _dev, inst = hid_instance_of(path)
    if not inst:
        return None
    seg = inst.split("&")
    if len(seg) <= 2:
        return inst
    return "&".join(seg[:-2])


def main():
    idx = build_index()
    print("=" * 78)
    print("同一台物理设备的多个 HID 集合，是否共享同一个容器？")
    print("=" * 78)

    # ── 1. 收集：物理设备前缀 → 容器集合 / 成员集合 / 型号 ──────
    inst_containers = defaultdict(set)   # 物理设备前缀 → {容器}
    inst_members = defaultdict(set)      # 物理设备前缀 → {HID 实例路径}
    inst_models = defaultdict(set)       # 物理设备前缀 → {VID:PID}
    inst_null = defaultdict(set)         # 物理设备前缀 → {占位容器}
    total_hid = 0

    for guid, members in idx.items():
        for m in members:
            if not m.upper().startswith("HID\\"):
                continue
            total_hid += 1
            phys = hid_phys_of(m)
            if not phys:
                continue
            inst_members[phys].add(m)
            if guid in NULL_CONTAINERS:
                inst_null[phys].add(guid)
            else:
                inst_containers[phys].add(guid)
            dev = m.split("\\")[1] if len(m.split("\\")) > 1 else ""
            if "VID_" in dev.upper() and "PID_" in dev.upper():
                up = dev.upper()
                vid = up.split("VID_")[1][:4]
                pid = up.split("PID_")[1][:4]
                inst_models[phys].add(f"{vid}:{pid}")

    print(f"\n[0] 总览：HID 成员 {total_hid} 条 → 物理设备 {len(inst_members)} 台")
    multi = {i: ms for i, ms in inst_members.items() if len(ms) > 1}
    print(f"    其中「多 HID 集合」设备 {len(multi)} 台（其余 {len(inst_members) - len(multi)} 台只有 1 个集合）")

    # ── 2. 核心判据 A：同一物理设备的集合是否跨多个「真实」容器 ────
    print(f"\n[A] 核心判据：同一物理设备的多个 HID 集合是否落在**同一个真实容器**")
    multi_ok = []     # 集合数 >1 且容器唯一（最有力的正例）
    multi_bad = []    # 集合数 >1 但跨多个容器（缺陷）
    for inst, members in inst_members.items():
        cs = inst_containers[inst]
        if len(members) == 1:
            continue
        if len(cs) <= 1:
            multi_ok.append((inst, members, cs))
        else:
            multi_bad.append((inst, members, cs))

    print(f"    多集合 + 容器唯一 : {len(multi_ok)} 台   ← 正例（去重成一条）")
    print(f"    多集合 + 跨多容器 : {len(multi_bad)} 台   ← ⛔ 若 >0 则该设备会被拆开")

    for inst, members, cs in multi_ok[:10]:
        models = "/".join(sorted(inst_models.get(inst, {"?"})))
        c = sorted(cs)[0] if cs else "（全在占位容器）"
        print(f"      ✅ {inst}  {models}  {len(members)} 个集合 → 容器 {c}")
        for m in sorted(members)[:3]:
            print(f"           {m}")
        if len(members) > 3:
            print(f"           … 另 {len(members) - 3} 条")

    if multi_bad:
        for inst, members, cs in multi_bad:
            print(f"      ⛔ {inst} → {len(cs)} 个容器: {sorted(cs)}")
            for m in sorted(members):
                print(f"           {m}")
    else:
        print("      ✅ 无跨容器设备 ⇒ 容器优先路径**不会**把一台设备拆成多台")

    # ── 3. 降级路径 B：i: 键必然是逐集合的 ───────────────────
    print(f"\n[B] 降级路径 `i:<实例路径>`：逐 HID 集合，天然会拆")
    demoted = [i for i, ms in inst_members.items() if len(ms) > 1]
    print(f"    本机「多集合」物理设备 {len(demoted)} 台；")
    print(f"    若容器解析失败，这 {len(demoted)} 台设备会各被拆成多份 `i:` 键")
    print(f"    ⚠️ 注意方向：拆细**不会**跨设备串号（不同设备的实例路径必然不同），")
    print(f"       代价只是「一台设备出现多条、各显示同一个值」。")

    # ── 4. 占位容器里的 HID ──────────────────────────────────
    print(f"\n[4] 落在**占位容器**的 HID 集合（会被 usable_container 过滤掉 ⇒ 走降级）")
    ph = {i: ms for i, ms in inst_members.items() if i in inst_null and not inst_containers.get(i)}
    print(f"    纯占位容器设备: {len(ph)} 台")
    for i, ms in sorted(ph.items())[:6]:
        print(f"      {i}  {len(ms)} 个集合")

    # ── 5. 同型号多台：按型号聚合，看容器数 ──────────────────
    print(f"\n[5] 同型号多台：型号 → 物理设备数 / 容器数")
    model_inst = defaultdict(set)
    for inst, models in inst_models.items():
        for mo in models:
            model_inst[mo].add(inst)
    multi_model = {m: s for m, s in model_inst.items() if len(s) > 1}
    if not multi_model:
        print("    本机无「同型号多台」样本")
    for mo, insts in sorted(multi_model.items()):
        cs = set()
        for i in insts:
            cs |= inst_containers[i]
        flag = "✅可分域" if len(cs) == len(insts) else ("⚠️容器数<设备数" if len(cs) > 1 else "⛔全部同容器")
        print(f"    {mo}: {len(insts)} 台设备 / {len(cs)} 个容器  {flag}")
        for i in sorted(insts):
            print(f"         {i}  {len(inst_members[i])} 个集合  容器={sorted(inst_containers[i]) or '占位'}")


    # ── 6. 每个容器含多少个 HID 集合（= 会被判为 2.4G 的目标数） ──
    print(f"\n[6] 每个**容器**含多少个 HID 集合 —— 直接对应「同一台设备会有几个查询目标」")
    per_container = defaultdict(list)
    for guid, members in idx.items():
        for m in members:
            if m.upper().startswith("HID\\"):
                per_container[guid].append(m)
    hist = defaultdict(int)
    for guid, ms in per_container.items():
        hist[len(ms)] += 1
    print(f"    容器数 {len(per_container)}，集合数分布（集合数: 容器个数）：")
    for n in sorted(hist):
        print(f"        {n:>2} 个集合 : {hist[n]:>2} 个容器")
    multi_c = {g: ms for g, ms in per_container.items() if len(ms) > 1}
    print(f"    含 **多个** HID 集合的容器 {len(multi_c)} 个 —— 这些容器下的全部集合")
    print(f"    在容器优先路径下会拿到**同一个** `c:` 键 ⇒ 去重成 1 个查询目标。")
    for g, ms in sorted(multi_c.items(), key=lambda kv: -len(kv[1]))[:8]:
        kind = "⛔占位" if g in NULL_CONTAINERS else "✅真实"
        models = sorted({
            f"{m.split('\\')[1].upper().split('VID_')[1][:4]}:{m.split('\\')[1].upper().split('PID_')[1][:4]}"
            for m in ms if "VID_" in m.upper() and "PID_" in m.upper()
        })
        print(f"      {kind} {g}  {len(ms)} 个集合  型号={models or '?'}")


if __name__ == "__main__":
    main()
