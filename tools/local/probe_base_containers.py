"""只读探针 v3：`Control\\DeviceContainers\\<guid>\\BaseContainers` 的派生图。

v2 得到一个干净的交叉表：
    v1(时间戳)  端口派生 25 / 序列号形态 0
    v5(名字哈希) 端口派生 15 / 序列号形态 9
且 15 个「端口派生但 v5」的实例**恰好**都是序列号形态设备的子节点（IG_00 / MI_0x）。

本探针验证这个「子节点继承父容器」的说法是否被注册表**显式记录**：
`BaseContainers` 子键若指向父容器，则继承关系是**直接观测**而非推断。

全部只读。
"""
import sys

import winreg

sys.stdout.reconfigure(encoding="utf-8")

DC = r"SYSTEM\CurrentControlSet\Control\DeviceContainers"


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


def dump(path, depth=0, max_depth=3, limit=12):
    pad = "  " * depth
    for vn, (vt, vd) in list(values(path).items())[:limit]:
        print(f"{pad}  · {vn} = {as_str(vd)[:90]!r}")
    if depth >= max_depth:
        return
    for sk in subkeys(path):
        print(f"{pad}  └ {sk}")
        dump(f"{path}\\{sk}", depth + 1, max_depth, limit)


TARGETS = [
    # (说明, 容器 GUID) —— 一个 v5、一个 v1，对照
    ("v5 序列号派生（DUNU DTC100pro 耳机）", "82434cf9-6c86-5716-86b4-116c85d16b28"),
    ("v1 端口派生（25A7:FA70 接收器）", "0d85362f-9ba5-11f1-b7f2-105fadd8248b"),
    ("占位容器（对照）", "00000000-0000-0000-ffff-ffffffffffff"),
]


def main():
    print("=" * 78)
    print("§12.1 容器派生图：BaseContainers 实测")
    print("=" * 78)

    allc = subkeys(DC)
    print(f"\nDeviceContainers 条目总数: {len(allc)}")

    # 全局：BaseContainers 里到底存了什么
    print("\n[1] 全量统计 BaseContainers 子键名（看它是不是「父容器」）")
    from collections import Counter

    base_children = Counter()
    props_keys = Counter()
    for c in allc:
        for bc in subkeys(f"{DC}\\{c}\\BaseContainers"):
            base_children[bc] += 1
        for pk in subkeys(f"{DC}\\{c}\\Properties"):
            props_keys[pk] += 1
    print(f"    BaseContainers 下的子键名分布: {dict(base_children)}")
    print(f"    Properties 下的子键名分布: {dict(props_keys)}")

    # 逐个目标 dump
    for label, guid in TARGETS:
        key = "{" + guid.upper() + "}"
        p = f"{DC}\\{key}"
        print(f"\n[2] {label}")
        print(f"    {key}")
        if not subkeys(p) and not values(p):
            print("      （不存在或为空）")
            continue
        dump(p)

    # 找「一个容器的 BaseContainers 指向另一个容器」的证据
    print("\n[3] 是否存在容器间父子链（BaseContainers 指向别的容器 GUID）")
    linked = 0
    for c in allc:
        for bc in subkeys(f"{DC}\\{c}\\BaseContainers"):
            if bc.startswith("{"):
                linked += 1
                if linked <= 10:
                    print(f"      {c}  ←BaseContainers←  {bc}")
    print(f"    带容器型 BaseContainers 的条目数: {linked}")
    if linked == 0:
        print("      ⇒ BaseContainers 不存「父容器」⇒ 继承关系**未被显式记录**，")
        print("        故「子节点继承父容器」在 v2 里是**由共现模式推断**（15/15 完全吻合）。")


if __name__ == "__main__":
    main()
