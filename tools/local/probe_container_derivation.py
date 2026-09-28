"""只读探针 v2：ContainerId 的**派生方式**判别（设计稿 §12.1）。

v1 的发现：容器 GUID 的版本位不同 ——
  `18704038-c0af-5b9f-…` 是 **v5**（名字哈希，确定性、可复现）
  `0d85362f-9ba5-11f1-…` 是 **v1**（时间戳生成，一次生成后落盘）

假设 H：v5 ⇒ 容器由**设备自报的稳定标识**（序列号）哈希得到 ⇒ **跨 USB 口稳定**；
        v1 ⇒ Windows 在枚举时**生成**、绑定实例路径 ⇒ **换口即变**。

本探针做三件事（都可证伪）：
  A. 版本 × 实例形态 交叉表 —— 若 v5 与「序列号形态」高度共现、v1 与「端口派生」共现，H 得到支持。
  B. ConfigFlags 幽灵实例检测（`CONFIGFLAG_REMOVED`=0x1）—— 换过口的设备会留下幽灵，
     是「换口」这一事件的**间接痕迹**。
  C. UUIDv5 复现尝试 —— 若能用「序列号」在某个标准命名空间下**算出**同一个容器 GUID，
     则「v5 = 序列号哈希」从假设升级为**实测**。

全部只读。
"""
import re
import struct
import sys
import uuid
from collections import defaultdict

import winreg

sys.stdout.reconfigure(encoding="utf-8")

ENUM_USB = r"SYSTEM\CurrentControlSet\Enum\USB"
DEV_CONTAINERS = r"SYSTEM\CurrentControlSet\Control\DeviceContainers"
NULL_CONTAINERS = {
    "00000000-0000-0000-0000-000000000000",
    "00000000-0000-0000-ffff-ffffffffffff",
}
CONFIGFLAG_REMOVED = 0x0001
CONFIGFLAG_REINSTALL = 0x0020


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


def guid_from_reg_binary(raw):
    if not isinstance(raw, (bytes, bytearray)) or len(raw) < 16:
        return None
    b = raw[-16:]
    d1 = struct.unpack("<I", b[0:4])[0]
    d2 = struct.unpack("<H", b[4:6])[0]
    d3 = struct.unpack("<H", b[6:8])[0]
    return f"{d1:08x}-{d2:04x}-{d3:04x}-{b[8:10].hex()}-{b[10:16].hex()}"


def as_str(raw):
    if isinstance(raw, (bytes, bytearray)):
        return raw.decode("utf-16-le", "replace").rstrip("\x00")
    return str(raw)


def guid_version(g):
    """GUID 版本 = 第三组首字符。v5=名字哈希，v1=时间戳，v4=随机。"""
    if not g or g in NULL_CONTAINERS:
        return None
    return int(g.split("-")[2][0], 16)


def collect():
    rows = []
    for vidpid in subkeys(ENUM_USB):
        for inst in subkeys(f"{ENUM_USB}\\{vidpid}"):
            p = f"{ENUM_USB}\\{vidpid}\\{inst}"
            v = values(p)
            raw = v.get("ContainerID")
            cid = None
            if raw:
                # ⚠️ Enum\USB 下 ContainerID 是 REG_SZ（带花括号字符串），
                # 不是 REG_BINARY —— 同名键在不同位置存储类型不同，别想当然。
                cid = as_str(raw[1]).strip().strip("{}").lower() if raw[0] == winreg.REG_SZ \
                    else guid_from_reg_binary(raw[1])
            desc = as_str(v["DeviceDesc"][1]) if "DeviceDesc" in v else ""
            cfg = v.get("ConfigFlags")
            cfg = cfg[1] if cfg else None
            last = inst.rsplit("\\", 1)[-1] if "\\" in inst else inst
            rows.append(
                {
                    "vidpid": vidpid,
                    "inst": inst,
                    "last": last,
                    "cid": cid,
                    "desc": desc,
                    "cfg": cfg,
                    "port": "&" in last,
                }
            )
    return rows


