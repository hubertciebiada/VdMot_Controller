#!/usr/bin/env python3
"""Package VdMot Revamped release assets.

Input: a directory with the CI build artifacts (as produced by actions/download-artifact):
  <artifacts>/stm32-<env>/<env>.bin              env in STM32_release_C1/C2, STM32F411_release_C1/C2
  <artifacts>/esp32-revamped/esp32-revamped.bin  (+ esp32-revamped_nodigest.bin)
Local builds work too: --from-builds points at the repo and picks .pio/build/*/firmware.bin.

Output (--out): release assets with unmistakable names, SHA256SUMS, manifest.json,
INSTALL.md and RELEASE_NOTES.md. With --releases-dir the binaries are also copied into
the upstream-style folder layout  releases/revamped/<version>/  (ESP32_revamped_firmware.bin,
STM32F411_C2_revamped_firmware.bin, ...).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import sys

PRODUCT = "VdMot-Revamped"
STM_ENVS = {
    "STM32_release_C1": "STM32F401_C1",
    "STM32_release_C2": "STM32F401_C2",
    "STM32F411_release_C1": "STM32F411_C1",
    "STM32F411_release_C2": "STM32F411_C2",
}
# upstream releases/ folder naming (releases/STM32/<ver>/STM32_C2_firmware.bin, STM32F411_C2_firmware.bin)
UPSTREAM_STYLE = {
    "STM32_release_C1": "STM32_C1_revamped_firmware.bin",
    "STM32_release_C2": "STM32_C2_revamped_firmware.bin",
    "STM32F411_release_C1": "STM32F411_C1_revamped_firmware.bin",
    "STM32F411_release_C2": "STM32F411_C2_revamped_firmware.bin",
    "esp32": "ESP32_revamped_firmware.bin",
    "esp32_nodigest": "ESP32_revamped_firmware_nodigest.bin",
}


def sha256(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def find_inputs(args) -> dict[str, str]:
    found: dict[str, str] = {}
    if args.artifacts:
        for env in STM_ENVS:
            p = os.path.join(args.artifacts, f"stm32-{env}", f"{env}.bin")
            if os.path.isfile(p):
                found[env] = p
        for key, name in (("esp32", "esp32-revamped.bin"), ("esp32_nodigest", "esp32-revamped_nodigest.bin")):
            p = os.path.join(args.artifacts, "esp32-revamped", name)
            if os.path.isfile(p):
                found[key] = p
    if args.from_builds:
        for env in STM_ENVS:
            p = os.path.join(args.from_builds, "software_stm32", ".pio", "build", env, "firmware.bin")
            if os.path.isfile(p):
                found.setdefault(env, p)
        bdir = os.path.join(args.from_builds, "software_esp32_revamped", ".pio", "build", "wt32-eth01_revamped")
        for key, name in (("esp32", "firmware.bin"), ("esp32_nodigest", "firmware_nodigest.bin")):
            p = os.path.join(bdir, name)
            if os.path.isfile(p):
                found.setdefault(key, p)
    return found


def changelog_section(repo: str, version: str) -> str:
    path = os.path.join(repo, "docs", "revamped", "CHANGELOG.md")
    if not os.path.isfile(path):
        return ""
    text = open(path, encoding="utf-8").read()
    # a release candidate (2.1.0-revamped-rc1) ships the changes of its version
    for v in (version, re.sub(r"-rc\d+$", "", version)):
        m = re.search(rf"^## \[?{re.escape(v)}\]?.*?$(.*?)(?=^## |\Z)", text, re.M | re.S)
        if m:
            return m.group(1).strip()
    return ""


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tag", required=True, help="git tag, e.g. v2.1.0-revamped")
    ap.add_argument("--artifacts", help="downloaded CI artifacts directory")
    ap.add_argument("--from-builds", help="repo root with local .pio builds")
    ap.add_argument("--out", required=True)
    ap.add_argument("--releases-dir", help="also copy binaries to <dir>/<version>/ (upstream layout)")
    ap.add_argument("--repo", default=os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..")))
    ap.add_argument("--require-all", action="store_true", default=True)
    args = ap.parse_args()

    version = args.tag[1:] if args.tag.startswith("v") else args.tag
    if "-revamped" not in version:
        print(f"tag {args.tag} must contain '-revamped'", file=sys.stderr)
        return 2
    inputs = find_inputs(args)
    required = list(STM_ENVS) + ["esp32", "esp32_nodigest"]
    missing = [k for k in required if k not in inputs]
    if missing and args.require_all:
        print("missing inputs: " + ", ".join(missing), file=sys.stderr)
        return 1

    os.makedirs(args.out, exist_ok=True)
    assets = []
    for key, src in sorted(inputs.items()):
        if key in STM_ENVS:
            name, target = f"{PRODUCT}_{version}_{STM_ENVS[key]}.bin", STM_ENVS[key]
        elif key == "esp32":
            name, target = f"{PRODUCT}_{version}_ESP32-WT32-ETH01.bin", "ESP32 WT32-ETH01"
        else:
            name, target = f"{PRODUCT}_{version}_ESP32-WT32-ETH01_nodigest.bin", "ESP32 WT32-ETH01 (no SHA256 digest)"
        dst = os.path.join(args.out, name)
        shutil.copyfile(src, dst)
        assets.append({"file": name, "target": target, "bytes": os.path.getsize(dst), "sha256": sha256(dst)})
        if args.releases_dir:
            rdir = os.path.join(args.releases_dir, version)
            os.makedirs(rdir, exist_ok=True)
            shutil.copyfile(src, os.path.join(rdir, UPSTREAM_STYLE[key]))

    with open(os.path.join(args.out, "SHA256SUMS"), "w") as f:
        for a in assets:
            f.write(f"{a['sha256']}  {a['file']}\n")
    if args.releases_dir:
        rdir = os.path.join(args.releases_dir, version)
        with open(os.path.join(rdir, "SHA256SUMS"), "w") as f:
            for n in sorted(os.listdir(rdir)):
                if n.endswith(".bin"):
                    f.write(f"{sha256(os.path.join(rdir, n))}  {n}\n")

    manifest = {"product": "VdMot Revamped", "version": version, "tag": args.tag,
                "note": "Community fork build. NOT an official VdMot_Controller release.", "assets": assets}
    json.dump(manifest, open(os.path.join(args.out, "manifest.json"), "w"), indent=2)

    install = os.path.join(args.repo, "docs", "revamped", "INSTALL.md")
    if os.path.isfile(install):
        shutil.copyfile(install, os.path.join(args.out, "INSTALL.md"))

    notes = [f"# VdMot Revamped {version}", "",
             "> Unofficial, independently maintained firmware for the VdMot Controller "
             "(fork of Lenti84/SurfGargano VdMot_Controller). Not supported by the upstream project.", ""]
    section = changelog_section(args.repo, version)
    if section:
        notes += [section, ""]
    notes += ["## Assets", "", "| file | target | bytes | sha256 |", "|---|---|---|---|"]
    notes += [f"| {a['file']} | {a['target']} | {a['bytes']} | `{a['sha256'][:16]}…` |" for a in assets]
    notes += ["", "Flashing order and recovery: see INSTALL.md."]
    open(os.path.join(args.out, "RELEASE_NOTES.md"), "w").write("\n".join(notes) + "\n")
    print("\n".join(f"{a['file']}  {a['bytes']} B" for a in assets))
    return 0


if __name__ == "__main__":
    sys.exit(main())
