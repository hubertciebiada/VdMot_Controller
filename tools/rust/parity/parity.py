#!/usr/bin/env python3
"""Test parity of the Rust port: every C++ test case -> its Rust test(s) or a documented reason.

Extracts the doctest cases of the four C++ native suites (TEST_CASE and their SUBCASEs):

    software_esp32_revamped/test/native/test_*.cpp          esp-core  -> software_esp32_rust/core
    software_esp32_revamped/test/native/glue/*.cpp          esp-glue  -> software_esp32_rust/glue
    software_stm32/test/native/test_*.cpp                   stm-core  -> software_stm32_rust/core
    software_stm32/test/native/glue/test_*.cpp              stm-glue  -> software_stm32_rust/glue, boot

and the `#[test]` functions of the Rust crates, then matches them by the naming convention of
the ports (docs/rust/PORTING.md "Modules and tests", GLUE-DESIGN-*.md "Suites"):

    test_<m>.cpp -> <crate>/src/<m>/tests.rs, test_<m>__<p>.cpp / test_<m>_<p>.cpp -> tests_<p>.rs
    "<module> <topic>: <text>" -> fn <topic>_<text> (or <text>), snake_case, camelCase split

Match kinds, in the order they are tried (each C++ case keeps the first that applies):

    manual   a row of tools/rust/parity/manual.tsv (hand review; wins over everything)
    hint     a Rust comment `C++ "<case name>"` (or the start of it) in or right above a test
    exact    a normalised name of the case equals the Rust test name (the mapped files first,
             then the whole crate)
    prefix   the Rust name is the start of a normalised case name (at least 5 words, unique)
    subcase  every SUBCASE of the case has a Rust test of its name ("one test per SUBCASE")
    fuzzy    the best token-sequence similarity >= 0.72 inside the mapped files, one Rust test
             per C++ case (reviewed by hand: a wrong pair goes to manual.tsv)

A case without a match is "unmatched" and must get a manual.tsv row: `rust` (the Rust tests),
`noform` (no Rust form; the reason is a PORT-NOTES entry) or `retired` (a retired case of the
glue design) or `missing` (a gap).

manual.tsv (tab separated, '#' comments):  <cpp file> TAB <case name> TAB <kind> TAB <target>
    kind rust:     target = space separated Rust tests as <file>::<fn> (file relative to the repo)
    kind noform:   target = the PORT-NOTES / design anchor, e.g. docs/rust/PORT-NOTES.md#ota,
                   and the Rust tests that stand in for the subject (checked like `rust`)
    kind retired:  target = the design anchor that retires the case (and its replacement)
    kind missing:  target = what is missing

The run also checks that every test (`esp:<file>::<fn>`, `stm:<file>::<fn>`) and file named in
docs/rust/PARITY.md exists.

Usage (repo root):
    python tools/rust/parity/parity.py              summary per file; exit 1 on a gap, a stale
                                                    manual row or a name of PARITY.md not found
    python tools/rust/parity/parity.py --unmatched  the cases without a match
    python tools/rust/parity/parity.py --fuzzy      the fuzzy pairs (to review)
    python tools/rust/parity/parity.py --write      docs/rust/PARITY-TESTS.md and the summary
                                                    block of docs/rust/PARITY.md
    python tools/rust/parity/parity.py --check      both up to date (CI)
"""
import argparse
import difflib
import os
import re
import sys
from collections import Counter, defaultdict

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".."))
MANUAL = os.path.join(os.path.dirname(os.path.abspath(__file__)), "manual.tsv")

SUITES = [
    # (suite, C++ dir, file glob regex, Rust crate dirs (repo relative))
    ("esp-core", "software_esp32_revamped/test/native", r"^test_.*\.cpp$",
     ["software_esp32_rust/core"]),
    ("esp-glue", "software_esp32_revamped/test/native/glue", r"^(test_.*|selftest)\.cpp$",
     ["software_esp32_rust/glue"]),
    ("stm-core", "software_stm32/test/native", r"^test_.*\.cpp$",
     ["software_stm32_rust/core"]),
    ("stm-glue", "software_stm32/test/native/glue", r"^test_.*\.cpp$",
     ["software_stm32_rust/glue", "software_stm32_rust/boot"]),
]

