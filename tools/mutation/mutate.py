#!/usr/bin/env python3
"""Source-level mutation testing of the VdMot Revamped native (host) test suites.

Every mutable token of the configured source files is changed one at a time in a private copy
of the project, the module's test executable is rebuilt and its tests run: a failing test run
kills the mutant, a passing one lets it survive. Score = killed / (killed + survived); confirmed
timeouts count as killed, mutants that do not compile ("stillborn") are left out of the score
(or counted as killed with "stillborn": "kill").

Usage (normally through tools/native/docker.sh mutate <suite> [args]):
  python3 tools/mutation/mutate.py --config tools/mutation/stm32.json --jobs 6
  ... --files lib/core/src/tokenizer.cpp [--lines 30-80] [--max 20]
  ... --list                  list the mutants and exit
  ... --no-fallback           stage 1 only: a quick run, not a gate
  ... --changed-since <rev>   print the configured files changed since <rev> and exit (needs
                              git, so on the host: the native image has none)
  ... --workdir <dir>         where the worker copies live (default: the system temp directory)
  ... --no-cache              ignore the kill cache

Config (JSON; every key is required except "test"; keys starting with "_" are comments):
  repo             repository root, relative to the config file
  root             project directory (relative to repo); commands run in its worker copy
  shared           directories (relative to repo) copied into every worker next to root
  files            files to mutate: a glob or a list of paths (relative to root)
  scope            glob (relative to root): every matching file is in files or in excluded
  excluded         {"path": "reason"} for the scope files that are not mutated
  equivalents_dir  equivalent mutants (relative to repo): one JSON array per source file in
                   <equivalents_dir>/<path with "/" replaced by "__">.json, entries
                   {"line": 12, "col": 9, "original": "<", "replacement": "<=",
                    "text": "<the source line, trimmed>", "reason": "why no test can tell"}
  setup            worker setup, e.g. a cmake configure; {build} is the worker's build directory
  build            builds the tests of one module; {stem} is the file name without extension;
                   mutant builds run with CCACHE_DISABLE=1
  test_file        stage 1: the module's fast tests ({build}, {stem}); must pass unmutated
  test             optional stage 2 for the survivors of stage 1 (slow and fuzz cases)
  timeout_min      test budget per stem and stage: max(timeout_min, timeout_factor x the
  timeout_factor   unmutated run) seconds; a timeout (also exit 124 of the glue runner, a case
                   that ran out of VDM_CASE_TIMEOUT_S) is confirmed by one solo re-run
  timeout_build    limit in seconds of setup, builds and the unmutated test runs
  threshold        minimum score (%) of the run
  file_threshold   minimum score (%) of every file
  deadcode         true: mutants the build does not compile (inactive #if, dropped macro
                   arguments) are "not_compiled"; needs {build}/compile_commands.json
  stillborn        "exclude" or "kill": how mutants that do not compile are scored

Source markers: "// NOMUTATE: <reason>" on a line suppresses its mutants; NOMUTATE without a
reason is a configuration error. Preprocessor directive lines are never mutated.

Statuses: killed; survived; timeout (confirmed by a solo re-run, counts as killed; a mutant
that fails its tests in the solo re-run is killed); stillborn (a compiler error located in the
mutated file: on the error line, in its include chain, as the origin of a template instantiation
or in a macro expansion; or an undefined symbol; a warning never counts); not_compiled;
equivalent; error (a timeout that passes its tests alone, a build that failed for another reason,
e.g. a compiler or linker crash, a full disk or a build timeout, also when the mutant warns in
the file, a test run that did not start or whose runner failed (exit 125-127); never cached,
the run exits 2). Killed, timeout, stillborn and not_compiled results are cached in
<config>.cache.json (key: file, source hash, hash of the config commands and of mutate.py, hash
of every file the workers copy (tests, headers, build files, sources), position, operator,
replacement), so an interrupted run resumes and a change of the tests or of this tool
invalidates the results; the file keeps the results of the current tree only, and survivors
always run again. The checkout is never written: every worker is a copy under --workdir. The
mean s/mutant covers the mutants built in this run; a re-run timeout counts with both runs.

Exit code: 0 when the score >= threshold, every file >= file_threshold and no mutant has the
status error; 1 when a threshold is missed; 2 on a configuration error, a failing unmutated
build or test, or an error mutant. A JSON report is written to <config>.report.json and a
Markdown summary to stdout.
"""
from __future__ import annotations

import argparse
import glob
import hashlib
import json
import math
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import asdict, dataclass

# ---------------------------------------------------------------- operators
# (name, regex, replacements); the regexes run on the source with comments, literals and
# directives blanked out, so offsets stay valid.
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

# Pointer and reference declarators ("char *p", "(uint16_t*)x", "int&& r") are not mutated:
# swapping their '*' or '&&' only gives compile errors.
TYPE_WORD = re.compile(r"(?:char|int|void|bool|float|double|auto|unsigned|signed|long|short|const|"
                       r"volatile|\w+_t|[A-Z]\w*[a-z]\w*)$")
DECL_PREFIX = re.compile(r"\b(?:struct|class|enum|union|const|volatile|static|extern|inline|"
                         r"constexpr|mutable|typename)\s+$")
DECLARED_NAME = re.compile(r"\s*[A-Za-z_)&*>,\[]")
# '&&' is a declarator only after a type: a builtin or *_t word, auto or const, a word after
# const/typename/..., or the closing '>' of a type template. A CamelCase word is no type there:
# enum values ("s == State::Idle && x") and members are operands of a logical and.
RVALUE_TYPE_WORD = re.compile(r"(?:char|int|void|bool|float|double|auto|unsigned|signed|long|short|"
                              r"const|volatile|\w+_t)$")
OPERAND_PREFIX = re.compile(r"(?:::|\.|->|==|!=|<=|>=|<|>)\s*$")
# '<' after a cast or template name opens a template argument list; that '<' and its matching '>'
# are not comparisons (a '<<' after std::cout is no template).
TEMPLATE_OPEN = re.compile(r"\b(?P<name>template|static_cast|reinterpret_cast|const_cast|dynamic_cast|"
                           r"std::\w+|array|vector)\s*<(?![<=])")
# The type templates among them; a std:: name ending in _v is a variable template, a value.
TYPE_TEMPLATE = re.compile(r"std::(?!\w*_v$)\w+|array|vector")

TIMEOUT_RC = -999
# Exit codes of a test command that are no kill: the glue runner (tools/native/testkit) exits with
# 124 when a case timed out and with 125 when the runner itself failed; the shell exits with 126
# or 127 when the command cannot be started.
RUNNER_TIMEOUT = 124
NOT_RUN = (125, 126, 127)
CACHEABLE = ("killed", "timeout", "stillborn", "not_compiled")
STATUSES = ("killed", "survived", "timeout", "stillborn", "not_compiled", "equivalent", "error")
# Toolchain and environment failures; "terminated with signal" is collect2's message for a killed
# ld or cc1plus ("ld terminated with signal 9 [Killed]").
BUILD_CRASHES = ("internal compiler error", "Killed signal terminated program",
                 "terminated with signal", "virtual memory exhausted", "out of memory",
                 "interrupted by user", "No space left on device", "ninja: error",
                 "Cannot allocate memory", "Input/output error")
