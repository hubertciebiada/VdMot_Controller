#!/usr/bin/env python3
"""Per-file mutation gate over a cargo-mutants run (tools/rust/docker.sh mutate).

Score of a file = killed / counted, where killed = caught + timeout and counted = every mutant
of the file except unviable ones (they do not compile, like the stillborn mutants of the C++
gate) and the documented equivalent ones. The gate passes when the overall score and the score
of every file reach the thresholds (95 % each, as for the C++ suites).

Equivalent mutants are listed in tools/rust/mutation/equivalents/<package>.json or, one file per
module, in tools/rust/mutation/equivalents/<package>/<module>.json:
    [{"file": "core/src/common.rs", "function": "elapsed_ms",
      "mutation": "replace elapsed_ms -> u32 with 0", "reason": "..."}]
"file" is relative to the workspace root, as cargo-mutants names it. "function" is "" for a
mutant outside any function (cargo-mutants also mutates the expressions of const items).
"mutation" is the cargo-mutants name without its "file:line:col: " prefix, so an entry survives
edits that move the code. Every entry needs a reason. An entry of a file in the run that
matches no mutant is reported as stale (and fails the gate, so the list cannot rot); entries of
files outside a partial run (--file) are not checked by that run.

Writes tools/rust/mutation/<package>.report.json and prints the per-file table and the
surviving mutants.

--outcomes takes several files: the shards of one package (cargo mutants --shard k/n, the CI
runs) are gated together, as one run.
"""
import argparse
import json
import os
import re
import sys

KILLED = {"CaughtMutant", "Timeout"}
SURVIVED = {"MissedMutant"}
UNVIABLE = {"Unviable"}
NAME_PREFIX = re.compile(r"^[^:]+:\d+:\d+: ")


def load_equivalents(path):
    if not os.path.exists(path):
        return []
    with open(path, encoding="utf-8") as f:
        entries = json.load(f)
    for e in entries:
        missing = [k for k in ("file", "mutation", "reason") if not e.get(k)]
        if not isinstance(e.get("function"), str):
            missing.append("function")
        if missing:
            sys.exit("%s: entry %r lacks %s" % (path, e, ", ".join(missing)))
    return entries


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--workspace", required=True)
    ap.add_argument("--package", required=True)
    ap.add_argument("--outcomes", required=True, nargs="+")
    ap.add_argument("--threshold", type=float, default=95.0)
    ap.add_argument("--file-threshold", type=float, default=95.0)
    ap.add_argument("--repo", default=os.path.join(os.path.dirname(__file__), "..", ".."))
    args = ap.parse_args()

    run = {"outcomes": []}
    for path in args.outcomes:
        if not os.path.exists(path):
            # cargo mutants writes no outcomes when the filters (--file, --exclude) match no mutant
            sys.exit("%s not found: no mutants were tested (a --file glob with '/' matches from the "
                     "workspace root, e.g. core/src/x.rs)" % path)
        with open(path, encoding="utf-8") as f:
            part = json.load(f)
        run["outcomes"] += part["outcomes"]
        run.setdefault("cargo_mutants_version", part.get("cargo_mutants_version"))
    # one list per package and/or one per module (tools/rust/mutation/equivalents/<package>/*.json),
    # so parallel ports never edit the same file
    eq_root = os.path.join(args.repo, "tools", "rust", "mutation", "equivalents")
    equivalents = load_equivalents(os.path.join(eq_root, args.package + ".json"))
    eq_dir = os.path.join(eq_root, args.package)
    if os.path.isdir(eq_dir):
        for name in sorted(os.listdir(eq_dir)):
            if name.endswith(".json"):
                equivalents += load_equivalents(os.path.join(eq_dir, name))
    eq_hits = [0] * len(equivalents)

    files = {}
    for o in run["outcomes"]:
        m = o["scenario"].get("Mutant") if isinstance(o["scenario"], dict) else None
        if not m:
            continue  # the baseline
        fname = m["file"]
        func = (m.get("function") or {}).get("function_name", "")
        desc = NAME_PREFIX.sub("", m["name"])
        st = files.setdefault(fname, {"killed": 0, "survived": 0, "unviable": 0, "equivalent": 0,
                                      "timeout": 0, "missed": []})
        summary = o["summary"]
        if summary in UNVIABLE:
            st["unviable"] += 1
            continue
        eq = None
        for i, e in enumerate(equivalents):
            if e["file"] == fname and e["function"] == func and e["mutation"] == desc:
                eq = i
                break
        if eq is not None:
            eq_hits[eq] += 1
            st["equivalent"] += 1
            continue
        if summary in KILLED:
            st["killed"] += 1
            if summary == "Timeout":
                st["timeout"] += 1
        elif summary in SURVIVED:
            st["survived"] += 1
            st["missed"].append(m["name"])
        else:
            sys.exit("unknown outcome %r for %s" % (summary, m["name"]))

    stale = [equivalents[i] for i, n in enumerate(eq_hits) if n == 0 and equivalents[i]["file"] in files]
    total_k = sum(s["killed"] for s in files.values())
    total_c = sum(s["killed"] + s["survived"] for s in files.values())
    overall = 100.0 * total_k / total_c if total_c else None

    ok = True
    print("| file | killed | survived | equivalent | unviable | score |")
    print("|---|---|---|---|---|---|")
    report_files = {}
    for fname in sorted(files):
        s = files[fname]
        counted = s["killed"] + s["survived"]
        score = 100.0 * s["killed"] / counted if counted else None
        if score is not None and score < args.file_threshold:
            ok = False
        print("| %s | %d | %d | %d | %d | %s |" % (
            fname, s["killed"], s["survived"], s["equivalent"], s["unviable"],
            "-" if score is None else "%.1f %%" % score))
        report_files[fname] = dict(s, score=score)
    if overall is not None and overall < args.threshold:
        ok = False
    print()
    print("overall: %s (%d of %d counted mutants killed)" % (
        "-" if overall is None else "%.2f %%" % overall, total_k, total_c))
    for fname in sorted(files):
        for name in files[fname]["missed"]:
            print("SURVIVED %s" % name)
    for e in stale:
        ok = False
        print("STALE equivalent (matches no mutant): %s %s %s" % (e["file"], e["function"], e["mutation"]))

    out = os.path.join(args.repo, "tools", "rust", "mutation", args.package + ".report.json")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        json.dump({"workspace": args.workspace, "package": args.package, "score": overall,
                   "threshold": args.threshold, "file_threshold": args.file_threshold,
                   "cargo_mutants_version": run.get("cargo_mutants_version"),
                   "passed": ok, "files": report_files}, f, indent=1, sort_keys=True)
        f.write("\n")
    print("gate: %s" % ("passed" if ok else "FAILED"))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