RUST_CRATES = [
    "software_esp32_rust/core", "software_esp32_rust/glue",
    "software_stm32_rust/core", "software_stm32_rust/glue", "software_stm32_rust/boot",
    "software_stm32_rust/image-check",
]

# C++ glue stems whose Rust module has another name (GLUE-DESIGN-ESP.md 5.2, GLUE-DESIGN-STM.md 7.1)
STEM_TO_MODULE = {
    ("stm-glue", "owDevices"): ["software_stm32_rust/glue/src/ow_devices"],
    ("stm-glue", "main"): ["software_stm32_rust/glue/src/main_loop"],
    ("stm-glue", "otasupport"): ["software_stm32_rust/boot/src/window"],
    ("stm-glue", "sysstat"): ["software_stm32_rust/boot/src/capture",
                              "software_stm32_rust/glue/src/sysstat"],
    ("stm-glue", "fakes"): ["software_stm32_rust/glue/src/test_support",
                            "software_stm32_rust/glue/src/print",
                            "software_stm32_rust/glue/src/serial"],
    ("stm-core", "fuzz"): [f"software_stm32_rust/core/src/{m}" for m in
                           ("arg_parser", "buf_writer", "line_assembler", "replies", "tokenizer")],
    ("stm-glue", "system"): ["software_stm32_rust/glue/src/system"],
    ("esp-glue", "selftest"): ["software_esp32_rust/glue/src/testkit"],
    ("esp-glue", "web_server__eq"): ["software_esp32_rust/glue/src/http_parse"],
}

# the module word(s) a C++ case name starts with, dropped for the Rust name
MODULE_WORDS = {"web", "mqtt", "net", "ota", "app", "storage", "logger", "link", "stm", "service",
                "session", "glue", "system", "motor", "comm", "communication", "eeprom",
                "terminal", "owdevices", "ow", "sysstat", "main", "selftest", "fakes", "sim",
                "otasupport", "i2c", "i2c_bus", "calib", "valve", "lease", "store"}


# ---------------------------------------------------------------- C++ extraction

CASE_RE = re.compile(r'^\s*TEST_CASE\(\s*"((?:[^"\\]|\\.)*)"(.*)$')
SUBCASE_RE = re.compile(r'^\s*SUBCASE\(\s*"((?:[^"\\]|\\.)*)"')
SUITE_RE = re.compile(r'doctest::test_suite\("([^"]+)"\)')


def c_unescape(s):
    return re.sub(r'\\(.)', lambda m: {"n": "\n", "t": "\t"}.get(m.group(1), m.group(1)), s)


class CppCase:
    def __init__(self, suite, path, line, name, tags):
        self.suite, self.path, self.line, self.name, self.tags = suite, path, line, name, tags
        self.subcases = []
        self.kind = None      # manual-rust, hint, exact, subcase, fuzzy, noform, retired, missing
        self.targets = []     # RustTest objects (or text for noform/retired/missing)
        self.note = ""
        self.score = 0.0

    @property
    def key(self):
        return (self.path, self.name)


def cpp_cases():
    cases = []
    for suite, rel, pattern, _crates in SUITES:
        d = os.path.join(ROOT, rel)
        for fn in sorted(os.listdir(d)):
            if not re.match(pattern, fn):
                continue
            path = f"{rel}/{fn}"
            with open(os.path.join(d, fn), encoding="utf-8") as f:
                lines = f.read().split("\n")
            cur = None
            for i, line in enumerate(lines, 1):
                m = CASE_RE.match(line)
                if m:
                    tail = m.group(2) + " " + (lines[i] if i < len(lines) else "")
                    cur = CppCase(suite, path, i, c_unescape(m.group(1)), SUITE_RE.findall(tail))
                    cases.append(cur)
                    continue
                m = SUBCASE_RE.match(line)
                if m and cur is not None:
                    cur.subcases.append(c_unescape(m.group(1)))
    return cases


# ---------------------------------------------------------------- Rust extraction

class RustTest:
    def __init__(self, path, line, name, hints, attrs):
        self.path, self.line, self.name, self.hints, self.attrs = path, line, name, hints, attrs
        self.used_by = []

    @property
    def ref(self):
        return f"{self.path}::{self.name}"