LINK_ERRORS = ("undefined reference to", "undefined symbol")
# GCC diagnostics: "<file>:<line>[:<col>]: <kind>: <message>". The context of a diagnostic comes
# before it: the include chain ("In file included from <file>:<line>," continued by "from" lines),
# the enclosing function or template ("<file>: In instantiation of ...:") and the chain of
# instantiations or constant evaluations ("<file>:<line>:<col>:   required from here").
DIAGNOSTIC = re.compile(r"(?P<file>[^\s:][^:]*):\d+(?::\d+)?: "
                        r"(?P<kind>fatal error|error|warning|note): (?P<text>.*)")
DIAGNOSTIC_CONTEXT = re.compile(r"(?:In file included|\s+) from (?P<include>[^:]+):\d+(?::\d+)?[,:]"
                                r"|(?P<scope>[^\s:][^:]*): (?:In|At) .*:"
                                r"|(?P<chain>[^\s:][^:]*):\d+(?::\d+)?:   \S.*")
ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")


class ConfigError(Exception):
    pass


class Interrupted(Exception):
    pass


@dataclass
class Mutant:
    file: str
    line: int
    col: int
    pos: int
    op: str
    original: str
    replacement: str
    status: str = "pending"
    seconds: float = 0.0
    stage: int = 0
    detail: str = ""
    cached: bool = False
    close: int = -1  # negcond: position of the closing parenthesis of the condition


# ---------------------------------------------------------------- source scanning
def read_text(path: str) -> str:
    """Byte-exact text: CRLF and non-UTF-8 bytes survive a write_text() round trip."""
    with open(path, encoding="utf-8", errors="surrogateescape", newline="") as f:
        return f.read()


def write_text(path: str, text: str) -> None:
    with open(path, "w", encoding="utf-8", errors="surrogateescape", newline="") as f:
        f.write(text)


def scan(src: str) -> str:
    """Blanks comments, string/char literals and preprocessor directives (same length,
    newlines kept)."""
    out = list(src)
    i, n = 0, len(src)
    line_start = True

    def blank(a: int, b: int) -> None:
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = src[i]
        if c == "\n":
            line_start = True
            i += 1
            continue
        if line_start and c in " \t\r\f\v":
            i += 1
            continue
        if line_start and c == "#":
            j = i
            while True:  # to the end of the directive, following backslash continuations
                k = src.find("\n", j)
                if k == -1:
                    k = n
                if src[j:k].rstrip("\r").endswith("\\") and k < n:
                    j = k + 1
                    continue
                break
            blank(i, k)
            i = k
            continue
        line_start = False
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j == -1 else j
            blank(i, j)
            i = j
        elif src.startswith("/*", i):
            j = src.find("*/", i + 2)
            j = n if j == -1 else j + 2
            blank(i, j)
            i = j
        elif c in "\"'":
            j = i + 1
            while j < n and src[j] != c and src[j] != "\n":
                j += 2 if src[j] == "\\" else 1
            if j < n and src[j] == "\n":
                blank(i, j)
                i = j
            else:
                blank(i, min(j + 1, n))
                i = j + 1
        else:
            i += 1
    return "".join(out)


def line_col(src: str, pos: int) -> tuple[int, int]:
    line = src.count("\n", 0, pos) + 1
    col = pos - (src.rfind("\n", 0, pos) + 1) + 1
    return line, col


def directive_lines(src: str) -> set[int]:
    """Lines whose first non-blank character is '#', with their backslash continuations."""
    result: set[int] = set()
    continued = False
    for no, text in enumerate(src.split("\n"), 1):
        body = text.rstrip("\r")
        if continued or body.lstrip().startswith("#"):
            result.add(no)
            continued = body.endswith("\\")
        else:
            continued = False
    return result


NOMUTATE_MARK = re.compile(r"(//|/\*)\s*NOMUTATE:\s*(\S.*?)\s*(?:\*/)?\s*$")


def nomutate_lines(src: str, rel: str) -> tuple[dict[int, str], list[str]]:
    """{line: reason} of '// NOMUTATE: <reason>' markers and the errors for markers without
    a reason."""
    reasons: dict[int, str] = {}
    errors: list[str] = []
    for no, text in enumerate(src.split("\n"), 1):
        if "NOMUTATE" not in text:
            continue
        m = NOMUTATE_MARK.search(text.rstrip("\r"))
        if m:
            reasons[no] = m.group(2)
        else:
            errors.append(f"{rel}:{no}: NOMUTATE without ': <reason>': {text.strip()}")
    return reasons, errors


def template_brackets(clean: str) -> dict[int, re.Match]:
    """The template brackets of TEMPLATE_OPEN: {position of each '<' and of its matching '>': the
    TEMPLATE_OPEN match}, e.g. both brackets of static_cast<size_t>(n) but not the comparison in
    'static_cast<size_t>(n) < cap'."""
    result: dict[int, re.Match] = {}
    for m in TEMPLATE_OPEN.finditer(clean):
        result[m.end() - 1] = m
        close = matching_angle(clean, m.end() - 1)
        if close != -1:
            result[close] = m
    return result


def matching_angle(clean: str, open_pos: int) -> int:
    """The '>' that closes the template argument list opened at open_pos, or -1. Brackets inside
    parentheses and '<<', '<=', '>=', '->' do not count; ';' ends the search."""
    depth = parens = 0
    j = open_pos
    while j < min(len(clean), open_pos + 2000):
        c = clean[j]
        two = clean[j:j + 2]
        if c in "([{":
            parens += 1
        elif c in ")]}":
            if parens == 0:
                return -1
            parens -= 1
        elif c == ";":
            return -1
        elif parens == 0 and two in ("<<", "<=", ">=", "->"):
            j += 2
            continue
        elif parens == 0 and c == "<":
            depth += 1
        elif parens == 0 and c == ">":
            depth -= 1
            if depth == 0:
                return j
        j += 1
    return -1


def pointer_declarator(clean: str, start: int, end: int, brackets: dict[int, re.Match]) -> bool:
    """True when the '*' or '&&' at clean[start:end] declares a pointer or reference; brackets are
    the template brackets of template_brackets()."""
    before = clean[max(0, start - 200):start]
    m = re.search(r"(\w+)\s*$", before)
    if DECLARED_NAME.match(clean, end) is None:
        return False
    if clean[start:end] == "&&":
        return rvalue_declarator(clean, start, m, brackets)
    if m is None:
        return False
    if TYPE_WORD.match(m.group(1)):
        return True
    prefix = before[:m.start(1)]
    if DECL_PREFIX.search(prefix):
        return True
    # "name * other = ..." at the start of a statement: a declaration (a product would be unused)
    statement_start = re.search(r"[;{}]\s*$", prefix) is not None or \
        (start <= 200 and not prefix.strip())
    return clean[start:end] == "*" and statement_start and \
        re.match(r"\s*\w+\s*(?:[=;,\[)(]|$)", clean[end:end + 200]) is not None


