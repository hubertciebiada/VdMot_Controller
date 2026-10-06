#!/usr/bin/env python3
"""Size of a firmware app image against the budget of the devices, and what fills it.

    image_size.py <app.bin> [<linker.map>]

The budget is the one of the C++ firmware (software_esp32_revamped/tools/check_image_size.py):
1,228,800 B of the 1,310,720 B OTA slot. With a GNU ld map file the bytes that go into the image
(flash code and constants, IRAM code, initialised DRAM; not .bss) are summed per library.
"""
from __future__ import annotations

import os
import re
import sys
from collections import defaultdict

BUDGET = 1_228_800
SLOT = 0x140000

# output sections whose input sections are part of the image
LOADED = re.compile(r"^\.(flash\.(text|rodata|appdesc|rodata_noload)|iram0\.(text|vectors|data)|"
                    r"dram0\.data|rtc\.(text|data)|rtc_noinit_dummy)")
NOT_LOADED = re.compile(r"(\.bss|\.noinit|rodata_noload|\.rtc_noinit)")
INPUT = re.compile(r"^\s*(\.[^\s]+)?\s+0x([0-9a-f]+)\s+0x([0-9a-f]+)\s+(\S+)$")


def owner(path: str) -> str:
    base = os.path.basename(path)
    m = re.match(r"(lib[^()]+\.a)\(", base)
    if m:
        return m.group(1)
    if ".rcgu." in base:
        # fat LTO: std, esp-idf-svc/hal/sys and the firmware are one object
        return "Rust (std, esp-idf-svc/hal/sys, vdm-esp-fw; one LTO object)"
    return base


def map_breakdown(path: str) -> list[tuple[str, int]]:
    sizes: dict[str, int] = defaultdict(int)
    out_section = ""
    pending_name = None
    with open(path, encoding="utf-8", errors="replace") as f:
        for raw in f:
            line = raw.rstrip("\n")
            if line and not line.startswith(" "):
                out_section = line.split()[0]
                continue
            if not LOADED.match(out_section) or NOT_LOADED.search(out_section):
                continue
            stripped = line.strip()
            if pending_name is not None:
                m = re.match(r"^0x([0-9a-f]+)\s+0x([0-9a-f]+)\s+(\S+)$", stripped)
                if m:
                    if not NOT_LOADED.search(pending_name):
                        sizes[owner(m.group(3))] += int(m.group(2), 16)
                pending_name = None
                continue
            m = INPUT.match(line)
            if m and m.group(1):
                if not NOT_LOADED.search(m.group(1)):
                    sizes[owner(m.group(4))] += int(m.group(3), 16)
            elif re.match(r"^\s\.[^\s]+$", line):
                pending_name = stripped
    return sorted(sizes.items(), key=lambda kv: -kv[1])


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    image = sys.argv[1]
    size = os.path.getsize(image)
    print(f"image {image}: {size:,} B")
    print(f"budget {BUDGET:,} B: headroom {BUDGET - size:+,} B ({100 * size / BUDGET:.1f} % used); "
          f"slot {SLOT:,} B: headroom {SLOT - size:+,} B")
    if len(sys.argv) > 2 and os.path.exists(sys.argv[2]):
        rows = map_breakdown(sys.argv[2])
        total = sum(v for _, v in rows)
        print(f"map {sys.argv[2]}: {total:,} B in loaded sections; largest contributors:")
        for name, v in rows[:25]:
            print(f"  {v:>9,} B  {100 * v / max(total, 1):5.1f} %  {name}")
    return 0 if size <= BUDGET else 1


if __name__ == "__main__":
    sys.exit(main())