FN_RE = re.compile(r'^\s*(?:pub\s+)?fn\s+([A-Za-z0-9_]+)\s*\(')
HINT_RE = re.compile(r'C\+\+[^"]{0,40}"([^"]{3,})"')


def comment_text(lines):
    """Joins the `//` comment lines of a region into one text per comment block."""
    blocks, cur = [], []
    for line in lines:
        s = line.strip()
        m = re.match(r'^(?://[/!]?)\s?(.*)$', s)
        if m:
            cur.append(m.group(1))
        else:
            # a trailing comment on a code line
            t = re.search(r'//\s?(.*)$', s)
            if t:
                cur.append(t.group(1))
            if cur:
                blocks.append(" ".join(cur))
                cur = []
    if cur:
        blocks.append(" ".join(cur))
    return blocks


def rust_tests():
    tests = []
    for crate in RUST_CRATES:
        base = os.path.join(ROOT, crate)
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = [d for d in dirnames if d not in ("target", "images")]
            for fn in sorted(filenames):
                if not fn.endswith(".rs"):
                    continue
                full = os.path.join(dirpath, fn)
                path = os.path.relpath(full, ROOT).replace(os.sep, "/")
                with open(full, encoding="utf-8") as f:
                    lines = f.read().split("\n")
                starts = [i for i, l in enumerate(lines) if l.strip() == "#[test]"]
                for n, s in enumerate(starts):
                    attrs = []
                    j = s + 1
                    while j < len(lines) and not FN_RE.match(lines[j]):
                        t = lines[j].strip()
                        if t.startswith("#["):
                            attrs.append(t)
                        j += 1
                    if j >= len(lines):
                        continue
                    name = FN_RE.match(lines[j]).group(1)
                    # hint region: the comment block right above #[test] and the body
                    k = s - 1
                    while k >= 0 and (lines[k].strip().startswith("//") or lines[k].strip().startswith("#[")):
                        k -= 1
                    end = starts[n + 1] if n + 1 < len(starts) else len(lines)
                    # the body ends at the first `}` in column 0 (rustfmt), comments between two
                    # tests belong to neither
                    for e in range(j + 1, end):
                        if lines[e].startswith("}"):
                            end = e + 1
                            break
                    region = lines[k + 1:end]
                    hints = []
                    for block in comment_text(region):
                        hints.extend(HINT_RE.findall(block))
                    tests.append(RustTest(path, j + 1, name, hints, attrs))
    return tests


# ---------------------------------------------------------------- names

WORDS = {"IPv4": "ipv4", "WiFi": "wifi", "ETag": "etag", "UTF-8": "utf8", "CRC-32": "crc32",
         "CRC-8": "crc8", "mDNS": "mdns", "NaN": "nan", "MiB": "mib", "KiB": "kib"}


def snake(s):
    for k, v in WORDS.items():
        s = s.replace(k, f" {v} ")
    s = re.sub(r'U\+([0-9A-Fa-f]{4})', lambda m: " u" + m.group(1).lower() + " ", s)
    s = s.replace("..", " to ")
    s = s.replace("->", " ").replace("=>", " ")
    s = re.sub(r'(^|[\s(\[,])-(?=\d)', r'\1 minus ', s)
    s = s.replace("%", " percent ").replace("°C", " c ").replace("+", " plus ")
    s = re.sub(r'\bk([A-Z][a-z])', r'\1', s)          # kItemNameMax -> ItemNameMax
    s = re.sub(r'\b([A-Z]{2,})s\b', lambda m: m.group(1).lower() + "s", s)   # NACKs -> nacks
    s = re.sub(r'\b0x([0-9A-Fa-f]+)', lambda m: "0x" + m.group(1).lower(), s)
    s = re.sub(r'([a-z])([A-Z])', r'\1_\2', s)
    s = re.sub(r'([0-9])([A-Z][a-z])', r'\1_\2', s)
    s = re.sub(r'([A-Z]+)([A-Z][a-z])', r'\1_\2', s)
    s = s.lower()
    s = re.sub(r'[^a-z0-9]+', '_', s).strip('_')
    return s