def rvalue_declarator(clean: str, start: int, word: re.Match | None,
                      brackets: dict[int, re.Match]) -> bool:
    """True when the '&&' at `start` declares an rvalue reference ("int&& r", "auto&& a",
    "const Foo&& f", "Foo const&& g", "std::array<int, 2>&& v"); `word` is the word right before
    it, if any, as a match on the 200 characters before `start`. When in doubt the '&&' is a
    logical and: a wrong mutant is only stillborn, a skipped one is invisible."""
    before = clean[max(0, start - 200):start]
    if word is None:
        close = re.search(r">\s*$", before)
        opener = brackets.get(start - len(before) + close.start()) if close else None
        return opener is not None and type_declaration(clean, opener)
    prefix = before[:word.start(1)]
    if OPERAND_PREFIX.search(prefix):
        return False
    return RVALUE_TYPE_WORD.match(word.group(1)) is not None or DECL_PREFIX.search(prefix) is not None


def type_declaration(clean: str, opener: re.Match) -> bool:
    """True when the template argument list of `opener` (a TEMPLATE_OPEN match) belongs to a type
    template in a declaration position: at the start of the file or after '(', ',', ';', '{', '<'
    or a declaration keyword. "std::is_signed_v<T> && x" and "kWide<T> && x" are no declarations."""
    if TYPE_TEMPLATE.fullmatch(opener.group("name")) is None:
        return False
    head = clean[:opener.start()].rstrip()
    keyword = DECL_PREFIX.search(clean[max(0, opener.start() - 200):opener.start()])
    return not head or head[-1] in "(,;{<" or keyword is not None


def generate(src: str, rel: str, skip_lines: set[int]) -> list[Mutant]:
    clean = scan(src)
    muts: list[Mutant] = []
    seen: set[tuple[int, str, str]] = set()

    def add(pos: int, op: str, orig: str, rep: str, close: int = -1) -> None:
        line, col = line_col(src, pos)
        if line in skip_lines or (pos, orig, rep) in seen:
            return
        seen.add((pos, orig, rep))
        muts.append(Mutant(rel, line, col, pos, op, orig, rep, close=close))

    brackets = template_brackets(clean)
    for op, pattern, reps in BINARY_SWAPS:
        for m in re.finditer(pattern, clean):
            tok = m.group(0)
            if op == "rel" and m.start() in brackets:
                continue
            if op == "arith" and tok == "-" and re.match(r"\s*[\d(]", clean[m.end():]) \
                    and re.search(r"[=(,\[{?:]\s*$|return\s*$", clean[max(0, m.start() - 200):m.start()]):
                continue  # unary minus: the literal operator covers it
            if op == "arith" and tok == "*" and \
                    re.search(r"[\w\)\]]\s*$", clean[max(0, m.start() - 200):m.start()]) is None:
                continue  # dereference
            if tok in ("*", "&&") and pointer_declarator(clean, m.start(), m.end(), brackets):
                continue
            for r in reps:
                add(m.start(), op, tok, r)

    for m in INT_LITERAL.finditer(clean):
        v = int(m.group(1))
        for r in sorted({v + 1, max(v - 1, 0), 0 if v else 1}):
            if r != v:
                add(m.start(), "const", m.group(1), str(r))

    for m in NEGATE_IF.finditer(clean):
        close = matching_paren(clean, m.end() - 1)
        if close != -1:
            add(m.end() - 1, "negcond", "(", "(!", close)
    for m in RETURN_EXPR.finditer(clean):
        expr = m.group(1).strip()
        if expr in ("true", "false", "0", "1", "nullptr"):
            continue
        # the original text comes from src: literals are blanked in clean
        add(m.start(1), "retval", src[m.start(1):m.end(1)], "0")

    muts.sort(key=lambda x: (x.line, x.col, x.op, x.replacement))
    return muts


def matching_paren(clean: str, open_pos: int) -> int:
    depth = 0
    for j in range(open_pos, len(clean)):
        if clean[j] == "(":
            depth += 1
        elif clean[j] == ")":
            depth -= 1
            if depth == 0:
                return j
    return -1


def apply_mutant(src: str, m: Mutant) -> str:
    if src[m.pos:m.pos + len(m.original)] != m.original:
        raise ValueError(f"{m.file}:{m.line}:{m.col}: source does not match the mutant")
    if m.op == "negcond":
        return src[:m.pos] + "(!(" + src[m.pos + 1:m.close] + "))" + src[m.close + 1:]
    return src[:m.pos] + m.replacement + src[m.pos + len(m.original):]


# ---------------------------------------------------------------- configuration
NUMBER = (int, float)
REQUIRED_KEYS = {
    "repo": str, "root": str, "shared": list, "files": (str, list), "scope": str,
    "excluded": dict, "equivalents_dir": str, "setup": str, "build": str, "test_file": str,
    "timeout_min": NUMBER, "timeout_factor": NUMBER, "timeout_build": NUMBER,
    "threshold": NUMBER, "file_threshold": NUMBER, "deadcode": bool, "stillborn": str,
}
OPTIONAL_KEYS = {"test": str}


def relative_path_ok(p: str) -> bool:
    return bool(p) and not os.path.isabs(p) and not p.startswith(("/", "\\")) and \
        ".." not in p.replace("\\", "/").split("/")


def load_config(path: str) -> dict:
    """Reads and validates a mutation config (one schema for every suite)."""
    try:
        with open(path, encoding="utf-8") as f:
            cfg = json.load(f)
    except (OSError, ValueError) as e:
        raise ConfigError(f"{path}: {e}") from None
    if not isinstance(cfg, dict):
        raise ConfigError(f"{path}: the config is not a JSON object")
    problems = []
    for key, value in cfg.items():
        if key.startswith("_"):
            continue
        typ = REQUIRED_KEYS.get(key, OPTIONAL_KEYS.get(key))
        if typ is None:
            problems.append(f"unknown key {key!r}")
        elif not isinstance(value, typ) or (typ is NUMBER and isinstance(value, bool)):
            problems.append(f"{key!r} has the wrong type")
    problems += [f"missing key {k!r}" for k in REQUIRED_KEYS if k not in cfg]
    if not problems:
        files = cfg["files"] if isinstance(cfg["files"], list) else [cfg["files"]]
        if not files or not all(isinstance(p, str) and relative_path_ok(p) for p in files):
            problems.append("'files' must be a relative glob or a list of relative paths")
        if not all(isinstance(p, str) and relative_path_ok(p) for p in cfg["shared"]):
            problems.append("'shared' must list relative directories")
        for key in ("root", "scope", "equivalents_dir"):
            if not relative_path_ok(cfg[key]):
                problems.append(f"{key!r} must be a relative path")
        for p, reason in cfg["excluded"].items():
            if not isinstance(reason, str) or not reason.strip():
                problems.append(f"excluded {p!r} needs a written reason")
        if cfg["stillborn"] not in ("exclude", "kill"):
            problems.append("'stillborn' must be \"exclude\" or \"kill\"")
        for key in ("timeout_min", "timeout_factor", "timeout_build"):
            if cfg[key] <= 0:
                problems.append(f"{key!r} must be > 0")
        for key in ("threshold", "file_threshold"):
            if not 0 <= cfg[key] <= 100:
                problems.append(f"{key!r} must be 0..100")
        if "{build}" not in cfg["setup"]:
            problems.append("'setup' must use {build}")
        for key in ("build", "test_file"):
            if "{build}" not in cfg[key] or "{stem}" not in cfg[key]:
                problems.append(f"{key!r} must use {{build}} and {{stem}}")
    if problems:
        raise ConfigError("\n".join(f"{path}: {p}" for p in problems))
    return cfg


