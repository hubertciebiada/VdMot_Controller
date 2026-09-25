#!/usr/bin/env python3
"""The STM32 glue suite compiles the firmware sources with the -D flags of the release builds.

Compares the -D macros of [env:STM32_release_C2] and [env:STM32_release_C1] in
software_stm32/platformio.ini (the ${section.option} values they use resolved) with
VDM_GLUE_DEFINES and VDM_GLUE_DEFINES_C1 of software_stm32/test/native/glue/CMakeLists.txt, and exits
1 naming every difference. tools/native/docker.sh test stm32 runs it before cmake.

  python3 tools/native/check_release_flags.py [--root <repository root>]
"""
from __future__ import annotations

import argparse
import configparser
import os
import re
import shlex
import sys

INI = os.path.join("software_stm32", "platformio.ini")
CMAKE = os.path.join("software_stm32", "test", "native", "glue", "CMakeLists.txt")
# release env -> CMake list of the glue build
PAIRS = [("env:STM32_release_C2", "VDM_GLUE_DEFINES"), ("env:STM32_release_C1", "VDM_GLUE_DEFINES_C1")]
# defined by the framework, not by build_flags: allowed in the CMake lists without an ini flag
FRAMEWORK = {"USE_HAL_DRIVER", "STM32F4"}
REFERENCE = re.compile(r"\$\{([^.}]+)\.([^}]+)\}")


def resolve(cfg: configparser.ConfigParser, section: str, option: str, depth: int = 0) -> str:
    """The option's value with every ${section.option} reference replaced (PlatformIO syntax)."""
    if depth > 10:
        raise ValueError(f"[{section}] {option}: references nest too deep")
    value = cfg.get(section, option)
    return REFERENCE.sub(lambda m: resolve(cfg, m.group(1), m.group(2).lower(), depth + 1), value)


def ini_defines(path: str, section: str) -> dict[str, str | None]:
    cfg = configparser.ConfigParser(interpolation=None)
    with open(path, encoding="utf-8") as f:
        cfg.read_file(f)
    tokens = shlex.split(resolve(cfg, section, "build_flags"), comments=True)
    defines: dict[str, str | None] = {}
    for i, t in enumerate(tokens):
        if t == "-D" and i + 1 < len(tokens):
            t = "-D" + tokens[i + 1]
        if t.startswith("-D") and len(t) > 2:
            name, eq, value = t[2:].partition("=")
            defines[name] = value if eq else None
    return defines


def cmake_defines(path: str, variable: str) -> dict[str, str | None]:
    with open(path, encoding="utf-8") as f:
        text = f.read()
    text = re.sub(r"#[^\n]*", "", text)
    m = re.search(r"\bset\(\s*" + re.escape(variable) + r"\s+([^)]*)\)", text)
    if m is None:
        raise ValueError(f"{path}: no set({variable} ...)")
    defines: dict[str, str | None] = {}
    for item in shlex.split(m.group(1), posix=False):
        name, eq, value = item.partition("=")
        defines[name] = value if eq else None
    return defines


def show(name: str, value: str | None) -> str:
    return f"-D{name}" if value is None else f"-D{name}={value}"


def compare(ini: dict[str, str | None], glue: dict[str, str | None], env: str, variable: str) -> list[str]:
    problems = []
    for name in sorted(set(ini) | set(glue)):
        if name in ini and name not in glue:
            problems.append(f"[{env}] has {show(name, ini[name])}, {variable} does not")
        elif name in glue and name not in ini:
            if name not in FRAMEWORK:
                problems.append(f"{variable} has {show(name, glue[name])}, [{env}] does not")
        elif ini[name] != glue[name]:
            problems.append(f"[{env}] has {show(name, ini[name])}, {variable} has {show(name, glue[name])}")
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
    args = parser.parse_args(argv)
    ini_path = os.path.join(args.root, INI)
    cmake_path = os.path.join(args.root, CMAKE)
    problems = []
    try:
        for section, variable in PAIRS:
            problems += compare(ini_defines(ini_path, section), cmake_defines(cmake_path, variable),
                                section, variable)
    except (OSError, ValueError, configparser.Error) as e:
        print(f"check_release_flags: {e}", file=sys.stderr)
        return 2
    if problems:
        print("check_release_flags: the glue build does not compile like the release build:", file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        print(f"  fix {CMAKE} or {INI}", file=sys.stderr)
        return 1
    print("check_release_flags: " + ", ".join(f"{v} = [{s}]" for s, v in PAIRS))
    return 0


if __name__ == "__main__":
    sys.exit(main())
