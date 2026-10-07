#!/usr/bin/env python3
"""Package VdMot Revamped release assets.

C++ firmware (software_stm32, software_esp32_revamped).
Input: a directory with the CI build artifacts (as produced by actions/download-artifact):
  <artifacts>/stm32-<env>/<env>.bin              env in STM32_release_C1/C2, STM32F411_release_C1/C2
  <artifacts>/esp32-revamped/esp32-revamped.bin  (+ esp32-revamped_nodigest.bin)
Local builds work too: --from-builds points at the repo and picks .pio/build/*/firmware.bin.

Rust firmware (software_stm32_rust, software_esp32_rust; docs/rust/README.md):
  <artifacts>/stm32-rust/STM32F401_C1.bin ... STM32F411_C2.bin       tools/rust/docker.sh fw
  <artifacts>/esp32-rust/vdm-esp-fw.bin (+ vdm-esp-fw_nodigest.bin)  tools/rust/esp/docker.sh export
--from-builds takes them from software_stm32_rust/firmware/images/ and
software_esp32_rust/firmware/images/.

A tag is a Rust release when its version (a release candidate without its -rcN) is a version of
the Rust firmware: the STM32 workspace version (software_stm32_rust/Cargo.toml), the ESP32
VDM_VERSION (software_esp32_rust/firmware/.cargo/config.toml) or the ESP32 crate version
(software_esp32_rust/firmware/Cargo.toml, the app descriptor). All three must be that version,
and every Rust image must carry it (the STM32 ID block with the board tag and the stack pointer
of the asset's name; the ESP32 app descriptor and the reported version) before anything is
packaged. --firmware overrides the choice; --firmware-of-tag prints it.

Output (--out): release assets with unmistakable names, SHA256SUMS, manifest.json,
INSTALL.md and RELEASE_NOTES.md (C++: docs/revamped/INSTALL.md and the section of the version
in docs/revamped/CHANGELOG.md; Rust: docs/rust/INSTALL.md and the section in
docs/rust/CHANGES.md, whose relative links then point at the tag on GitHub when
GITHUB_REPOSITORY names the repository, as in CI). With --releases-dir the binaries are also
copied into the upstream-style folder layout  releases/revamped/<version>/
(ESP32_revamped_firmware.bin, STM32F411_C2_revamped_firmware.bin, ...).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import posixpath
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

# The Rust images: the STM32 ones carry the asset names, the ESP32 ones come from the export
RUST_STM = list(STM_ENVS.values())
RUST_ESP = {"esp32": "vdm-esp-fw.bin", "esp32_nodigest": "vdm-esp-fw_nodigest.bin"}
RUST_UPSTREAM_STYLE = {name: UPSTREAM_STYLE[env] for env, name in STM_ENVS.items()}
RUST_UPSTREAM_STYLE.update({k: UPSTREAM_STYLE[k] for k in RUST_ESP})
# STM32: the ID block at 0x08000200 (docs/rust/GLUE-DESIGN-STM.md §6.2) and the initial stack
# pointer of the chip (§5.6)
STM_ID_BLOCK = 0x200
STM_SP0 = {"STM32F401": 0x2001_0000, "STM32F411": 0x2002_0000}
# ESP32 app image: 24-byte image header, 8-byte header of the first segment, then esp_app_desc_t
# (magic word, secure version, 2 reserved words, version[32], project_name[32], ...)
ESP_IMAGE_MAGIC = 0xE9
ESP_APP_DESC = 0x20
ESP_APP_DESC_MAGIC = 0xABCD5432


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


def find_rust_inputs(args) -> dict[str, str]:
    places = []
    if args.artifacts:
        places.append((os.path.join(args.artifacts, "stm32-rust"), os.path.join(args.artifacts, "esp32-rust")))
    if args.from_builds:
        places.append((os.path.join(args.from_builds, "software_stm32_rust", "firmware", "images"),
                       os.path.join(args.from_builds, "software_esp32_rust", "firmware", "images")))
    found: dict[str, str] = {}
    for stm_dir, esp_dir in places:
        for name in RUST_STM:
            p = os.path.join(stm_dir, name + ".bin")
            if os.path.isfile(p):
                found.setdefault(name, p)
        for key, name in RUST_ESP.items():
            p = os.path.join(esp_dir, name)
            if os.path.isfile(p):
                found.setdefault(key, p)
    return found


def first_match(path: str, pattern: str) -> str | None:
    if not os.path.isfile(path):
        return None
    with open(path, encoding="utf-8") as f:
        for line in f:
            m = re.match(pattern, line.rstrip("\r\n"))
            if m:
                return m.group(1)
    return None


RUST_VERSION_SOURCES = (
    # the ID block of the four STM32 images (firmware/build.rs), gvers
    ("software_stm32_rust version", ("software_stm32_rust", "Cargo.toml"), r'^version = "(.*)"$'),
    # the version the ESP32 firmware reports (core/src/version.rs), the dashboard shows
    ("software_esp32_rust VDM_VERSION", ("software_esp32_rust", "firmware", ".cargo", "config.toml"),
     r'^VDM_VERSION = "(.*)"$'),
    # the version of the ESP32 app descriptor (esp_app_desc_t), the package version of the crate
    ("software_esp32_rust/firmware version", ("software_esp32_rust", "firmware", "Cargo.toml"),
     r'^version = "(.*)"$'),
)


def rust_versions(repo: str) -> dict[str, str | None]:
    """The versions of the Rust firmware by source; a release needs one version in all."""
    return {what: first_match(os.path.join(repo, *path), pattern) for what, path, pattern in RUST_VERSION_SOURCES}


def base_version(version: str) -> str:
    """The firmware version of a release version: a release candidate ships its version."""
    return re.sub(r"-rc\d+$", "", version)


def firmware_of(repo: str, version: str) -> str:
    return "rust" if base_version(version) in rust_versions(repo).values() else "cpp"


def check_rust_image(key: str, path: str, version: str) -> str | None:
    """Why the Rust image `key` at `path` is not the image of `version`, or None."""
    with open(path, "rb") as f:
        data = f.read()
    if key in RUST_STM:
        chip, board = key.split("_")
        want = b"\0" + version.encode() + b"\0VDM-HW:" + board.encode() + b"\0"
        got = data[STM_ID_BLOCK:STM_ID_BLOCK + len(want)]
        if got != want:
            return f"ID block {got!r}, expected {want!r}"
        sp = int.from_bytes(data[0:4], "little")
        if sp != STM_SP0[chip]:
            return f"initial SP 0x{sp:08x}, {chip} has 0x{STM_SP0[chip]:08x}"
        return None
    if len(data) < ESP_APP_DESC + 0x50 or data[0] != ESP_IMAGE_MAGIC:
        return "not an ESP32 app image"
    if int.from_bytes(data[ESP_APP_DESC:ESP_APP_DESC + 4], "little") != ESP_APP_DESC_MAGIC:
        return "no app descriptor after the first segment header"
    field = data[ESP_APP_DESC + 0x10:ESP_APP_DESC + 0x30].split(b"\0", 1)[0]
    if field != version.encode():
        return f"app descriptor version {field.decode(errors='replace')!r}, expected {version!r}"
    if version.encode() not in data[ESP_APP_DESC + 0x30:]:
        return f"no {version!r} in the image beside the app descriptor (VDM_VERSION)"
    return None


def changelog_section(repo: str, version: str, rel: str = os.path.join("docs", "revamped", "CHANGELOG.md")) -> str:
    path = os.path.join(repo, rel)
    if not os.path.isfile(path):
        return ""
    text = open(path, encoding="utf-8").read()
    # a release candidate (2.1.0-revamped-rc1) ships the changes of its version
    for v in (version, base_version(version)):
        m = re.search(rf"^## \[?{re.escape(v)}\]?.*?$(.*?)(?=^## |\Z)", text, re.M | re.S)
        if m:
            return m.group(1).strip()
    return ""


def link_to_tag(text: str, src_dir: str, base_url: str | None) -> str:
    """Relative Markdown links of a file in src_dir (repository path) as links to base_url."""
    if not base_url:
        return text

    def repl(m: re.Match) -> str:
        target = m.group(2)
        if re.match(r"^(?:[a-z][a-z0-9+.-]*:|#|/)", target, re.I):
            return m.group(0)
        path, sep, frag = target.partition("#")
        resolved = posixpath.normpath(posixpath.join(src_dir, path))
        return f"{m.group(1)}({base_url}/{resolved}{sep}{frag})"

    return re.sub(r"(\[[^\]]*\])\(([^)\s]+)\)", repl, text)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tag", required=True, help="git tag, e.g. v2.1.0-revamped")
    ap.add_argument("--artifacts", help="downloaded CI artifacts directory")
    ap.add_argument("--from-builds", help="repo root with local builds")
    ap.add_argument("--out")
    ap.add_argument("--releases-dir", help="also copy binaries to <dir>/<version>/ (upstream layout)")
    ap.add_argument("--repo", default=os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..")))
    ap.add_argument("--require-all", action="store_true", default=True)
    ap.add_argument("--firmware", choices=("auto", "cpp", "rust"), default="auto",
                    help="auto: rust when the tag carries the version of the Rust firmware")
    ap.add_argument("--firmware-of-tag", action="store_true", help="print cpp or rust for --tag and exit")
    args = ap.parse_args()

    version = args.tag[1:] if args.tag.startswith("v") else args.tag
    if "-revamped" not in version:
        print(f"tag {args.tag} must contain '-revamped'", file=sys.stderr)
        return 2
    firmware = firmware_of(args.repo, version) if args.firmware == "auto" else args.firmware
    if args.firmware_of_tag:
        print(firmware)
        return 0
    if not args.out:
        print("--out is required", file=sys.stderr)
        return 2
    rust = firmware == "rust"
    print(f"firmware: {'Rust' if rust else 'C++'}")

    if rust:
        base = base_version(version)
        wrong = [f"{what} is {v}" for what, v in rust_versions(args.repo).items() if v != base]
        if wrong:
            print(f"tag {args.tag} is not the version of the Rust firmware: " + "; ".join(wrong), file=sys.stderr)
            return 1
        inputs = find_rust_inputs(args)
        required = RUST_STM + list(RUST_ESP)
    else:
        inputs = find_inputs(args)
        required = list(STM_ENVS) + ["esp32", "esp32_nodigest"]
    missing = [k for k in required if k not in inputs]
    if missing and args.require_all:
        print("missing inputs: " + ", ".join(missing), file=sys.stderr)
        return 1
    if rust:
        bad = [f"{inputs[k]}: {why}" for k in sorted(inputs) if (why := check_rust_image(k, inputs[k], base))]
        if bad:
            print("images that are not the release:\n  " + "\n  ".join(bad), file=sys.stderr)
            return 1

    os.makedirs(args.out, exist_ok=True)
    assets = []
    for key, src in sorted(inputs.items()):
        if key in STM_ENVS or key in RUST_STM:
            stm = STM_ENVS.get(key, key)
            name, target = f"{PRODUCT}_{version}_{stm}.bin", stm
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
            shutil.copyfile(src, os.path.join(rdir, (RUST_UPSTREAM_STYLE if rust else UPSTREAM_STYLE)[key]))

    with open(os.path.join(args.out, "SHA256SUMS"), "w") as f:
        for a in assets:
            f.write(f"{a['sha256']}  {a['file']}\n")
    if args.releases_dir:
        rdir = os.path.join(args.releases_dir, version)
        with open(os.path.join(rdir, "SHA256SUMS"), "w") as f:
            for n in sorted(os.listdir(rdir)):
                if n.endswith(".bin"):
                    f.write(f"{sha256(os.path.join(rdir, n))}  {n}\n")

    manifest = {"product": "VdMot Revamped", "version": version, "tag": args.tag}
    if rust:
        manifest["firmware"] = "rust"
    manifest.update({"note": "Community fork build. NOT an official VdMot_Controller release.", "assets": assets})
    json.dump(manifest, open(os.path.join(args.out, "manifest.json"), "w"), indent=2)

    if rust:
        repository = os.environ.get("GITHUB_REPOSITORY")
        server = os.environ.get("GITHUB_SERVER_URL", "https://github.com")
        base_url = f"{server}/{repository}/blob/{args.tag}" if repository else None
        install = os.path.join(args.repo, "docs", "rust", "INSTALL.md")
        if os.path.isfile(install):
            text = open(install, encoding="utf-8").read()
            with open(os.path.join(args.out, "INSTALL.md"), "w", encoding="utf-8") as f:
                f.write(link_to_tag(text, "docs/rust", base_url))
        notes = [f"# VdMot Revamped {version} (Rust)", "",
                 "> Unofficial, independently maintained firmware for the VdMot Controller "
                 "(fork of Lenti84/SurfGargano VdMot_Controller). Not supported by the upstream project.", "",
                 "ESP32 and STM32 firmware of the Rust port of VdMot Revamped 2.1.7 (docs/rust/README.md). "
                 "Read INSTALL.md before the first flash: the bench unit, the trial of the new ESP32 image "
                 "and the way back to the C++ firmware 2.1.7.", ""]
        section = changelog_section(args.repo, version, os.path.join("docs", "rust", "CHANGES.md"))
        if section:
            notes += [link_to_tag(section, "docs/rust", base_url), ""]
    else:
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
    open(os.path.join(args.out, "RELEASE_NOTES.md"), "w", encoding="utf-8").write("\n".join(notes) + "\n")
    print("\n".join(f"{a['file']}  {a['bytes']} B" for a in assets))
    return 0


if __name__ == "__main__":
    sys.exit(main())