def expand(root: str, pattern: str) -> list[str]:
    hits = glob.glob(os.path.join(root, pattern), recursive=True)
    return sorted(os.path.relpath(h, root).replace(os.sep, "/") for h in hits if os.path.isfile(h))


def resolve_files(cfg: dict, root: str) -> list[str]:
    """The configured files, checked against scope and excluded."""
    if isinstance(cfg["files"], str):
        files = expand(root, cfg["files"])
    else:
        files = list(cfg["files"])
    problems = []
    for f in files:
        if not os.path.isfile(os.path.join(root, f)):
            problems.append(f"{f}: file not found")
    in_scope = expand(root, cfg["scope"])
    for f in cfg["excluded"]:
        if f in files:
            problems.append(f"{f}: both in files and in excluded")
        if f not in in_scope:
            problems.append(f"{f}: excluded but not an existing file of the scope {cfg['scope']!r}")
    for f in in_scope:
        if f not in files and f not in cfg["excluded"]:
            problems.append(f"{f}: in the scope {cfg['scope']!r} but neither in files nor in excluded")
    if not files:
        problems.append("no files to mutate")
    stems: dict[str, str] = {}
    for f in files:
        s = stem_of(f)
        if s in stems:
            problems.append(f"{f}: same stem as {stems[s]}")
        stems[s] = f
    if problems:
        raise ConfigError("\n".join(problems))
    return files


def stem_of(rel: str) -> str:
    return os.path.splitext(os.path.basename(rel))[0]


def sidecar_name(rel: str) -> str:
    return rel.replace("/", "__") + ".json"


def load_equivalents(eq_dir: str, rel: str, src: str, muts: list[Mutant]) -> dict[int, str]:
    """{mutant index: reason} from the sidecar of one file; raises ConfigError on a bad or
    stale entry."""
    path = os.path.join(eq_dir, sidecar_name(rel))
    if not os.path.isfile(path):
        return {}
    try:
        with open(path, encoding="utf-8") as f:
            entries = json.load(f)
    except (OSError, ValueError) as e:
        raise ConfigError(f"{path}: {e}") from None
    if not isinstance(entries, list):
        raise ConfigError(f"{path}: expected a JSON array")
    lines = src.split("\n")
    index = {(m.line, m.col, m.original, m.replacement): i for i, m in enumerate(muts)}
    result: dict[int, str] = {}
    problems = []
    keys = {"line": int, "col": int, "original": str, "replacement": str, "text": str, "reason": str}
    for n, e in enumerate(entries, 1):
        where = f"{path} entry {n}"
        if not isinstance(e, dict) or set(e) != set(keys) or \
                not all(isinstance(e[k], t) and not isinstance(e[k], bool) for k, t in keys.items()):
            problems.append(f"{where}: needs exactly line, col, original, replacement, text, reason: {e}")
            continue
        if not e["reason"].strip():
            problems.append(f"{where}: empty reason")
            continue
        text = lines[e["line"] - 1].strip() if 0 < e["line"] <= len(lines) else None
        i = index.get((e["line"], e["col"], e["original"], e["replacement"]))
        if text != e["text"].strip() or i is None:
            problems.append(f"{where}: stale, no mutant {e['original']!r} -> {e['replacement']!r} at "
                            f"{rel}:{e['line']}:{e['col']} on the line {e['text']!r}")
            continue
        if i in result:
            problems.append(f"{where}: duplicate entry")
            continue
        result[i] = e["reason"].strip()
    if problems:
        raise ConfigError("\n".join(problems))
    return result