def name_variants(name):
    """Normalised forms of a C++ case name that a Rust port may have used."""
    out = []
    # without a trailing tag of the review lists ("(C-7)", "(W18-2, W-4)")
    bare = re.sub(r'\s*\([A-Z][A-Za-z]*\d*-?\d*[a-z]?(?:,\s*[A-Z][A-Za-z]*\d*-?\d*[a-z]?)*\)\s*$', '', name)
    if bare != name:
        out.extend(name_variants(bare))
    if "->" in name:
        out.extend(name_variants(name.replace("->", " to ")))
    full = snake(name)
    out.append(full)
    if ":" in name:
        prefix, rest = name.split(":", 1)
        r = snake(rest)
        pw = snake(prefix).split("_")
        out.append(r)
        # drop the leading module word(s) of the prefix, keep the topic
        for cut in range(1, len(pw)):
            out.append("_".join(pw[cut:] + ([r] if r else [])))
        # topic words without the module word in front ("app setup" -> "setup")
        topic = [w for w in pw if w not in MODULE_WORDS]
        if topic:
            out.append("_".join(topic + ([r] if r else [])))
    seen, res = set(), []
    for v in out:
        v = v.strip("_")
        if v and v not in seen:
            seen.add(v)
            res.append(v)
    return res


RUST_SUFFIXES = ("_cases", "_case", "_c1", "_c2")


def rust_forms(name):
    forms = {name}
    for suf in RUST_SUFFIXES:
        if name.endswith(suf):
            forms.add(name[: -len(suf)])
    return forms


def tokens(s):
    return [t for t in s.split("_") if t]


def similarity(a, b):
    ta, tb = tokens(a), tokens(b)
    if not ta or not tb:
        return 0.0
    return difflib.SequenceMatcher(None, ta, tb, autojunk=False).ratio()


# ---------------------------------------------------------------- file mapping

def mapped_dirs(case):
    """The Rust module directories of a C++ test file (the convention, then the exceptions)."""
    fn = os.path.basename(case.path)
    stem = fn[:-4]
    suite = case.suite
    crate = {s: c for s, _, _, c in SUITES}[suite]
    if stem.startswith("test_"):
        stem = stem[5:]
    if (suite, stem) in STEM_TO_MODULE:
        return STEM_TO_MODULE[(suite, stem)], None
    module, part = stem, None
    if "__" in stem:
        module, part = stem.split("__", 1)
    for prefix, p in (("system_", None),):
        if module.startswith(prefix) and suite == "stm-glue":
            return ["software_stm32_rust/glue/src/system"], module[len(prefix):]
    key = (suite, module)
    if key in STEM_TO_MODULE:
        return STEM_TO_MODULE[key], part
    dirs = []
    for c in crate:
        d = f"{c}/src/{module}"
        if os.path.isdir(os.path.join(ROOT, d)):
            dirs.append(d)
    if not dirs:
        # test_<module>_<topic>.cpp (glue) -> <module>/tests_<topic>.rs
        bits = module.split("_")
        for cut in range(len(bits) - 1, 0, -1):
            m2, topic = "_".join(bits[:cut]), "_".join(bits[cut:])
            if (suite, m2) in STEM_TO_MODULE:
                return STEM_TO_MODULE[(suite, m2)], topic
            for c in crate:
                d = f"{c}/src/{m2}"
                if os.path.isdir(os.path.join(ROOT, d)):
                    dirs.append(d)
            if dirs:
                return dirs, topic
    return dirs, part


def crate_of(case):
    return {s: c for s, _, _, c in SUITES}[case.suite]


# ---------------------------------------------------------------- matching

def load_manual():
    rows = {}
    if not os.path.exists(MANUAL):
        return rows
    with open(MANUAL, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            line = line.rstrip("\n")
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) < 4:
                sys.exit(f"manual.tsv:{n}: 4 tab separated fields expected")
            path, name, kind, target = parts[0], parts[1], parts[2], "\t".join(parts[3:])
            if kind not in ("rust", "noform", "retired", "missing"):
                sys.exit(f"manual.tsv:{n}: unknown kind {kind}")
            rows[(path, name)] = (kind, target, n)
    return rows


