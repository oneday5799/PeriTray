"""只读探针：判定 USB 设备的 ContainerId 是「序列号派生」还是「端口拓扑派生」。

问题（设计稿 §12.1）：换 USB 口是否改 ContainerId？
  - 若**序列号派生** ⇒ 跨口稳定 ⇒ 只存容器键就够，fallback 是纯保险。
  - 若**端口派生** ⇒ 换口即变 ⇒ fallback（名称键）是**必需**的。

判据设计（可证伪）：
  1. 实例路径末段含 `&` ⇒ 端口派生形态（`5&2a3b4c5d&0&1`）；
     末段是纯序列号（不含 `&`）⇒ 序列号形态。
  2. 找**同一序列号设备的历史多实例**：若同一 `VID/PID` + 序列号存在两个实例路径，
     且两者 ContainerID **相同** ⇒ 序列号派生（跨口稳定）得到直接证据。
  3. 若同一设备的多实例 ContainerID **不同** ⇒ 端口派生。
  4. 反控：若本机压根不存在「同设备多实例」，则 2/3 都无从判定 ⇒ 必须如实报告
     「无法判定」，而不是拿形态推测冒充实测。

全部只读，不写任何注册表。
"""
import winreg
import struct
import sys
from collections import defaultdict

sys.stdout.reconfigure(encoding="utf-8")

ENUM_USB = r"SYSTEM\CurrentControlSet\Enum\USB"
DEV_CONTAINERS = r"SYSTEM\CurrentControlSet\Control\DeviceContainers"
NULL_CONTAINERS = {
    "00000000-0000-0000-0000-000000000000",
    "00000000-0000-0000-ffff-ffffffffffff",
}


def subkeys(path):
    out = []
    try:
        k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, path)
    except OSError as e:
        print(f"[!] 打不开 {path}: {e}")
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
    """返回 {值名: (类型, 数据)}。"""
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


def guid_from_reg_binary(raw):
    """注册表里的 GUID 型 REG_BINARY 带 8 字节头，真实 GUID 在末尾 16 字节。

    前 4/2/2 字节小端（与 CFGMGR32 的 DEVPKEY 取值一致）。
    """
    if not isinstance(raw, (bytes, bytearray)) or len(raw) < 16:
        return None
    b = raw[-16:]
    d1 = struct.unpack("<I", b[0:4])[0]
    d2 = struct.unpack("<H", b[4:6])[0]
    d3 = struct.unpack("<H", b[6:8])[0]
    d4 = b[8:10].hex()
    d5 = b[10:16].hex()
    return f"{d1:08x}-{d2:04x}-{d3:04x}-{d4}-{d5}"


def fmt(raw, ty):
    if ty == winreg.REG_BINARY:
        g = guid_from_reg_binary(raw)
        return g if g else f"<binary {len(raw)}B>"
    if isinstance(raw, (bytes, bytearray)):
        return raw.decode("utf-16-le", "replace").rstrip("\x00")
    return str(raw)


def is_port_derived(last_segment):
    """末段含 `&` ⇒ 端口派生形态。"""
    return "&" in last_segment


def main():
    print("=" * 78)
    print("§12.1 ContainerId 是否随 USB 口变化 —— 只读取证")
    print("=" * 78)

    # ── 1. 全量扫 Enum\USB ────────────────────────────────
    tops = subkeys(ENUM_USB)
    print(f"\n[1] Enum\\USB 顶层 VID/PID 键数: {len(tops)}")

    # devdesc -> [(instance_path, container, last_segment, port_derived)]
    by_model = defaultdict(list)
    all_rows = []
    for vidpid in tops:
        for inst in subkeys(f"{ENUM_USB}\\{vidpid}"):
            p = f"{ENUM_USB}\\{vidpid}\\{inst}"
            v = values(p)
            cid = None
            if "ContainerID" in v:
                cid = fmt(v["ContainerID"][1], v["ContainerID"][0])
            desc = fmt(v["DeviceDesc"][1], v["DeviceDesc"][0]) if "DeviceDesc" in v else ""
            last = inst.rsplit("\\", 1)[-1] if "\\" in inst else inst
            all_rows.append((vidpid, inst, last, cid, desc, is_port_derived(last)))
            by_model[(vidpid, desc)].append((inst, last, cid, is_port_derived(last)))

    print(f"    实例总数: {len(all_rows)}")
    with_cid = [r for r in all_rows if r[3]]
    print(f"    带 ContainerID 的实例: {len(with_cid)}")
    null_cid = [r for r in with_cid if r[3] in NULL_CONTAINERS]
    print(f"    其中是占位容器的: {len(null_cid)}")

    port = [r for r in with_cid if r[5]]
    serial = [r for r in with_cid if not r[5]]
    print(f"\n[2] 形态分布（按实例路径末段是否含 `&`）")
    print(f"    端口派生（末段含 &）: {len(port)}")
    print(f"    序列号形态（末段无 &）: {len(serial)}")
    print("    示例 端口派生:")
    for r in port[:5]:
        print(f"      {r[0]}\\{r[1]}")
        print(f"        cid={r[3]}  desc={r[4]!r}")
    print("    示例 序列号形态:")
    for r in serial[:5]:
        print(f"      {r[0]}\\{r[1]}")
        print(f"        cid={r[3]}  desc={r[4]!r}")

    # ── 3. 关键判据：同一 (VID/PID + 描述) 的多实例容器是否一致 ──
    print(f"\n[3] 关键判据：同一型号多实例的容器一致性")
    multi = {k: v for k, v in by_model.items() if len(v) > 1}
    print(f"    多实例型号数: {len(multi)}")
    same_container = 0
    diff_container = 0
    for (vidpid, desc), rows in sorted(multi.items(), key=lambda kv: -len(kv[1])):
        cids = {r[2] for r in rows}
        tag = "同容器" if len(cids) == 1 else f"**{len(cids)} 个不同容器**"
        if len(cids) == 1:
            same_container += 1
        else:
            diff_container += 1
        print(f"    {vidpid}  {desc!r}  {len(rows)} 实例 → {tag}")
        for inst, last, cid, pd in rows:
            print(f"        {last:<34} port_derived={str(pd):<5} cid={cid}")
    print(f"\n    小结: 同容器 {same_container} 组 / 不同容器 {diff_container} 组")

    # ── 4. DeviceContainers 历史（换口会留下多条历史容器）──
    print(f"\n[4] Control\\DeviceContainers 历史")
    dc = subkeys(DEV_CONTAINERS)
    print(f"    历史容器条目数: {len(dc)}")

    # ── 5. 反控 ───────────────────────────────────────────
    print(f"\n[5] 反控")
    if not multi:
        print("    ✗ 本机不存在「同型号多实例」⇒ 判据 3 **无从判定**（必须如实报告无法判定）")
    else:
        print(f"    ✓ 存在 {len(multi)} 组多实例 ⇒ 判据 3 有区分度")
    if not port:
        print("    ✗ 本机不存在端口派生形态 ⇒ 无法观察「换口」这一情形")
    else:
        print(f"    ✓ 存在 {len(port)} 个端口派生实例 ⇒ 形态分类有区分度")


if __name__ == "__main__":
    main()