# ---------------------------------------------------------------- processes
class Runner:
    """Runs shell commands in their own process group; a timeout kills the whole group."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.groups: set[int] = set()
        self.stopped = False

    def run(self, cmd: str, cwd: str, timeout: float, env: dict | None = None) -> tuple[int, str, float]:
        # CPU limit for orphans: a group that survives a killed runner stops by itself
        cpu = int(max(timeout * 2, 60))
        start = time.monotonic()
        with self.lock:
            if self.stopped:
                raise Interrupted()
            p = subprocess.Popen(f"ulimit -t {cpu}; {cmd}", shell=True, cwd=cwd, env=env,
                                 stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
            self.groups.add(p.pid)
        try:
            out, _ = p.communicate(timeout=timeout)
            rc = p.returncode
        except subprocess.TimeoutExpired:
            self.kill(p.pid)
            out, _ = p.communicate()
            rc = TIMEOUT_RC
        finally:
            with self.lock:
                self.groups.discard(p.pid)
        return rc, out.decode("utf-8", "replace"), time.monotonic() - start

    @staticmethod
    def kill(pgid: int) -> None:
        try:
            os.killpg(pgid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass

    def stop(self) -> None:
        """Kills every running command; later run() calls raise Interrupted."""
        with self.lock:
            self.stopped = True
            groups = list(self.groups)
        for g in groups:
            self.kill(g)


def fmt(cmd: str, build: str, stem: str = "") -> str:
    return cmd.replace("{build}", build).replace("{stem}", stem)


def tail(text: str, n: int = 3000) -> str:
    return text[-n:]


def compiler_errors(out: str) -> list[tuple[str, list[str]]]:
    """(error line, files it is located in) of each compiler error of a build output. The files are
    those of the error line, of the context lines before it and of its 'in expansion of macro'
    notes. Warnings and their context are skipped: a mutant that only warns compiles."""
    errors: list[tuple[str, list[str]]] = []
    context: list[str] = []
    in_error = False
    for raw in out.splitlines():
        line = ANSI_ESCAPE.sub("", raw).rstrip()
        d = DIAGNOSTIC.fullmatch(line)
        if d is None:
            c = DIAGNOSTIC_CONTEXT.fullmatch(line)
            if c is not None:
                context.append(next(f for f in c.groups() if f))
            continue
        if d.group("kind") == "note":
            if in_error and d.group("text").startswith("in expansion of macro"):
                errors[-1][1].append(d.group("file"))
        else:
            in_error = d.group("kind") != "warning"
            if in_error:
                errors.append((line, context + [d.group("file")]))
        context = []
    return errors


def classify_build_failure(rc: int, out: str, rel: str) -> tuple[str, str]:
    """A failed mutant build is stillborn when the compiler reports an error located in the mutated
    file (on the error line, in its include chain, as the origin of a template instantiation or
    in a macro expansion), or when the linker misses a symbol. A warning in the mutated file does
    not count, and a toolchain crash is never stillborn: any other failure is an error of the
    build environment."""
    if rc == TIMEOUT_RC:
        return "error", "build timeout"
    for pattern in BUILD_CRASHES:
        if pattern in out:
            return "error", f"build failed: {pattern}"
    if rc < 0:
        return "error", f"build killed by signal {-rc}"
    target = os.path.normpath(rel).replace(os.sep, "/")

    def mutated(path: str) -> bool:
        p = os.path.normpath(path).replace(os.sep, "/")
        return p == target or p.endswith("/" + target)

    located = [line for line, files in compiler_errors(out) if any(mutated(f) for f in files)]
    if located:
        return "stillborn", located[0][:200]
    lines = [line.strip() for line in out.splitlines()]
    missing = [line for line in lines if any(p in line for p in LINK_ERRORS)]
    if missing and any("error:" in line for line in lines):
        return "stillborn", missing[0][:200]
    return "error", f"build failed (exit {rc}) without a compiler error in {rel}"


# ---------------------------------------------------------------- workers
class Worker:
    def __init__(self, base: str, index: int, cfg: dict) -> None:
        self.index = index
        self.dir = os.path.join(base, f"w{index}")
        self.root = os.path.join(self.dir, cfg["root"])
        self.build = os.path.join(self.dir, "build")


IGNORED_NAMES = (".pio", "build", ".git", "node_modules", "__pycache__")
IGNORED = shutil.ignore_patterns(*IGNORED_NAMES)


def copy_sources(repo: str, dst: str, cfg: dict) -> None:
    for rel in [cfg["root"]] + list(cfg["shared"]):
        src = os.path.join(repo, rel)
        if not os.path.isdir(src):
            raise ConfigError(f"{rel}: directory not found")
        shutil.copytree(src, os.path.join(dst, rel), symlinks=True, ignore=IGNORED)


def inputs_hash(repo: str, cfg: dict) -> str:
    """Hash of every file a worker copies (root and shared): a cached result is reused only while
    the tests, headers, build files and sources it came from are unchanged."""
    h = hashlib.sha1()
    for top in [cfg["root"]] + list(cfg["shared"]):
        for dirpath, dirnames, filenames in os.walk(os.path.join(repo, top)):
            dirnames[:] = sorted(d for d in dirnames if d not in IGNORED_NAMES)
            for name in sorted(n for n in filenames if n not in IGNORED_NAMES):
                path = os.path.join(dirpath, name)
                rel = os.path.relpath(path, repo).replace(os.sep, "/")
                h.update(rel.encode("utf-8", "surrogateescape") + b"\0")
                try:
                    with open(path, "rb") as f:
                        h.update(hashlib.sha1(f.read()).digest())
                except OSError:
                    h.update(b"\0unreadable")
    return h.hexdigest()[:16]


def build_env(disable_ccache: bool, jobs: int) -> dict:
    env = dict(os.environ)
    if disable_ccache:
        env["CCACHE_DISABLE"] = "1"
    else:
        env.setdefault("CMAKE_BUILD_PARALLEL_LEVEL", str(max(1, (os.cpu_count() or 1) // jobs)))
    return env


# ---------------------------------------------------------------- not compiled
def compile_command(build: str, path: str) -> tuple[str, list[str]] | None:
    try:
        with open(os.path.join(build, "compile_commands.json"), encoding="utf-8") as f:
            db = json.load(f)
    except (OSError, ValueError):
        return None
    target = os.path.realpath(path)
    for e in db:
        f = e["file"] if os.path.isabs(e["file"]) else os.path.join(e["directory"], e["file"])
        if os.path.realpath(f) == target:
            return e["directory"], (e.get("arguments") or shlex.split(e["command"]))
    return None


def preprocess_args(directory: str, args: list[str], original: str, source: str,
                    extra: list[str]) -> list[str]:
    """The compile command of `original` turned into a preprocessor run of `source` (to stdout)."""
    out: list[str] = []
    skip_next = False
    target = os.path.realpath(original)
    for a in args[1:]:
        if skip_next:
            skip_next = False
            continue
        if a in ("-o", "-MF", "-MT", "-MQ"):
            skip_next = True
            continue
        if a in ("-c", "-MD", "-MMD", "-MP") or a.startswith(("-MF", "-MT", "-MQ")):
            continue
        if not a.startswith("-") and os.path.realpath(os.path.join(directory, a)) == target:
            continue
        out.append(a)
    compiler = args[0]
    if os.path.basename(compiler) == "ccache":
        compiler, out = out[0], out[1:]
    return [compiler] + out + ["-E", "-P", "-w"] + extra + [source]


def preprocess(cmd: tuple[str, list[str]], original: str, source: str, extra: list[str],
               timeout: float) -> str | None:
    directory, args = cmd
    try:
        p = subprocess.run(preprocess_args(directory, args, original, source, extra), cwd=directory,
                           capture_output=True, timeout=timeout)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return p.stdout.decode("utf-8", "replace") if p.returncode == 0 else None


def compiled_positions(cmd: tuple[str, list[str]], path: str, src: str, positions: list[int],
                       timeout: float) -> set[int] | None:
    """The mutant positions whose marker comment survives the preprocessor: a marker is dropped
    with an inactive #if region or a macro argument the macro does not use."""
    marked = src
    for pos in sorted(set(positions), reverse=True):
        # the space keeps a preceding '/' from turning the marker into a line comment
        marked = f"{marked[:pos]} /*VDMPOS:{pos}*/{marked[pos:]}"
    tmp = os.path.join(os.path.dirname(path), ".vdm_deadcode_" + os.path.basename(path))
    write_text(tmp, marked)
    try:
        out = preprocess(cmd, path, tmp, ["-C"], timeout)
    finally:
        os.remove(tmp)
    if out is None:
        return None
    return {int(n) for n in re.findall(r"VDMPOS:(\d+)", out)}


def classify_not_compiled(w: Worker, rel: str, src: str, muts: list[Mutant], timeout: float) -> str:
    """Marks the mutants the build does not compile as not_compiled; returns a note. Mutants
    whose marker is missing are confirmed by comparing the preprocessed file with and without
    the mutant."""
    path = os.path.join(w.root, rel)
    cmd = compile_command(w.build, path)
    if cmd is None:
        return f"{rel}: no compile command, every mutant is built"
    pending = [m for m in muts if m.status == "pending"]
    compiled = compiled_positions(cmd, path, src, [m.pos for m in pending], timeout)
    if compiled is None:
        return f"{rel}: the preprocessor failed, every mutant is built"
    candidates = [m for m in pending if m.pos not in compiled]
    if not candidates:
        return ""
    original = preprocess(cmd, path, path, [], timeout)
    try:
        for m in candidates:
            write_text(path, apply_mutant(src, m))
            if original is not None and preprocess(cmd, path, path, [], timeout) == original:
                m.status = "not_compiled"
                m.detail = "not compiled with the build's defines"
    finally:
        write_text(path, src)
    return ""