def match(cases, tests):
    by_name = defaultdict(list)
    for t in tests:
        by_name[t.name].append(t)
    by_ref = {t.ref: t for t in tests}
    manual = load_manual()
    used_manual = set()

    def in_dirs(t, dirs):
        return any(t.path.startswith(d + "/") or t.path == d + ".rs" for d in dirs)

    def in_crates(t, crates):
        return any(t.path.startswith(c + "/") for c in crates)

    # 1. manual
    for c in cases:
        row = manual.get(c.key)
        if not row:
            continue
        used_manual.add(c.key)
        kind, target, n = row
        if kind == "rust":
            refs = target.split()
            ts = []
            for r in refs:
                t = by_ref.get(r)
                if t is None:
                    sys.exit(f"manual.tsv:{n}: unknown Rust test {r}")
                ts.append(t)
            c.kind, c.targets = "manual", ts
        else:
            # the tests named next to a reason must exist too
            for r in re.findall(r'\S+\.rs::[A-Za-z0-9_]+', target):
                if r not in by_ref:
                    sys.exit(f"manual.tsv:{n}: unknown Rust test {r}")
            c.kind, c.targets = kind, [target]
    stale = [k for k in manual if k not in used_manual]

    # 2. hints: a Rust comment that quotes the case name (full, or without the module prefix)
    hint_index = defaultdict(list)
    for t in tests:
        for h in t.hints:
            hint_index[h.strip()].append(t)
    for c in cases:
        if c.kind:
            continue
        rest = c.name.split(":", 1)[1].strip() if ":" in c.name else None
        hits = []
        for key in (c.name, rest):
            # a short quote ("fuzz") is a word, not a case name
            if key and len(key) >= 12:
                hits += [t for t in hint_index.get(key, []) if in_crates(t, crate_of(c))]
        if hits:
            c.kind, c.targets = "hint", list(dict.fromkeys(hits))
    # a shortened quote: the start of exactly one case name of the crate (at least 20 characters)
    def low(s):
        return re.sub(r"\s+", " ", s.strip().lower())
    for h, ts in hint_index.items():
        if len(h) < 20:
            continue
        hl = low(h)
        for crates in {tuple(crate_of(c)) for c in cases}:
            tts = [t for t in ts if in_crates(t, crates)]
            if not tts:
                continue
            owners = [c for c in cases if tuple(crate_of(c)) == crates and
                      (low(c.name).startswith(hl) or
                       (":" in c.name and low(c.name.split(":", 1)[1]).startswith(hl)))]
            if len(owners) == 1 and not owners[0].kind:
                owners[0].kind, owners[0].targets = "hint", tts

    # 3. exact names (mapped files first, then the crate)
    for c in cases:
        if c.kind:
            continue
        dirs, part = mapped_dirs(c)
        variants = name_variants(c.name)
        cands_local, cands_crate = [], []
        for t in tests:
            if not in_crates(t, crate_of(c)):
                continue
            forms = rust_forms(t.name)
            if any(v in forms for v in variants):
                (cands_local if in_dirs(t, dirs) else cands_crate).append(t)
        cands = cands_local or cands_crate
        if cands:
            # prefer the file of the part (tests_<part>.rs)
            if part and len(cands) > 1:
                pref = [t for t in cands if os.path.basename(t.path) == f"tests_{part}.rs"]
                cands = pref or cands
            c.kind, c.targets = "exact", cands[:1] if len(cands) > 1 and part else cands

    # 3b. a shortened name: the Rust name is the start of a normalised C++ name (>= 5 words),
    # in the mapped files, unique
    for c in cases:
        if c.kind:
            continue
        dirs, part = mapped_dirs(c)
        variants = name_variants(c.name)
        cands = []
        for t in tests:
            if not in_dirs(t, dirs) or len(tokens(t.name)) < 5:
                continue
            if any(v.startswith(t.name + "_") for v in variants):
                cands.append(t)
        if len(cands) == 1:
            c.kind, c.targets = "prefix", cands

    # 4. one Rust test per SUBCASE
    for c in cases:
        if c.kind or not c.subcases:
            continue
        dirs, _ = mapped_dirs(c)
        found = []
        for s in c.subcases:
            vs = set(name_variants(s)) | {snake(s)}
            local = [t for t in tests if in_dirs(t, dirs)]
            hit = [t for t in local if rust_forms(t.name) & vs or any(t.name.endswith("_" + v) for v in vs)]
            if not hit:
                # the Rust name of the subcase with a short topic in front, spelt a little apart
                best = max(((max(similarity(v, "_".join(tokens(t.name)[k:])) for v in vs for k in (0, 1, 2)), t)
                            for t in local), key=lambda p: p[0], default=(0, None))
                if best[0] >= 0.8:
                    hit = [best[1]]
            if len(hit) > 1:
                # "NACK once" of the write case: write_nack_once, not erase_nack_once
                words = set(tokens(snake(c.name)))
                pref = [t for t in hit if tokens(t.name)[0] in words]
                hit = pref or hit
            if not hit:
                found = None
                break
            found.extend(hit)
        if found:
            c.kind, c.targets = "subcase", list(dict.fromkeys(found))

    taken = set()
    for c in cases:
        for t in c.targets:
            if isinstance(t, RustTest):
                taken.add(t.ref)

    # 5. fuzzy inside the mapped files, best pairs first, one Rust test per case
    pairs = []
    for c in cases:
        if c.kind:
            continue
        dirs, part = mapped_dirs(c)
        variants = name_variants(c.name)
        for t in tests:
            if t.ref in taken or not in_dirs(t, dirs):
                continue
            best = max(max(similarity(v, f) for f in rust_forms(t.name)) for v in variants)
            if part and os.path.basename(t.path) == f"tests_{part}.rs":
                best += 0.02
            if best >= 0.72:
                pairs.append((best, c, t))
    pairs.sort(key=lambda p: -p[0])
    for score, c, t in pairs:
        if c.kind or t.ref in taken:
            continue
        c.kind, c.targets, c.score = "fuzzy", [t], score
        taken.add(t.ref)

    for c in cases:
        if not c.kind:
            c.kind = "unmatched"
        for t in c.targets:
            if isinstance(t, RustTest):
                t.used_by.append(c)
    return stale