def main():
    rows = collect()
    print("=" * 78)
    print("§12.1 容器派生方式判别 —— 只读取证 v2")
    print("=" * 78)

    # ── A. 版本 × 形态 交叉表 ─────────────────────────────
    print("\n[A] 容器 GUID 版本 × 实例形态 交叉表")
    print(f"    实例总数 {len(rows)}；占位容器 {sum(1 for r in rows if r['cid'] in NULL_CONTAINERS)}")
    tab = defaultdict(int)
    for r in rows:
        ver = guid_version(r["cid"])
        form = "端口派生" if r["port"] else "序列号形态"
        tab[(ver, form)] += 1
    print(f"    {'版本':<8}{'端口派生':>10}{'序列号形态':>12}")
    for ver in sorted({k[0] for k in tab}, key=lambda x: (x is None, x)):
        label = {None: "占位/无", 1: "v1(时间戳)", 4: "v4(随机)", 5: "v5(名字哈希)"}.get(ver, f"v{ver}")
        print(f"    {label:<8}{tab[(ver,'端口派生')]:>10}{tab[(ver,'序列号形态')]:>12}")

    # ── B. 幽灵实例（换口的间接痕迹）──────────────────────
    print("\n[B] 幽灵实例检测（ConfigFlags 位）")
    ghosts = [r for r in rows if r["cfg"] is not None and (r["cfg"] & CONFIGFLAG_REMOVED)]
    print(f"    带 CONFIGFLAG_REMOVED(0x1) 的实例: {len(ghosts)}")
    for r in ghosts:
        print(f"      {r['vidpid']}\\{r['last']}  cfg=0x{r['cfg']:x}  cid={r['cid']}")
    if not ghosts:
        print("      （无幽灵实例 ⇒ 本机未观察到「设备换过口/被移除」的痕迹）")

    # ── C. UUIDv5 复现尝试 ────────────────────────────────
    print("\n[C] UUIDv5 复现尝试（验证「v5 = 稳定标识的哈希」）")
    ns_map = {
        "DNS": uuid.NAMESPACE_DNS,
        "URL": uuid.NAMESPACE_URL,
        "OID": uuid.NAMESPACE_OID,
        "X500": uuid.NAMESPACE_X500,
        "NIL": uuid.UUID(int=0),
    }
    v5_rows = [r for r in rows if guid_version(r["cid"]) == 5]
    print(f"    v5 容器实例 {len(v5_rows)} 个，逐个尝试复现：")
    hit = 0
    for r in v5_rows:
        target = uuid.UUID(r["cid"])
        cands = [
            r["last"],
            r["inst"],
            f"{r['vidpid']}\\{r['last']}",
            r["last"].upper(),
            r["last"].lower(),
        ]
        found = None
        for ns_name, ns in ns_map.items():
            for c in cands:
                if uuid.uuid5(ns, c) == target:
                    found = f"{ns_name} / {c!r}"
                    break
            if found:
                break
        if found:
            hit += 1
            print(f"      ✓ 复现 {r['vidpid']}\\{r['last']} → {found}")
        else:
            print(f"      ✗ 未复现 {r['vidpid']}\\{r['last']}  cid={r['cid']}")
    print(f"    复现命中 {hit}/{len(v5_rows)}")
    if hit == 0:
        print("      ⇒ 标准 UUIDv5 命名空间下**无法复现** ⇒ Windows 用的是自有哈希方案，")
        print("        故「v5 = 序列号哈希」**仍属外推**，不是直接观测。")

    # ── D. 容器 → 实例 归属表（看一容器多实例的规模）────────
    print("\n[D] 容器 → USB 实例归属")
    by_cid = defaultdict(list)
    for r in rows:
        by_cid[r["cid"]].append(r)
    multi = sorted(((c, v) for c, v in by_cid.items() if len(v) > 1), key=lambda kv: -len(kv[1]))
    print(f"    容器数 {len(by_cid)}；其中聚合了多个 USB 实例的 {len(multi)} 个")
    for cid, v in multi[:8]:
        print(f"      {cid}  v{guid_version(cid)}  → {len(v)} 实例")
        for r in v:
            print(f"          {r['vidpid']}\\{r['last']:<28} port={str(r['port']):<5}")

    # ── E. DeviceContainers 历史条目 ──────────────────────
    print("\n[E] Control\\DeviceContainers 历史")
    dc = subkeys(DEV_CONTAINERS)
    print(f"    条目数 {len(dc)}")
    sample = dc[:3]
    for name in sample:
        print(f"      {name}")
        for k2 in subkeys(f"{DEV_CONTAINERS}\\{name}"):
            print(f"          └ {k2}  ({len(subkeys(f'{DEV_CONTAINERS}\\{name}\\{k2}'))} 子键)")
        for vn, (vt, vd) in list(values(f"{DEV_CONTAINERS}\\{name}").items())[:6]:
            print(f"          {vn} = {as_str(vd)[:70]!r}")


if __name__ == "__main__":
    main()