# ---------------------------------------------------------------- evaluation
def evaluate(m: Mutant, w: Worker, cfg: dict, budgets: dict, quick: bool, runner: Runner,
             src: str, env: dict) -> None:
    """Builds and tests one mutant in worker w; sets status, stage, detail and seconds."""
    path = os.path.join(w.root, m.file)
    stem = stem_of(m.file)
    t0 = time.monotonic()
    write_text(path, apply_mutant(src, m))
    try:
        rc, out, _ = runner.run(fmt(cfg["build"], w.build, stem), w.root, cfg["timeout_build"], env)
        if rc != 0:
            m.status, m.detail = classify_build_failure(rc, out, m.file)
            return
        stages = [(1, cfg["test_file"])]
        if cfg.get("test") and not quick:
            stages.append((2, cfg["test"]))
        for stage, cmd in stages:
            m.stage = stage
            budget = budgets[stem][stage - 1]
            rc, out, _ = runner.run(fmt(cmd, w.build, stem), w.root, budget, env)
            if rc == TIMEOUT_RC:
                m.status = "timeout?"
                m.detail = f"stage {stage} exceeded {budget:.1f} s"
                return
            if rc == RUNNER_TIMEOUT:
                m.status = "timeout?"
                m.detail = f"stage {stage}: a test case timed out (exit {rc})"
                return
            if rc in NOT_RUN:
                last = out.strip().splitlines()[-1:] or [""]
                m.status = "error"
                m.detail = f"stage {stage}: no test verdict (exit {rc}): {last[0][:200]}"
                return
            if rc != 0:
                m.status = "killed"
                return
        m.status = "survived"
    finally:
        write_text(path, src)
        m.seconds = round(time.monotonic() - t0, 3)


def mutant_key(m: Mutant, src_hash: str, run_hash: str) -> str:
    """Cache key; run_hash covers this tool, the config commands and every file of the worker
    copies. It comes first: load_cache() keeps the keys of one run hash."""
    return f"{run_hash}|{m.file}|{src_hash}|{m.line}:{m.col}|{m.op}|{m.original}|{m.replacement}"


def config_hash(cfg: dict) -> str:
    """Hash of the config commands and of this file: a cached result is reused only with the
    generator and the classification that made it."""
    keys = ("setup", "build", "test_file", "test", "shared", "deadcode", "stillborn",
            "timeout_min", "timeout_factor")
    blob = json.dumps({k: cfg.get(k) for k in keys}, sort_keys=True)
    with open(__file__, "rb") as f:
        tool = f.read()
    return hashlib.sha1(blob.encode() + tool).hexdigest()[:12]


def load_cache(path: str, run_hash: str) -> dict:
    """The cached results of run_hash. Results of another tree, config or tool version are never
    reused, so they are dropped: the file holds one run hash instead of growing with every edit,
    and an interrupted run on the same tree still resumes."""
    try:
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return {}
    if not isinstance(data, dict):
        return {}
    return {k: v for k, v in data.items() if k.startswith(run_hash + "|")}


def save_cache(path: str, cache: dict) -> None:
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(cache, f)
    os.replace(tmp, path)


# ---------------------------------------------------------------- report
def counted_kills(counts: dict, stillborn_kills: bool) -> int:
    return counts["killed"] + counts["timeout"] + (counts["stillborn"] if stillborn_kills else 0)


def summarize(muts: list[Mutant], files: list[str], cfg: dict) -> tuple[dict, dict]:
    stillborn_kills = cfg["stillborn"] == "kill"
    per_file: dict[str, dict] = {}
    for f in files:
        per_file[f] = {s: 0 for s in STATUSES}
        per_file[f].update(pending=0, run=0, seconds=0.0)
    for m in muts:
        c = per_file[m.file]
        c[m.status if m.status in STATUSES else "pending"] += 1
        # the mean covers the mutants that were built: not the cached, equivalent and not
        # compiled ones
        if not m.cached and m.status not in ("pending", "equivalent", "not_compiled"):
            c["run"] += 1
            c["seconds"] += m.seconds
    totals = {s: 0 for s in STATUSES}
    totals.update(pending=0, run=0, seconds=0.0)
    for c in per_file.values():
        for k in totals:
            totals[k] += c[k]
    for c in list(per_file.values()) + [totals]:
        kills = counted_kills(c, stillborn_kills)
        counted = kills + c["survived"]
        c["counted"] = counted
        c["score"] = round(100.0 * kills / counted, 2) if counted else None
        c["mean_s"] = round(c["seconds"] / c["run"], 3) if c["run"] else None
        need = math.ceil(cfg["file_threshold"] * counted / 100.0 - 1e-9)
        c["gap"] = max(0, need - kills)
        c["seconds"] = round(c["seconds"], 1)
    return per_file, totals