# ---------------------------------------------------------------- reports

KIND_ORDER = ["manual", "hint", "exact", "prefix", "subcase", "fuzzy", "noform", "retired", "missing", "unmatched"]
PORTED = {"manual", "hint", "exact", "prefix", "subcase", "fuzzy"}
SUITE_ORDER = [s for s, _, _, _ in SUITES]
LISTING = "docs/rust/PARITY-TESTS.md"
SUMMARY_DOC = "docs/rust/PARITY.md"
BEGIN, END = "<!-- parity.py summary begin -->", "<!-- parity.py summary end -->"


def file_order(cases):
    """The C++ files in suite order (ESP core, ESP glue, STM core, STM glue), then by name."""
    suite = {c.path: c.suite for c in cases}
    return sorted(suite, key=lambda p: (SUITE_ORDER.index(suite[p]), p))


def summary(cases, tests):
    per_file = defaultdict(lambda: defaultdict(int))
    for c in cases:
        per_file[c.path][c.kind] += 1
    rows = []
    totals = defaultdict(int)
    for path in file_order(cases):
        k = per_file[path]
        n = sum(k.values())
        ported = sum(k[x] for x in PORTED)
        rows.append((path, n, ported, k["noform"], k["retired"], k["missing"], k["unmatched"]))
        for x, v in k.items():
            totals[x] += v
    return rows, totals


def rust_only(tests):
    return [t for t in tests if not t.used_by]


def short(path):
    """A Rust path relative to its workspace (glue/src/x/tests.rs)."""
    return path.split("/", 1)[1] if path.startswith("software_") else path


def doc_links(text):
    """docs/rust/X.md#a -> a link relative to docs/rust; a Rust test -> code."""
    text = re.sub(r'docs/rust/([A-Za-z0-9_.-]+\.md)(#[A-Za-z0-9_-]+)?',
                  lambda m: f"[{m.group(1)}{m.group(2) or ''}]({m.group(1)}{m.group(2) or ''})", text)
    return re.sub(r'(\S+\.rs)::([A-Za-z0-9_]+)', lambda m: f"`{short(m.group(1))}::{m.group(2)}`", text)


def cell(text):
    return text.replace("|", "\\|")


