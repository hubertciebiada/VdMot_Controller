#!/usr/bin/env python3
"""Source-level mutation testing for the VdMot Revamped native (host) test suites.

For every mutable token in the configured source files the tool applies one
mutation at a time, rebuilds and runs the test command, and records whether
the tests detected it (killed) or not (survived).

Usage:
  python3 tools/mutation/mutate.py --config tools/mutation/stm32.json
  python3 tools/mutation/mutate.py --config ... --files src/logic/endstop.cpp --max 200

Config (JSON):
  {
    "root": "software_stm32",               # working dir for commands, relative to repo
    "files": ["lib/logic/src/endstop.cpp"],  # files to mutate (relative to root)
    "build": "pio test -e native --without-uploading --without-testing",  # optional
    "test":  "pio test -e native",           # must exit 0 on success
    "timeout": 120,                          # seconds per test run
    "threshold": 80,                         # minimum mutation score (%) to pass
    "equivalent": {"file.cpp": [12, 40]}     # optional: lines whose mutants are documented as equivalent
  }

Lines can be excluded in source with a trailing  // NOMUTATE  comment
(use sparingly and explain why in the same comment).

Exit code: 0 when score >= threshold, 1 otherwise, 2 on configuration error.
A JSON report is written next to the config (<name>.report.json) and a
Markdown summary to stdout.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import resource
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, asdict

# ---------------------------------------------------------------- operators
# Each operator: (name, regex, list of replacements). Regexes are applied on
# code with strings/comments blanked out, so offsets stay valid.
BINARY_SWAPS = [
    ("rel", r"(?<![<>=!])==(?!=)", ["!="]),
    ("rel", r"!=", ["=="]),
    ("rel", r"(?<![<>=-])<=(?!=)", ["<", ">"]),
    ("rel", r"(?<![<>=-])>=(?!=)", [">", "<"]),
    ("rel", r"(?<![<>=\-])<(?![<=])", ["<=", ">="]),
    ("rel", r"(?<![<>=\-])>(?![>=])", [">=", "<="]),
    ("log", r"&&", ["||"]),
    ("log", r"\|\|", ["&&"]),
    ("arith", r"(?<![+])\+(?![+=])", ["-"]),
    ("arith", r"(?<![-])-(?![-=>])", ["+"]),
    ("arith", r"(?<![*/])\*(?![=/])", ["/"]),
    ("arith", r"(?<![/*])/(?![=/*])", ["*"]),
    ("arith", r"%(?!=)", ["*"]),
    ("asgn", r"\+=", ["-="]),
    ("asgn", r"-=", ["+="]),
    ("incdec", r"\+\+", ["--"]),
    ("incdec", r"--", ["++"]),
    ("bool", r"\btrue\b", ["false"]),
    ("bool", r"\bfalse\b", ["true"]),
]
INT_LITERAL = re.compile(r"(?<![\w.])(\d+)(?![\w.])")
NEGATE_IF = re.compile(r"\b(if|while)\s*\(")
RETURN_EXPR = re.compile(r"\breturn\s+([^;]+);")


@dataclass
class Mutant:
    file: str
    line: int
    col: int
    op: str
    original: str
    replacement: str
    status: str = "pending"   # killed | survived | timeout | build_error | equivalent
    seconds: float = 0.0


def blank_strings_and_comments(src: str) -> str:
    """Replace string/char literals and comments with spaces (same length)."""
    out = list(src)
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j == -1 else j
            for k in range(i, j):
                out[k] = " "
            i = j
        elif src.startswith("/*", i):
            j = src.find("*/", i + 2)
            j = n if j == -1 else j + 2
            for k in range(i, j):
                if out[k] != "\n":
                    out[k] = " "
            i = j
        elif c in "\"'":
            j = i + 1
            while j < n and src[j] != c:
                j += 2 if src[j] == "\\" else 1
            for k in range(i, min(j + 1, n)):
                if out[k] != "\n":
                    out[k] = " "
            i = j + 1
        elif c == "#" and (i == 0 or src[i - 1] == "\n"):
            j = src.find("\n", i)       # preprocessor line: never mutate
            j = n if j == -1 else j
            for k in range(i, j):
                out[k] = " "
            i = j
        else:
            i += 1
    return "".join(out)


def line_col(src: str, pos: int) -> tuple[int, int]:
    line = src.count("\n", 0, pos) + 1
    col = pos - (src.rfind("\n", 0, pos) + 1) + 1
    return line, col


def excluded_lines(src: str) -> set[int]:
    return {i + 1 for i, l in enumerate(src.splitlines()) if "NOMUTATE" in l}


def in_template_or_include(clean: str, pos: int) -> bool:
    # skip '<' / '>' that look like template brackets: e.g. std::array<uint8_t, 4>
    line_start = clean.rfind("\n", 0, pos) + 1
    line_end = clean.find("\n", pos)
    line = clean[line_start: line_end if line_end != -1 else len(clean)]
    return bool(re.search(r"\b(template|static_cast|reinterpret_cast|const_cast|std::\w+|array|vector)\s*<", line))


def generate(path: str, rel: str) -> list[Mutant]:
    src = open(path, encoding="utf-8").read()
    clean = blank_strings_and_comments(src)
    skip = excluded_lines(src)
    muts: list[Mutant] = []
    seen: set[tuple[int, str, str]] = set()

    def add(pos: int, op: str, orig: str, rep: str):
        line, col = line_col(src, pos)
        if line in skip:
            return
        key = (pos, orig, rep)
        if key in seen:
            return
        seen.add(key)
        muts.append(Mutant(rel, line, col, op, orig, rep))

    for op, pattern, reps in BINARY_SWAPS:
        for m in re.finditer(pattern, clean):
            if op == "rel" and m.group(0) in "<>" and in_template_or_include(clean, m.start()):
                continue
            if op == "arith" and m.group(0) == "-" and re.match(r"\s*[\d(]", clean[m.end():]) \
                    and re.search(r"[=(,\[{?:]\s*$|return\s*$", clean[:m.start()]):
                # unary minus on a literal: handled by literal operator instead
                continue
            if op == "arith" and m.group(0) == "*" and re.search(r"[\w\)\]]\s*$", clean[:m.start()]) is None:
                continue  # pointer deref / declaration, not multiplication
            for r in reps:
                add(m.start(), op, m.group(0), r)

    for m in INT_LITERAL.finditer(clean):
        v = int(m.group(1))
        for r in {v + 1, max(v - 1, 0), 0 if v else 1}:
            if r != v:
                add(m.start(), "const", m.group(1), str(r))

    for m in NEGATE_IF.finditer(clean):
        add(m.end() - 1, "negcond", "(", "(!")  # if (x) -> if (!x) needs matching ")"
    for m in RETURN_EXPR.finditer(clean):
        expr = m.group(1).strip()
        if expr in ("true", "false", "0", "1", "nullptr"):
            continue
        add(m.start(1), "retval", m.group(1), "0")

    muts.sort(key=lambda x: (x.line, x.col, x.op, x.replacement))
    return muts


def apply(src: str, mut: Mutant) -> str | None:
    lines = src.splitlines(keepends=True)
    idx = mut.line - 1
    line = lines[idx]
    c = mut.col - 1
    if line[c:c + len(mut.original)] != mut.original:
        return None
    if mut.op == "negcond":
        # wrap whole condition: find matching ')' across the rest of the file
        head = "".join(lines[:idx]) + line[:c]
        rest = line[c:] + "".join(lines[idx + 1:])
        depth, j = 0, 0
        for j, ch in enumerate(rest):
            if ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
                if depth == 0:
                    break
        return head + "(!(" + rest[1:j] + "))" + rest[j + 1:]
    lines[idx] = line[:c] + mut.replacement + line[c + len(mut.original):]
    return "".join(lines)


def run(cmd: str, cwd: str, timeout: int) -> tuple[int, str]:
    """Run a shell command in its own process group; on timeout kill the whole group
    (a mutant that loops forever must not survive as an orphaned test process)."""
    def limit_child():
        # Belt and braces: even if the group kill below never runs (runner killed, CI cancelled),
        # the kernel stops any process of the test command after a hard CPU budget.
        cpu = max(timeout * 2, 30)
        resource.setrlimit(resource.RLIMIT_CPU, (cpu, cpu + 5))

    p = subprocess.Popen(cmd, shell=True, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                         text=True, errors="replace", start_new_session=True,
                         preexec_fn=limit_child)
    try:
        out, _ = p.communicate(timeout=timeout)
        return p.returncode, (out or "")[-4000:]
    except subprocess.TimeoutExpired:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        p.communicate()
        return -999, "timeout"


def fmt(cmd: str, build: str, stem: str = "") -> str:
    return cmd.replace("{build}", build).replace("{stem}", stem)


def evaluate(m: Mutant, root: str, build: str, cfg: dict, timeout: int, source: str) -> None:
    """Apply one mutant in `root`, classify it, restore the file. Runs in a worker thread."""
    p = os.path.join(root, m.file)
    mutated = apply(source, m)
    if mutated is None:
        m.status = "equivalent"
        return
    t0 = time.time()
    try:
        open(p, "w", encoding="utf-8").write(mutated)
        if cfg.get("build"):
            bc, _ = run(fmt(cfg["build"], build), root, timeout)
            if bc != 0:
                m.status = "build_error"   # does not compile: killed by the compiler
                return
        stem = os.path.splitext(os.path.basename(m.file))[0]
        if cfg.get("test_file"):
            tc, _ = run(fmt(cfg["test_file"], build, stem), root, timeout)
            if tc == -999:
                m.status = "timeout"
                return
            if tc != 0:
                m.status = "killed"
                return
        tc, _ = run(fmt(cfg["test"], build, stem), root, timeout)
        m.status = "timeout" if tc == -999 else ("survived" if tc == 0 else "killed")
    finally:
        open(p, "w", encoding="utf-8").write(source)
        m.seconds = round(time.time() - t0, 2)


def make_worker(root: str, idx: int, tmp: str) -> str:
    if idx == 0 and not tmp:
        return root
    dst = os.path.join(tmp, f"w{idx}", os.path.basename(root.rstrip("/")))
    shutil.copytree(root, dst, symlinks=True,
                    ignore=shutil.ignore_patterns(".pio", "build", ".git", "node_modules"))
    return dst


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--files", nargs="*", help="limit to these files (relative to root)")
    ap.add_argument("--max", type=int, default=0, help="limit number of mutants (0 = all)")
    ap.add_argument("--jobs", type=int, default=1, help="parallel workers (each on its own copy of root)")
    ap.add_argument("--list", action="store_true", help="only list mutants")
    ap.add_argument("--changed-since", metavar="GIT_REV",
                    help="print the configured files changed since GIT_REV (space separated) and exit")
    args = ap.parse_args()

    cfg_path = os.path.abspath(args.config)
    cfg = json.load(open(cfg_path))
    repo = os.path.abspath(os.path.join(os.path.dirname(cfg_path), cfg.get("repo", "../..")))
    root = os.path.join(repo, cfg["root"])
    files = args.files or cfg["files"]
    if args.changed_since:
        p = subprocess.run(["git", "diff", "--name-only", args.changed_since, "--", "."],
                           cwd=root, capture_output=True, text=True)
        if p.returncode != 0:
            print(p.stderr, file=sys.stderr)
            return 2
        changed = {os.path.relpath(os.path.join(repo, l.strip()), root) for l in p.stdout.splitlines() if l.strip()}
        print(" ".join(f for f in cfg["files"] if f in changed))
        return 0
    timeout = int(cfg.get("timeout", 120))
    threshold = float(cfg.get("threshold", 80))
    file_threshold = float(cfg.get("file_threshold", 0))
    equivalent = {k: {e if isinstance(e, int) else e["line"] for e in v} for k, v in cfg.get("equivalent", {}).items()}

    all_muts: list[Mutant] = []
    sources: dict[str, str] = {}
    for rel in files:
        p = os.path.join(root, rel)
        if not os.path.isfile(p):
            print(f"config error: {p} not found", file=sys.stderr)
            return 2
        sources[rel] = open(p, encoding="utf-8").read()
        all_muts += generate(p, rel)
    if args.max:
        all_muts = all_muts[: args.max]
    for m in all_muts:
        if m.line in equivalent.get(m.file, set()):
            m.status = "equivalent"
    if args.list:
        for m in all_muts:
            print(f"{m.file}:{m.line}:{m.col} {m.op} {m.original!r} -> {m.replacement!r}")
        print(f"{len(all_muts)} mutants")
        return 0

    jobs = max(1, args.jobs)
    tmp = tempfile.mkdtemp(prefix="mutate-") if jobs > 1 else ""
    workers = []
    try:
        for i in range(jobs):
            wroot = make_worker(root, i, tmp)
            build = os.path.join(tmp or os.path.join(root, "build"), f"mutation-{i}")
            if cfg.get("setup"):
                sc, out = run(fmt(cfg["setup"], build), wroot, timeout * 5)
                if sc != 0:
                    print("setup failed\n" + out, file=sys.stderr)
                    return 2
            if cfg.get("build"):
                bc, out = run(fmt(cfg["build"], build), wroot, timeout * 5)
                if bc != 0:
                    print("baseline build fails\n" + out, file=sys.stderr)
                    return 2
            tc, out = run(fmt(cfg["test"], build), wroot, timeout * 3)
            if tc != 0:
                print("baseline test run fails; fix tests before mutation testing\n" + out, file=sys.stderr)
                return 2
            workers.append((wroot, build))

        todo = [m for m in all_muts if m.status == "pending"]
        lock = threading.Lock()
        done = [0]
        stop = threading.Event()

        def work(wroot: str, build: str) -> None:
            while not stop.is_set():
                with lock:
                    if not todo:
                        return
                    m = todo.pop(0)
                try:
                    evaluate(m, wroot, build, cfg, timeout, sources[m.file])
                except Exception as exc:  # never let one mutant kill a worker thread
                    m.status = "error"
                    print(f"error evaluating {m.file}:{m.line}: {exc!r}", file=sys.stderr, flush=True)
                with lock:
                    done[0] += 1
                    if done[0] % 25 == 0 or m.status == "survived":
                        print(f"[{done[0]}/{len(all_muts)}] {m.status:9} {m.file}:{m.line} {m.op} "
                              f"{m.original!r}->{m.replacement!r}", flush=True)

        def on_signal(signum, _frame):
            stop.set()
            for rel, text in sources.items():
                open(os.path.join(root, rel), "w", encoding="utf-8").write(text)
            sys.exit(128 + signum)

        signal.signal(signal.SIGTERM, on_signal)
        signal.signal(signal.SIGINT, on_signal)
        threads = [threading.Thread(target=work, args=w, daemon=True) for w in workers]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
    finally:
        for rel, text in sources.items():
            open(os.path.join(root, rel), "w", encoding="utf-8").write(text)
        if tmp:
            shutil.rmtree(tmp, ignore_errors=True)

    KILLED = ("killed", "timeout", "build_error")
    not_run = [m for m in all_muts if m.status in ("pending", "error")]
    counted = [m for m in all_muts if m.status != "equivalent"]
    killed = sum(m.status in KILLED for m in counted)
    score = 100.0 * killed / len(counted) if counted else 100.0
    report = {"score": round(score, 2), "threshold": threshold, "file_threshold": file_threshold,
              "total": len(all_muts), "killed": killed,
              "survived": sum(m.status == "survived" for m in counted),
              "equivalent": len(all_muts) - len(counted), "mutants": [asdict(m) for m in all_muts]}
    rpt = os.path.splitext(cfg_path)[0] + ".report.json"
    json.dump(report, open(rpt, "w"), indent=1)

    print(f"\n## Mutation score: {score:.1f}% (threshold {threshold}%)")
    print(f"killed {killed} / {len(counted)} counted, equivalent {report['equivalent']}")
    per_file: dict[str, list[int]] = {}
    for m in counted:
        k = per_file.setdefault(m.file, [0, 0])
        k[1] += 1
        k[0] += m.status in KILLED
    failing_files = []
    print("\n| file | killed/total | score |\n|---|---|---|")
    for f, (k, t) in sorted(per_file.items()):
        fs = 100.0 * k / t
        if fs < file_threshold:
            failing_files.append(f)
        print(f"| {f} | {k}/{t} | {fs:.1f}% |")
    surv = [m for m in counted if m.status == "survived"]
    if surv:
        print("\n### Surviving mutants")
        for m in surv:
            print(f"- {m.file}:{m.line}:{m.col} `{m.original}` -> `{m.replacement}` ({m.op})")
    if failing_files:
        print("\nfiles below the per-file threshold: " + ", ".join(failing_files))
    if not_run:
        print(f"\nERROR: {len(not_run)} mutants were not evaluated (status pending/error); the score is not valid.")
        return 2
    return 0 if score >= threshold and not failing_files else 1


if __name__ == "__main__":
    sys.exit(main())