def print_report(per_file: dict, totals: dict, muts: list[Mutant], cfg: dict, quick: bool,
                 nomutate: dict, notes: list[str], wall: float, jobs: int) -> list[str]:
    """Markdown summary; returns the files below file_threshold."""
    def pct(v):
        return "-" if v is None else f"{v:.1f} %"

    score = totals["score"] if totals["score"] is not None else 100.0
    if quick:
        print(f"\n## Mutation score: {score:.1f} % (quick run, stage 1 only, not a gate)")
    else:
        print(f"\n## Mutation score: {score:.1f} % (threshold {cfg['threshold']} %, "
              f"per file {cfg['file_threshold']} %)")
    print(f"killed {totals['killed']} + timeout {totals['timeout']} of {totals['counted']} counted; "
          f"stillborn {totals['stillborn']} ({cfg['stillborn']}), not compiled {totals['not_compiled']}, "
          f"equivalent {totals['equivalent']}, error {totals['error']}")
    mean = totals["mean_s"]
    print(f"{totals['run']} mutants run, mean {mean if mean is not None else '-'} s per mutant, "
          f"wall {wall / 60:.1f} min, {jobs} workers")
    print("\n| file | killed | survived | timeout | stillborn | not compiled | equivalent | error "
          "| score | gap | s/mutant |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    failing = []
    for f, c in sorted(per_file.items()):
        if c["score"] is not None and c["score"] < cfg["file_threshold"]:
            failing.append(f)
        print(f"| {f} | {c['killed']} | {c['survived']} | {c['timeout']} | {c['stillborn']} | "
              f"{c['not_compiled']} | {c['equivalent']} | {c['error']} | {pct(c['score'])} | "
              f"{c['gap']} | {c['mean_s'] if c['mean_s'] is not None else '-'} |")
    t = totals
    print(f"| **total** | {t['killed']} | {t['survived']} | {t['timeout']} | {t['stillborn']} | "
          f"{t['not_compiled']} | {t['equivalent']} | {t['error']} | {pct(t['score'])} | "
          f"{t['gap']} | {t['mean_s'] if t['mean_s'] is not None else '-'} |")

    def listing(title: str, status: str, with_detail: bool) -> None:
        sel = [m for m in muts if m.status == status]
        if sel:
            print(f"\n### {title} ({len(sel)})")
            for m in sel:
                extra = f": {m.detail}" if with_detail and m.detail else ""
                print(f"- {m.file}:{m.line}:{m.col} `{m.original}` -> `{m.replacement}` ({m.op}){extra}")

    listing("Surviving mutants", "survived", False)
    listing("Errors", "error", True)
    listing("Equivalent mutants", "equivalent", True)
    listing("Not compiled", "not_compiled", False)
    listing("Stillborn", "stillborn", True)
    marks = [(f, line, reason) for f, d in sorted(nomutate.items()) for line, reason in sorted(d.items())]
    if marks:
        print(f"\n### NOMUTATE lines ({len(marks)})")
        for f, line, reason in marks:
            print(f"- {f}:{line}: {reason}")
    for note in notes:
        print(f"\nnote: {note}")
    if failing and not quick:
        print("\nfiles below the per-file threshold: " +
              ", ".join(f"{f} ({per_file[f]['score']:.1f} %)" for f in failing))
    return failing


# ---------------------------------------------------------------- main
def parse_args(argv: list[str] | None) -> argparse.Namespace:
    ap = argparse.ArgumentParser(description="Mutation testing of the native test suites.")
    ap.add_argument("--config", required=True)
    ap.add_argument("--files", nargs="+", help="limit to these configured files (relative to root)")
    ap.add_argument("--lines", help="limit to the mutants on lines a-b (or one line a)")
    ap.add_argument("--max", type=int, default=0, help="limit the number of mutants (0 = all)")
    ap.add_argument("--jobs", type=int, default=1, help="parallel workers, each on its own copy")
    ap.add_argument("--list", action="store_true", help="only list the mutants")
    ap.add_argument("--no-fallback", action="store_true",
                    help="stage 1 only: a quick run, not a gate")
    ap.add_argument("--changed-since", metavar="GIT_REV",
                    help="print the configured files changed since GIT_REV (space separated) and exit")
    ap.add_argument("--workdir", default=tempfile.gettempdir(),
                    help="directory for the worker copies (default: the system temp directory)")
    ap.add_argument("--no-cache", action="store_true", help="ignore the kill cache")
    return ap.parse_args(argv)


def parse_lines(spec: str) -> tuple[int, int]:
    m = re.fullmatch(r"(\d+)(?:-(\d+))?", spec)
    if m is None:
        raise ConfigError(f"--lines {spec!r}: expected a-b or a")
    a = int(m.group(1))
    b = int(m.group(2) or a)
    if b < a:
        raise ConfigError(f"--lines {spec!r}: empty range")
    return a, b


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        return run(args)
    except ConfigError as e:
        print(f"config error:\n{e}", file=sys.stderr)
        return 2


def run(args: argparse.Namespace) -> int:
    cfg_path = os.path.abspath(args.config)
    cfg = load_config(cfg_path)
    repo = os.path.abspath(os.path.join(os.path.dirname(cfg_path), cfg["repo"]))
    root = os.path.join(repo, cfg["root"])
    if not os.path.isdir(root):
        raise ConfigError(f"{root}: root directory not found")
    files = resolve_files(cfg, root)

    if args.changed_since:
        try:
            p = subprocess.run(["git", "diff", "--name-only", args.changed_since, "--", "."],
                               cwd=root, capture_output=True, text=True)
        except OSError as e:
            # the native image has no git on purpose: in the container it would not see the host's
            # core.autocrlf and would report every CRLF file as changed
            print(f"--changed-since needs git: {e.strerror}. Run it where the checkout's git is "
                  f"(the host) and pass the files with --files.", file=sys.stderr)
            return 2
        if p.returncode != 0:
            print(p.stderr, file=sys.stderr)
            return 2
        changed = {os.path.relpath(os.path.join(repo, x.strip()), root).replace(os.sep, "/")
                   for x in p.stdout.splitlines() if x.strip()}
        print(" ".join(f for f in files if f in changed))
        return 0

    selected = files
    if args.files:
        unknown = [f for f in args.files if f not in files]
        if unknown:
            raise ConfigError("not configured files: " + ", ".join(unknown))
        selected = [f for f in files if f in args.files]
    line_range = parse_lines(args.lines) if args.lines else None

    eq_dir = os.path.join(repo, cfg["equivalents_dir"])
    if os.path.isdir(eq_dir):
        expected = {sidecar_name(f) for f in files}
        orphans = sorted(n for n in os.listdir(eq_dir) if n.endswith(".json") and n not in expected)
        if orphans:
            raise ConfigError("equivalent files without a configured source: " +
                              ", ".join(os.path.join(cfg["equivalents_dir"], o) for o in orphans))

    sources: dict[str, str] = {}
    src_hash: dict[str, str] = {}
    nomutate: dict[str, dict[int, str]] = {}
    all_muts: list[Mutant] = []
    problems: list[str] = []
    for rel in selected:
        path = os.path.join(root, rel)
        src = read_text(path)
        sources[rel] = src
        with open(path, "rb") as f:
            src_hash[rel] = hashlib.sha1(f.read()).hexdigest()[:16]
        reasons, errors = nomutate_lines(src, rel)
        problems += errors
        nomutate[rel] = reasons
        muts = generate(src, rel, set(reasons) | directive_lines(src))
        try:
            for i, reason in load_equivalents(eq_dir, rel, src, muts).items():
                muts[i].status = "equivalent"
                muts[i].detail = reason
        except ConfigError as e:
            problems.append(str(e))
        if line_range:
            muts = [m for m in muts if line_range[0] <= m.line <= line_range[1]]
        all_muts += muts
    if problems:
        raise ConfigError("\n".join(problems))
    if args.max:
        all_muts = all_muts[:args.max]

    if args.list:
        for m in all_muts:
            extra = f"  [equivalent: {m.detail}]" if m.status == "equivalent" else ""
            print(f"{m.file}:{m.line}:{m.col} {m.op} {m.original!r} -> {m.replacement!r}{extra}")
        print(f"{len(all_muts)} mutants")
        return 0

    run_hash = f"{config_hash(cfg)}|{inputs_hash(repo, cfg)}"
    cache_path = os.path.splitext(cfg_path)[0] + ".cache.json"
    cache = {} if args.no_cache else load_cache(cache_path, run_hash)
    reused = 0
    for m in all_muts:
        if m.status != "pending":
            continue
        hit = cache.get(mutant_key(m, src_hash[m.file], run_hash))
        if hit in CACHEABLE:
            m.status = hit
            m.cached = True
            reused += 1

    jobs = max(1, args.jobs)
    quick = args.no_fallback
    runner = Runner()
    started = time.monotonic()
    os.makedirs(args.workdir, exist_ok=True)
    base = tempfile.mkdtemp(prefix="mutate-", dir=args.workdir)
    notes: list[str] = []
    interrupted = [0]
    cache_lock = threading.Lock()
    todo_lock = threading.Lock()

    def on_signal(signum, _frame):
        interrupted[0] = signum
        raise Interrupted()

    old_handlers = {s: signal.signal(s, on_signal) for s in (signal.SIGTERM, signal.SIGINT)}
    try:
        todo = [m for m in all_muts if m.status == "pending"]
        print(f"{len(all_muts)} mutants, {reused} from the cache, {len(todo)} to run", flush=True)
        workers = [Worker(base, 0, cfg)]
        stems = sorted({stem_of(m.file) for m in all_muts})
        budgets: dict[str, tuple[float, float]] = {}
        setup_env = build_env(False, jobs)
        if all_muts:
            # the unmutated tests run also when every result comes from the cache: cached kills
            # of a broken test suite would be worthless
            copy_sources(repo, workers[0].dir, cfg)
            prepare(workers[0], cfg, stems, runner, setup_env)
            budgets = baseline(workers[0], cfg, stems, quick, runner)
        if todo and cfg["deadcode"]:
            for rel in sorted({m.file for m in todo}):
                note = classify_not_compiled(workers[0], rel, sources[rel],
                                             [m for m in todo if m.file == rel], cfg["timeout_build"])
                if note:
                    notes.append(note)
            with cache_lock:
                for m in todo:
                    if m.status == "not_compiled":
                        cache[mutant_key(m, src_hash[m.file], run_hash)] = m.status
            todo = [m for m in todo if m.status == "pending"]
        workers += [Worker(base, i, cfg) for i in range(1, min(jobs, len(todo)))]
        for w in workers[1:]:
            shutil.copytree(workers[0].dir, w.dir, symlinks=True, ignore=IGNORED)
        threads = [threading.Thread(target=prepare, args=(w, cfg, stems, runner, setup_env), daemon=True)
                   for w in workers[1:]]
        for t in threads:
            t.start()
        join_all(threads)
        for w in workers[1:]:
            if getattr(w, "failure", None):
                raise ConfigError(w.failure)

        mutant_env = build_env(True, jobs)
        total = len(all_muts)
        done = [reused + sum(m.status == "not_compiled" and not m.cached for m in all_muts)]
        confirm: list[Mutant] = []

        def record(m: Mutant) -> None:
            with cache_lock:
                done[0] += 1
                if m.status in CACHEABLE:
                    cache[mutant_key(m, src_hash[m.file], run_hash)] = m.status
                if done[0] % 50 == 0:
                    save_cache(cache_path, cache)
                if done[0] % 25 == 0 or m.status in ("survived", "error"):
                    print(f"[{done[0]}/{total}] {m.status:9} {m.file}:{m.line}:{m.col} {m.op} "
                          f"{m.original!r}->{m.replacement!r}", flush=True)

        def work(w: Worker) -> None:
            while True:
                with todo_lock:
                    if not todo:
                        return
                    m = todo.pop(0)
                try:
                    evaluate(m, w, cfg, budgets, quick, runner, sources[m.file], mutant_env)
                except Interrupted:
                    m.status = "pending"
                    return
                except Exception as exc:  # one broken mutant must not stop a worker
                    m.status = "error"
                    m.detail = f"runner exception: {exc!r}"
                if m.status == "timeout?":
                    with todo_lock:
                        confirm.append(m)
                    continue
                record(m)

        threads = [threading.Thread(target=work, args=(w,), daemon=True) for w in workers]
        for t in threads:
            t.start()
        join_all(threads)

        # timeouts are confirmed alone, so the load of the other workers cannot cause them; a
        # mutant that fails its tests alone is killed, one that passes them alone is an error
        for m in sorted(confirm, key=lambda x: (x.file, x.line, x.col)):
            first, first_s = m.detail, m.seconds
            m.detail = ""
            evaluate(m, workers[0], cfg, budgets, quick, runner, sources[m.file], mutant_env)
            if m.status == "timeout?":
                m.status = "timeout"
                m.detail = f"{first}; confirmed alone"
            elif m.status == "killed":
                m.detail = f"{first} under load; alone killed after {m.seconds:.1f} s"
            else:
                solo = f"{m.status} ({m.detail})" if m.detail else m.status
                m.detail = (f"unconfirmed timeout: {first}, alone the mutant was {solo} "
                            f"after {m.seconds:.1f} s")
                m.status = "error"
            m.seconds = round(first_s + m.seconds, 3)
            record(m)
    except Interrupted:
        print(f"interrupted by signal {interrupted[0]}", file=sys.stderr)
        return 128 + interrupted[0]
    finally:
        runner.stop()
        with cache_lock:
            save_cache(cache_path, cache)
        shutil.rmtree(base, ignore_errors=True)
        for s, h in old_handlers.items():
            signal.signal(s, h)

    wall = time.monotonic() - started
    per_file, totals = summarize(all_muts, selected, cfg)
    failing = print_report(per_file, totals, all_muts, cfg, quick, nomutate, notes, wall, jobs)
    score = totals["score"] if totals["score"] is not None else 100.0
    unfinished = totals["error"] + totals["pending"]
    passed = not quick and unfinished == 0 and score >= cfg["threshold"] and not failing
    report = {
        "config": os.path.relpath(cfg_path, repo).replace(os.sep, "/"),
        "quick": quick, "passed": passed, "score": round(score, 2),
        "threshold": cfg["threshold"], "file_threshold": cfg["file_threshold"],
        "stillborn": cfg["stillborn"], "jobs": jobs, "wall_s": round(wall, 1),
        "totals": totals, "files": per_file,
        "nomutate": {f: {str(k): v for k, v in d.items()} for f, d in nomutate.items() if d},
        "notes": notes,
        "mutants": [{k: v for k, v in asdict(m).items() if k not in ("pos", "cached", "close")}
                    for m in all_muts],
    }
    rpt = os.path.splitext(cfg_path)[0] + ".report.json"
    with open(rpt, "w", encoding="utf-8") as f:
        json.dump(report, f, indent=1)
    if unfinished:
        print(f"\nERROR: {unfinished} mutants have the status error or were not run; the score is not valid.")
        return 2
    if quick:
        return 0
    return 0 if passed else 1


def join_all(threads: list[threading.Thread]) -> None:
    for t in threads:
        while t.is_alive():
            t.join(0.5)


def prepare(w: Worker, cfg: dict, stems: list[str], runner: Runner, env: dict) -> None:
    """Configures a worker and builds the test executable of every stem."""
    try:
        rc, out, _ = runner.run(fmt(cfg["setup"], w.build), w.root, cfg["timeout_build"], env)
        if rc != 0:
            raise ConfigError(f"worker {w.index}: setup failed (exit {rc})\n{tail(out)}")
        for stem in stems:
            rc, out, _ = runner.run(fmt(cfg["build"], w.build, stem), w.root, cfg["timeout_build"], env)
            if rc != 0:
                raise ConfigError(f"worker {w.index}: the unmutated build of {stem!r} fails (exit {rc})\n"
                                  f"{tail(out)}")
    except ConfigError as e:
        if w.index == 0:
            raise
        w.failure = str(e)


def baseline(w: Worker, cfg: dict, stems: list[str], quick: bool, runner: Runner) -> dict:
    """Runs the unmutated tests of every stem; returns {stem: (stage 1 budget, stage 2 budget)}."""
    budgets = {}
    shared_stage2 = None
    for stem in stems:
        times = []
        for stage, key in ((1, "test_file"), (2, "test")):
            if key not in cfg or (stage == 2 and quick):
                times.append(0.0)
                continue
            if stage == 2 and "{stem}" not in cfg[key] and shared_stage2 is not None:
                times.append(shared_stage2)
                continue
            rc, out, secs = runner.run(fmt(cfg[key], w.build, stem), w.root, cfg["timeout_build"])
            if rc != 0:
                raise ConfigError(f"the unmutated stage {stage} test of {stem!r} fails "
                                  f"({'timeout' if rc == TIMEOUT_RC else f'exit {rc}'}); fix the tests "
                                  f"before mutation testing\n{tail(out)}")
            times.append(secs)
            if stage == 2 and "{stem}" not in cfg[key]:
                shared_stage2 = secs
        budgets[stem] = tuple(max(cfg["timeout_min"], cfg["timeout_factor"] * t) for t in times)
    return budgets


if __name__ == "__main__":
    sys.exit(main())