def listing(cases, tests):
    rows, totals = summary(cases, tests)
    lines = []
    w = lines.append
    w("# Test parity: every C++ case and its Rust tests")
    w("")
    w("Generated by `python tools/rust/parity/parity.py --write`; do not edit. The method, the counts")
    w("per file and the verdict are in [PARITY.md](PARITY.md); the hand-reviewed rows are")
    w("`tools/rust/parity/manual.tsv`. Kinds: `exact` (the Rust name is the C++ name in snake_case),")
    w("`prefix` (the Rust name is its start), `hint` (the Rust test quotes the C++ name), `subcase`")
    w("(one Rust test per SUBCASE), `fuzzy` (the closest name in the mapped file, reviewed),")
    w("`manual` (paired by hand), `noform` (no Rust form, the reason linked), `retired` (retired by")
    w("the glue design, the replacement named). Rust paths are relative to `software_esp32_rust` and")
    w("`software_stm32_rust`.")
    w("")
    w(f"C++ cases: {len(cases)}: " + ", ".join(f"{k} {totals[k]}" for k in KIND_ORDER if totals[k]) + ".")
    w(f"Rust tests: {len(tests)}, of them {len(rust_only(tests))} without a C++ case.")
    w("")
    by_file = defaultdict(list)
    for c in cases:
        by_file[c.path].append(c)
    for path in file_order(cases):
        cs = by_file[path]
        files = Counter(t.path for c in cs if c.kind in PORTED for t in c.targets)
        home = files.most_common(1)[0][0] if files else None
        w(f"## {path} ({len(cs)})")
        w("")
        if home:
            w(f"Rust tests in `{short(home)}` unless a path is given.")
            w("")
        w("| line | C++ case | kind | Rust |")
        w("|---|---|---|---|")
        for c in cs:
            if c.kind in PORTED:
                tgt = "<br>".join(f"`{t.name}`" if t.path == home else f"`{short(t.path)}::{t.name}`"
                                  for t in c.targets)
            else:
                tgt = doc_links(cell(" ".join(str(t) for t in c.targets)))
            name = cell(c.name)
            if c.subcases:
                name += f" ({len(c.subcases)} SUBCASEs)"
            w(f"| {c.line} | {name} | {c.kind} | {tgt} |")
        w("")
    return "\n".join(lines) + "\n"


def summary_block(cases, tests):
    rows, totals = summary(cases, tests)
    lines = [BEGIN, ""]
    w = lines.append
    w("| suite | C++ files | C++ cases | ported | no Rust form | retired | missing |")
    w("|---|---|---|---|---|---|---|")
    for suite in SUITE_ORDER:
        cs = [c for c in cases if c.suite == suite]
        files = len({c.path for c in cs})
        k = defaultdict(int)
        for c in cs:
            k[c.kind] += 1
        ported = sum(k[x] for x in PORTED)
        w(f"| {suite} | {files} | {len(cs)} | {ported} | {k['noform']} | {k['retired']} | "
          f"{k['missing'] + k['unmatched']} |")
    ported = sum(totals[x] for x in PORTED)
    w(f"| all | {len({c.path for c in cases})} | {len(cases)} | {ported} | {totals['noform']} | "
      f"{totals['retired']} | {totals['missing'] + totals['unmatched']} |")
    w("")
    w("Ported, by kind: " + ", ".join(f"{k} {totals[k]}" for k in KIND_ORDER if k in PORTED and totals[k])
      + f". Rust tests: {len(tests)}, {len(rust_only(tests))} of them without a C++ case.")
    w("")
    w("| C++ file | cases | ported | no Rust form | retired | missing |")
    w("|---|---|---|---|---|---|")
    for path, n, p, noform, retired, missing, unmatched in rows:
        head, rest = path.split("/test/native/", 1)
        w(f"| `{rest}` ({head}) | {n} | {p} | {noform} | {retired} | {missing + unmatched} |")
    w("")
    w(END)
    return "\n".join(lines)


def with_summary(doc, block):
    a, b = doc.find(BEGIN), doc.find(END)
    if a < 0 or b < a:
        return None
    return doc[:a] + block + doc[b + len(END):]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--unmatched", action="store_true", help="list the cases without a match")
    ap.add_argument("--fuzzy", action="store_true", help="list the fuzzy pairs")
    ap.add_argument("--kind", help="list the cases of one match kind")
    ap.add_argument("--rust-only", action="store_true", help="list the Rust tests without a C++ case")
    ap.add_argument("--write", action="store_true",
                    help=f"write {LISTING} and the summary block of {SUMMARY_DOC}")
    ap.add_argument("--check", action="store_true", help="fail when either is not up to date")
    a = ap.parse_args()

    cases = cpp_cases()
    tests = rust_tests()
    stale = match(cases, tests)

    if a.unmatched or a.fuzzy or a.kind:
        want = "unmatched" if a.unmatched else "fuzzy" if a.fuzzy else a.kind
        for c in cases:
            if c.kind == want:
                tgt = " ".join(t.ref if isinstance(t, RustTest) else str(t) for t in c.targets)
                sc = f" [{c.score:.2f}]" if c.kind == "fuzzy" else ""
                print(f"{c.path}:{c.line}\t{c.name}\t{tgt}{sc}")
        return 0
    if a.rust_only:
        for t in rust_only(tests):
            print(f"{t.path}:{t.line}\t{t.name}")
        return 0

    text = listing(cases, tests)
    block = summary_block(cases, tests)
    listing_path = os.path.join(ROOT, LISTING)
    doc_path = os.path.join(ROOT, SUMMARY_DOC)
    doc = open(doc_path, encoding="utf-8").read() if os.path.exists(doc_path) else ""
    new_doc = with_summary(doc, block)
    if a.write:
        with open(listing_path, "w", encoding="utf-8", newline="\n") as f:
            f.write(text)
        if new_doc is None:
            print(f"{SUMMARY_DOC}: no summary markers, the block is not written", file=sys.stderr)
        else:
            with open(doc_path, "w", encoding="utf-8", newline="\n") as f:
                f.write(new_doc)
    if a.check:
        old = open(listing_path, encoding="utf-8").read() if os.path.exists(listing_path) else ""
        if old != text or new_doc is None or new_doc != doc:
            print(f"{LISTING} or the summary of {SUMMARY_DOC} is not up to date: run --write",
                  file=sys.stderr)
            return 1

    rows, totals = summary(cases, tests)
    print(f"{'C++ file':66} cases ported noform retired missing unmatched")
    for path, n, ported, noform, retired, missing, unmatched in rows:
        print(f"{path:66} {n:5} {ported:6} {noform:6} {retired:7} {missing:7} {unmatched:9}")
    print(f"total C++ cases {len(cases)}: " + ", ".join(f"{k} {totals[k]}" for k in KIND_ORDER if totals[k]))
    print(f"Rust tests {len(tests)}, without a C++ case {len(rust_only(tests))}")
    if stale:
        print("manual.tsv rows that match no C++ case:", file=sys.stderr)
        for k in stale:
            print(f"  {k[0]}\t{k[1]}", file=sys.stderr)
    named, broken = doc_refs(doc, tests)
    print(f"{SUMMARY_DOC}: {named} tests and files named, {len(broken)} not found")
    for b in broken:
        print(f"  not found: {b}", file=sys.stderr)
    return 1 if totals["unmatched"] or totals["missing"] or stale or broken else 0


WORKSPACES = {"esp": "software_esp32_rust", "stm": "software_stm32_rust"}


def doc_refs(doc, tests):
    """The `esp:<file>::<fn>` tests and `esp:<file>` files named in PARITY.md that do not exist."""
    refs = {t.ref for t in tests}
    named, broken = 0, []
    for ws, path, fn in re.findall(r'\b(esp|stm):([A-Za-z0-9_./-]+?)(?:::([A-Za-z0-9_]+))?(?=[`\s,;)]|$)', doc):
        named += 1
        full = f"{WORKSPACES[ws]}/{path}"
        if fn:
            if f"{full}::{fn}" not in refs:
                broken.append(f"{ws}:{path}::{fn}")
        elif not os.path.exists(os.path.join(ROOT, full)):
            broken.append(f"{ws}:{path}")
    return named, sorted(set(broken))


if __name__ == "__main__":
    sys.exit(main())
