#!/usr/bin/env bash
# Self-test of mutate.py: each check builds a tiny project in a temp directory and runs the tool
# on it (CI native job; locally: tools/native/docker.sh run 'bash tools/mutation/selftest.sh').
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
MUTATE="$HERE/mutate.py"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
export PYTHONDONTWRITEBYTECODE=1

fail() {
  echo "mutate.py selftest FAILED: $*"
  [ -f "$T/out" ] && tail -n 40 "$T/out" | sed 's/^/  | /'
  exit 1
}
ok() { echo "ok: $*"; }

# project <name>: an empty project; sources in src/, test_<stem>.cpp includes src/<stem>.cpp.
# setup.sh writes compile_commands.json (deadcode analysis), build.sh logs the stem it builds.
project() {
  local p="$T/$1/proj"
  mkdir -p "$p/src" "$T/$1/equiv"
  cat >"$p/setup.sh" <<'SH'
set -e
mkdir -p "$1"
{
  echo "["
  sep=""
  for f in src/*.cpp; do
    printf '%s{"directory": "%s", "command": "g++ -std=c++17 -Wall -Wextra -c %s -o %s/x.o", "file": "%s"}\n' \
      "$sep" "$PWD" "$f" "$1" "$f"
    sep=","
  done
  echo "]"
} >"$1/compile_commands.json"
SH
  cat >"$p/build.sh" <<'SH'
echo "$2" >>"${BUILD_LOG:-/dev/null}"
exec g++ -std=c++17 -O0 -Wall -Wextra -I. "test_$2.cpp" -o "$1/t_$2"
SH
}

# config <name> '<JSON object merged over the defaults>'
config() {
  python3 -B - "$T/$1/cfg.json" "$2" <<'PY'
import json, sys
c = {"repo": ".", "root": "proj", "shared": [], "files": "src/*.cpp", "scope": "src/*.cpp",
     "excluded": {}, "equivalents_dir": "equiv", "setup": "sh setup.sh {build}",
     "build": "sh build.sh {build} {stem}", "test_file": "{build}/t_{stem}",
     "timeout_min": 2, "timeout_factor": 10, "timeout_build": 120, "threshold": 100,
     "file_threshold": 0, "deadcode": False, "stillborn": "exclude"}
c.update(json.loads(sys.argv[2]))
with open(sys.argv[1], "w") as f:
    json.dump(c, f, indent=1)
PY
}

# mutate <name> [args]: exit code in $RC, output in $T/out
mutate() {
  local name="$1"
  shift
  python3 -B "$MUTATE" --config "$T/$name/cfg.json" --workdir "$T/work" "$@" >"$T/out" 2>&1
  RC=$?
}

# report <name> <Python expression over r (the report) and M (its mutants)>
report() {
  python3 -B -c "import json, sys; r = json.load(open(sys.argv[1])); M = r['mutants']; print($2)" \
    "$T/$1/cfg.report.json"
}

# --- {stem} in build, per-stem baseline, confirmed timeout, --jobs 1 leaves CRLF sources alone
project basic
printf 'int f(int x) {\r\n  long long i = 0;\r\n  while (i < x) {\r\n    i++;\r\n  }\r\n  return (int)i;\r\n}\r\n' \
  >"$T/basic/proj/src/f.cpp"
# the failing test prints non-UTF-8 bytes: output decoding must never stop the runner
printf '#include "src/f.cpp"\n#include <cstdio>\nint main() { if (f(3) == 3 && f(0) == 0) return 0; std::fputs("\\xe4\\xff bad", stdout); return 1; }\n' \
  >"$T/basic/proj/test_f.cpp"
printf 'int g(int a) { return a + 1; }\n' >"$T/basic/proj/src/g.cpp"
printf '#include "src/g.cpp"\nint main() { return g(1) == 2 && g(-3) == -2 ? 0 : 1; }\n' >"$T/basic/proj/test_g.cpp"
config basic '{}'
cp -r "$T/basic/proj" "$T/basic/pristine"
BUILD_LOG="$T/basic/built.log" mutate basic --jobs 1
[ "$RC" -eq 0 ] || fail "basic run: exit $RC, expected 0"
[ "$(report basic "r['score']")" = "100.0" ] || fail "basic run: score"
[ "$(report basic "sum(m['status'] == 'timeout' for m in M)")" -ge 1 ] || fail "no confirmed timeout"
report basic "[m['detail'] for m in M if m['status'] == 'timeout'][0]" | grep -q "confirmed alone" ||
  fail "timeout without a solo confirmation"
[ "$(sort -u "$T/basic/built.log" | tr '\n' ' ')" = "f g " ] || fail "{stem} not substituted: $(sort -u "$T/basic/built.log")"
diff -r "$T/basic/proj" "$T/basic/pristine" >/dev/null || fail "--jobs 1 changed the checkout"
[ -z "$(ls -A "$T/work")" ] || fail "worker copies left in the workdir"
sleep 1
pgrep -f "$T/work" >/dev/null && fail "orphaned test process"
ok "{stem} in build, confirmed timeout, --jobs 1 leaves the CRLF checkout byte-identical"

# a stem whose unmutated test fails stops the run and is named
printf '#include "src/g.cpp"\nint main() { return 1; }\n' >"$T/basic/proj/test_g.cpp"
mutate basic --jobs 1
[ "$RC" -eq 2 ] || fail "failing baseline: exit $RC, expected 2"
grep -q "unmutated stage 1 test of 'g' fails" "$T/out" || fail "failing baseline: stem not named"
ok "per-stem baseline"

# --- directive lines and pointer declarators are never mutated
project decl
cat >"$T/decl/proj/src/d.cpp" <<'CPP'
#include <stdint.h>
  #define X 1
	#if X || 0
#define Y(a) \
  ((a) + 1)
int d(char *p, const char* q, int a, int b) {
  char *r = p;
  uint16_t* w = (uint16_t*)q;
  struct S* s = nullptr;
  (void)r; (void)w; (void)s;
  int y = a * b;
  return y;
}
	#endif
CPP
# rvalue references and template brackets are not mutated; the logical and after an enum value
# and a comparison after a cast are
cat >"$T/decl/proj/src/r.cpp" <<'CPP'
#include <array>
#include <cstdint>
enum class State { Idle, Busy };
struct Foo { int v; };
int r1(int&& i, uint8_t&& u, const Foo&& f, Foo const&& g, std::array<int, 2>&& v) {
  auto&& t = i;
  return t + u + f.v + g.v + v[0];
}
int r2(State s, bool x, int a, int b) {
  if (s == State::Idle && x) return 1;
  if (static_cast<int>(a) < b) return 2;
  return 0;
}
CPP
config decl '{}'
mutate decl --list
[ "$RC" -eq 0 ] || fail "--list: exit $RC"
grep -qE '^src/d\.cpp:(1|2|3|4|5|14):' "$T/out" && fail "a directive line was mutated"
[ "$(grep -c "'\*' -> '/'" "$T/out")" -eq 1 ] || fail "pointer declarators were mutated"
grep -q "^src/d.cpp:11:.* '\*' -> '/'" "$T/out" || fail "the product a * b was not mutated"
[ "$(grep -c "^src/r.cpp:.* '&&' -> '||'" "$T/out")" -eq 1 ] || fail "rvalue references were mutated"
grep -q "^src/r.cpp:10:24 log '&&' -> '||'" "$T/out" || fail "the '&&' after State::Idle was not mutated"
grep -q "^src/r.cpp:5:.* rel " "$T/out" && fail "the brackets of std::array<int, 2> were mutated"
[ "$(grep -c "^src/r.cpp:11:.* rel " "$T/out")" -eq 2 ] || fail "the brackets of static_cast<int> were mutated"
grep -q "^src/r.cpp:11:27 rel '<' -> '<='" "$T/out" && grep -q "^src/r.cpp:11:27 rel '<' -> '>='" "$T/out" ||
  fail "the comparison after static_cast<int>(a) was not mutated"
mutate decl --list --files src/d.cpp --lines 11-11
grep -vq '^src/d.cpp:11:\|mutants$' "$T/out" && fail "--lines kept other lines"
ok "directives, pointer and rvalue declarators and template brackets not generated, --lines"

# --- not_compiled: inactive #if and a dropped macro argument
project dead
cat >"$T/dead/proj/src/n.cpp" <<'CPP'
#define DROP(x)
int n(int a) {
  int r = a + 1;
#ifdef NEVER_DEFINED
  r = r * 3;
#endif
  DROP(r = r - 7);
  return r;
}
CPP
printf '#include "src/n.cpp"\nint main() { return n(1) == 2 ? 0 : 1; }\n' >"$T/dead/proj/test_n.cpp"
config dead '{"deadcode": true, "threshold": 0}'
mutate dead --jobs 2
[ "$RC" -eq 0 ] || fail "deadcode run: exit $RC"
[ "$(report dead "sorted({m['line'] for m in M if m['status'] == 'not_compiled'})")" = "[5, 7]" ] ||
  fail "not_compiled lines: $(report dead "sorted({m['line'] for m in M if m['status'] == 'not_compiled'})")"
[ "$(report dead "all(m['status'] == 'not_compiled' for m in M if m['line'] in (5, 7))")" = "True" ] ||
  fail "a mutant of an inactive line was built"
[ "$(report dead "all(m['status'] == 'killed' for m in M if m['line'] == 3)")" = "True" ] ||
  fail "a compiled line was classified not_compiled"
[ "$(report dead "r['totals']['run'] == sum(m['status'] not in ('not_compiled', 'equivalent') for m in M)")" = "True" ] ||
  fail "not_compiled mutants counted as run: $(report dead "r['totals']['run']")"
ok "not_compiled detection, not counted as run"

# --- a mutant that only warns (-Wparentheses) is built and counted; a compile error in the file
# and an undefined symbol are stillborn, and so is an error in a header that GCC locates in the
# file through its context: a template instantiated there ("required from"), a macro expanded
# there ("in expansion of macro") or the include chain ("In file included from")
project warn
cat >"$T/warn/proj/src/w.cpp" <<'CPP'
static_assert(sizeof(int) == 4, "int");
bool w(bool a, bool b, bool c) {
  if (a && b && c) return true;
  return false;
}
int missing();
int present() { return 7; }
int pick() {
  if constexpr (true)
    return present();
  else
    return missing();  // NOMUTATE: the discarded branch, only the mutant of the condition runs it
}
int fixed() { return Fixed<1>{}.v[0]; }
CHECKED(1);
constexpr int kSize = 4;
#include "wsize.h"
CPP
cat >"$T/warn/proj/src/wh.h" <<'CPP'
template <int N> struct Fixed { static_assert(N > 0, "N"); int v[N > 0 ? N : 1]; };
#define CHECKED(n) static_assert((n) > 0, "positive")
CPP
printf 'static_assert(kSize == 4, "size");\n' >"$T/warn/proj/src/wsize.h"
printf '#include "src/wh.h"\n#include "src/w.cpp"\nint main() { return w(true, true, true) && !w(false, true, true) && !w(true, false, true) && !w(true, true, false) && pick() == 7 ? 0 : 1; }\n' \
  >"$T/warn/proj/test_w.cpp"
config warn '{"threshold": 0}'
mutate warn --jobs 2
[ "$RC" -eq 0 ] || fail "warning run: exit $RC"
printf 'bool w(bool a, bool b, bool c) { if (a || b && c) return true; return false; }\n' |
  g++ -Wall -Wextra -fsyntax-only -x c++ - 2>&1 | grep -q "Wparentheses" || fail "g++ does not warn about the mutant"
[ "$(report warn "[m['status'] for m in M if m['line'] == 3 and m['col'] == 9 and m['replacement'] == '||'][0]")" = "killed" ] ||
  fail "the -Wparentheses mutant was not built and counted"
[ "$(report warn "all(m['status'] == 'stillborn' for m in M if m['line'] == 1)")" = "True" ] ||
  fail "the static_assert mutants are not stillborn"
report warn "[m['detail'] for m in M if m['line'] == 9 and m['replacement'] == 'false'][0]" | grep -q "undefined reference" ||
  fail "the undefined symbol is not stillborn: $(report warn "[(m['status'], m['detail']) for m in M if m['line'] == 9]")"
# the error lines of these mutants are in a header, only a context line names w.cpp
for check in "14:src/wh.h:required from" "15:src/wh.h:in expansion of macro" "16:src/wsize.h:In file included from"; do
  IFS=: read -r line header context <<<"$check"
  [ "$(report warn "sorted({(m['status'], m['detail'].split(':')[0]) for m in M if m['line'] == $line and m['op'] == 'const' and m['replacement'] == '0'})")" = "[('stillborn', '$header')]" ] ||
    fail "an error located in the file by '$context' is not stillborn: $(report warn "[(m['status'], m['detail']) for m in M if m['line'] == $line]")"
done
[ "$(report warn "r['totals']['counted'] == r['totals']['killed'] + r['totals']['survived'] + r['totals']['timeout']")" = "True" ] ||
  fail "stillborn mutants were counted"
ok "-Wparentheses mutant built and counted; compile errors located in the file and undefined symbols stillborn, excluded"

# --- a build that fails without an error in the mutated file (full disk, a killed linker, linker
# I/O, an error in another file) is an error: never stillborn, never cached, also when the mutant
# warns in the mutated file (return a -> return 0: unused parameter 'a')
project infra
printf 'int i1(int a) {\n  return a;\n}\n' >"$T/infra/proj/src/i.cpp"
printf '#include "src/i.cpp"\nint main() { return i1(4) == 4 ? 0 : 1; }\n' >"$T/infra/proj/test_i.cpp"
cat >"$T/infra/proj/build.sh" <<'SH'
if [ -n "${INFRA_MSG:-}" ] && ! grep -q "return a;" src/i.cpp; then
  g++ -std=c++17 -Wall -Wextra -fsyntax-only -I. "test_$2.cpp" 2>&1
  echo "$INFRA_MSG"
  exit 1
fi
exec g++ -std=c++17 -O0 -I. "test_$2.cpp" -o "$1/t_$2"
SH
config infra '{"threshold": 0}'
printf 'int i1(int a) {\n  return 0;\n}\n' | g++ -Wall -Wextra -fsyntax-only -x c++ - 2>&1 | grep -q "Wunused-parameter" ||
  fail "g++ does not warn about the mutant"
infra_case() {
  INFRA_MSG="$2" mutate infra --jobs 1
  [ "$RC" -eq 2 ] || fail "$1: exit $RC, expected 2"
  [ "$(report infra "[(m['status'], m['detail']) for m in M]")" = "[('error', '$3')]" ] ||
    fail "$1: $(report infra "[(m['status'], m['detail']) for m in M]")"
}
infra_case "full disk" "cc1plus: fatal error: error writing to /tmp/ccq.s: No space left on device" \
  "build failed: No space left on device"
infra_case "killed linker" "collect2: fatal error: ld terminated with signal 9 [Killed]" "build failed: terminated with signal"
infra_case "linker I/O" "ld: error: cannot open output file build/t_i: Input/output fault" \
  "build failed (exit 1) without a compiler error in src/i.cpp"
infra_case "an error in another file" "src/other.cpp:1:1: error: 'x' does not name a type" \
  "build failed (exit 1) without a compiler error in src/i.cpp"
mutate infra --jobs 1
[ "$RC" -eq 0 ] && grep -q "0 from the cache" "$T/out" || fail "an error result was cached"
[ "$(report infra "[m['status'] for m in M]")" = "['killed']" ] || fail "the rerun: $(report infra "[m['status'] for m in M]")"
ok "build failures of the environment are errors, also for a mutant that warns; not stillborn, not cached"

# --- exit codes of the glue runner: 124 (a case timed out) is a timeout that a solo re-run
# confirms, 125 (the runner failed) an error; neither is a kill by itself
project codes
printf 'int c1(int a) {\n  return a;\n}\nint c2(int a) {\n  return a;\n}\nint c3(int a) {\n  return a;\n}\n' >"$T/codes/proj/src/c.cpp"
printf '#include "src/c.cpp"\nint main() { if (c1(5) == 0) return 124; if (c2(5) == 0) return 125; return c3(5) == 5 ? 0 : 1; }\n' \
  >"$T/codes/proj/test_c.cpp"
config codes '{"threshold": 0}'
mutate codes --jobs 2
[ "$RC" -eq 2 ] || fail "runner exit codes: exit $RC, expected 2"
[ "$(report codes "[m['status'] for m in M]")" = "['timeout', 'error', 'killed']" ] ||
  fail "runner exit codes: $(report codes "[(m['status'], m['detail']) for m in M]")"
report codes "M[0]['detail']" | grep -q "a test case timed out (exit 124); confirmed alone" || fail "exit 124: detail"
report codes "M[1]['detail']" | grep -q "no test verdict (exit 125)" || fail "exit 125: detail"
ok "runner exit 124 is a timeout to confirm, 125 an error"

# --- equivalents: a valid entry, a stale entry, line-only and reason-less entries
project equiv
printf 'int e(int a) {\n  if (a > 5) return 5;\n  return a;\n}\n' >"$T/equiv/proj/src/e.cpp"
printf '#include "src/e.cpp"\nint main() { return e(3) == 3 && e(9) == 5 && e(6) == 5 ? 0 : 1; }\n' >"$T/equiv/proj/test_e.cpp"
config equiv '{"threshold": 0}'
EQ="$T/equiv/equiv/src__e.cpp.json"
echo '[{"line": 2, "col": 9, "original": ">", "replacement": ">=", "text": "if (a > 5) return 5;", "reason": "a == 5 returns 5 either way"}]' >"$EQ"
mutate equiv --jobs 2
[ "$RC" -eq 0 ] || fail "equivalents run: exit $RC"
[ "$(report equiv "[m['detail'] for m in M if m['status'] == 'equivalent']")" = "['a == 5 returns 5 either way']" ] ||
  fail "equivalent entry not applied"
grep -q "src/e.cpp:2:9 \`>\` -> \`>=\` (rel): a == 5 returns 5 either way" "$T/out" || fail "equivalent not reported with its reason"
printf '// shifted by one line\nint e(int a) {\n  if (a > 5) return 5;\n  return a;\n}\n' >"$T/equiv/proj/src/e.cpp"
mutate equiv --list
[ "$RC" -eq 2 ] || fail "stale entry: exit $RC, expected 2"
grep -q "src__e.cpp.json entry 1: stale" "$T/out" || fail "stale entry not named"
printf 'int e(int a) {\n  if (a > 5) return 5;\n  return a;\n}\n' >"$T/equiv/proj/src/e.cpp"
echo '[{"line": 2, "reason": "whole line"}]' >"$EQ"
mutate equiv --list
[ "$RC" -eq 2 ] || fail "line-only entry: exit $RC, expected 2"
echo '[{"line": 2, "col": 9, "original": ">", "replacement": ">=", "text": "if (a > 5) return 5;"}]' >"$EQ"
mutate equiv --list
[ "$RC" -eq 2 ] || fail "entry without a reason: exit $RC, expected 2"
echo '[{"line": 2, "col": 9, "original": ">", "replacement": ">=", "text": "if (a > 5) return 5;", "reason": " "}]' >"$EQ"
mutate equiv --list
[ "$RC" -eq 2 ] || fail "entry with an empty reason: exit $RC, expected 2"
rm "$EQ"
echo '[]' >"$T/equiv/equiv/src__gone.cpp.json"
mutate equiv --list
[ "$RC" -eq 2 ] && grep -q "src__gone.cpp.json" "$T/out" || fail "sidecar without a source not reported"
rm "$T/equiv/equiv/src__gone.cpp.json"
ok "equivalent entries: applied with reason; stale, line-only, reason-less and orphaned ones exit 2"

# --- NOMUTATE needs a reason
project marks
printf 'int b(int a) { return a + 1; }  // NOMUTATE\nint c(int a) { return a - 1; }\n' >"$T/marks/proj/src/b.cpp"
config marks '{}'
mutate marks --list
[ "$RC" -eq 2 ] || fail "bare NOMUTATE: exit $RC, expected 2"
grep -q "src/b.cpp:1: NOMUTATE without" "$T/out" || fail "bare NOMUTATE not named"
printf 'int b(int a) { return a + 1; }  // NOMUTATE: kept as an example\nint c(int a) { return a - 1; }\n' >"$T/marks/proj/src/b.cpp"
mutate marks --list
[ "$RC" -eq 0 ] || fail "NOMUTATE with a reason: exit $RC"
grep -q "^src/b.cpp:1:" "$T/out" && fail "a NOMUTATE line was mutated"
grep -q "^src/b.cpp:2:" "$T/out" || fail "the line after NOMUTATE lost its mutants"
ok "NOMUTATE with a reason suppresses the line, a bare one exits 2"

# --- scope: every file of the scope is listed or excluded with a reason
project scope
printf 'int a1(int a) { return a + 1; }\n' >"$T/scope/proj/src/a.cpp"
printf 'int h1(int a) { return a + 1; }\n' >"$T/scope/proj/src/h.cpp"
config scope '{"files": ["src/a.cpp"]}'
mutate scope --list
[ "$RC" -eq 2 ] || fail "scope: exit $RC, expected 2"
grep -q "src/h.cpp: in the scope" "$T/out" || fail "scope: unlisted file not named"
config scope '{"files": ["src/a.cpp"], "excluded": {"src/h.cpp": "generated"}}'
mutate scope --list
[ "$RC" -eq 0 ] || fail "scope with an exclusion: exit $RC"
config scope '{"files": ["src/a.cpp"], "excluded": {"src/h.cpp": ""}}'
mutate scope --list
[ "$RC" -eq 2 ] || fail "exclusion without a reason: exit $RC, expected 2"
ok "scope check"

# --- kill -9 during a run leaves the checkout byte-identical (CRLF sources)
project kill
printf 'int k(int a) {\r\n  int r = a * 3 + 1;\r\n  if (r > 10) r -= 2;\r\n  return r;\r\n}\r\n' >"$T/kill/proj/src/k.cpp"
printf '#include "src/k.cpp"\n#include <unistd.h>\nint main() { usleep(300000); return k(1) == 4 && k(4) == 11 ? 0 : 1; }\n' \
  >"$T/kill/proj/test_k.cpp"
config kill '{"threshold": 0}'
cp -r "$T/kill/proj" "$T/kill/pristine"
python3 -B "$MUTATE" --config "$T/kill/cfg.json" --workdir "$T/killwork" --jobs 1 >"$T/out" 2>&1 &
pid=$!
for _ in $(seq 1 100); do
  [ -n "$(ls "$T/killwork" 2>/dev/null)" ] && break
  sleep 0.1
done
sleep 3
kill -9 "$pid"
wait "$pid" 2>/dev/null
sleep 1
diff -r "$T/kill/proj" "$T/kill/pristine" >/dev/null || fail "kill -9 changed the checkout"
cmp -s "$T/kill/proj/src/k.cpp" "$T/kill/pristine/src/k.cpp" || fail "kill -9 changed the CRLF source"
ok "kill -9 leaves the checkout byte-identical"

# --- thresholds: 80 % overall passes 60, a file at 50 % misses file_threshold 95
project gate
printf 'int g1(int a) {\n  return a;\n}\nint g2(int a) {\n  return a;\n}\nint g3(int a) {\n  return a;\n}\n' >"$T/gate/proj/src/good.cpp"
printf '#include "src/good.cpp"\nint main() { return g1(5) == 5 && g2(5) == 5 && g3(5) == 5 ? 0 : 1; }\n' >"$T/gate/proj/test_good.cpp"
printf 'int h1(int a) {\n  return a;\n}\nint h2(int a) {\n  return a;\n}\n' >"$T/gate/proj/src/half.cpp"
printf '#include "src/half.cpp"\nint main() { return h1(5) == 5 && h2(0) == 0 ? 0 : 1; }\n' >"$T/gate/proj/test_half.cpp"
config gate '{"threshold": 60, "file_threshold": 95}'
mutate gate --jobs 2
[ "$RC" -eq 1 ] || fail "thresholds: exit $RC, expected 1"
[ "$(report gate "r['score']")" = "80.0" ] || fail "thresholds: overall score $(report gate "r['score']")"
grep -q "files below the per-file threshold: src/half.cpp (50.0 %)" "$T/out" || fail "the 50 % file is not named"
ok "threshold 60 met, file_threshold 95 missed by the 50 % file: exit 1 naming it"

# --- stage 2 runs only for the survivors of stage 1; --no-fallback is a quick run, not a gate
project stages
printf 'int s(int a) { return a * 2; }\n' >"$T/stages/proj/src/s.cpp"
printf '#include "src/s.cpp"\nint main(int argc, char**) { return argc > 1 ? (s(3) == 6 ? 0 : 1) : (s(0) == 0 ? 0 : 1); }\n' \
  >"$T/stages/proj/test_s.cpp"
config stages '{"test": "{build}/t_{stem} slow"}'
mutate stages --jobs 2
[ "$RC" -eq 0 ] || fail "stages: exit $RC"
[ "$(report stages "all(m['status'] == 'killed' and m['stage'] == 2 for m in M)")" = "True" ] || fail "stage 2 did not kill"
mutate stages --jobs 2 --no-cache --no-fallback
[ "$RC" -eq 0 ] || fail "--no-fallback: exit $RC"
grep -q "quick run, stage 1 only, not a gate" "$T/out" || fail "--no-fallback not reported as a quick run"
[ "$(report stages "r['quick'] and r['score'] == 0.0")" = "True" ] || fail "--no-fallback ran stage 2"
ok "stage 2 for stage-1 survivors, --no-fallback"

# --- a timeout caused by load only (a CPU hog in a parallel worker) is an error, not a kill; a
# mutant that times out under load and fails its tests alone is killed, with the time of both runs
project hog
printf 'int u1(int a) {\n  return a;\n}\nint u2(int a) {\n  return a;\n}\nint u3(int a) {\n  return a;\n}\n' \
  >"$T/hog/proj/src/u.cpp"
cat >"$T/hog/proj/test_u.cpp" <<'CPP'
#include "src/u.cpp"
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <unistd.h>
// Mutant of u1: a CPU hog for 3 s (inside the 4 s budget). Mutants of u2 and u3: sleep past their
// budget only while the hog runs; alone they end after 3 s, u2 passing, u3 failing.
int main() {
  const char* flag = getenv("HOG_FLAG");
  if (u1(5) == 0) {
    FILE* f = fopen(flag, "w");
    if (f) fclose(f);
    const auto end = std::chrono::steady_clock::now() + std::chrono::seconds(3);
    volatile unsigned long spin = 0;
    while (std::chrono::steady_clock::now() < end) spin = spin + 1;
    remove(flag);
    return 1;
  }
  const bool u2Mutant = u2(5) == 0;
  if (u2Mutant || u3(5) == 0) {
    for (int i = 0; i < 60; ++i) {
      if (access(flag, F_OK) == 0) {
        sleep(30);
        return 0;
      }
      usleep(50000);
    }
    return u2Mutant ? 0 : 1;
  }
  return 0;
}
CPP
config hog '{"threshold": 0, "timeout_min": 4}'
HOG_FLAG="$T/hog/flag" mutate hog --jobs 3
[ "$RC" -eq 2 ] || fail "load-only timeout: exit $RC, expected 2"
[ "$(report hog "[m['status'] for m in M]")" = "['killed', 'error', 'killed']" ] ||
  fail "statuses $(report hog "[(m['status'], m['detail']) for m in M]")"
report hog "M[1]['detail']" | grep -q "unconfirmed timeout" || fail "error without the unconfirmed-timeout detail"
report hog "M[2]['detail']" | grep -q "exceeded 4.0 s under load; alone killed after" || fail "kill after a timeout: detail"
[ "$(report hog "M[2]['seconds'] >= 7")" = "True" ] || fail "the first run of a re-run timeout was not counted"
ok "unconfirmed timeout under a CPU hog is an error; killed alone is a kill, timed with both runs"

# --- cached kills belong to the tests and the tool that made them: after a change of the tests or
# of mutate.py they run again, and the cache file keeps the results of the current tree only
project cache
printf 'int c(int a) {\n  return a + 1;\n}\n' >"$T/cache/proj/src/c.cpp"
printf '#include "src/c.cpp"\nint main() { return c(1) == 2 ? 0 : 1; }\n' >"$T/cache/proj/test_c.cpp"
config cache '{"threshold": 0}'
mutate cache --jobs 2
[ "$RC" -eq 0 ] && [ "$(report cache "sorted({m['status'] for m in M})")" = "['killed']" ] || fail "cache: first run"
mutate cache --jobs 2
grep -q "from the cache, 0 to run" "$T/out" || fail "cache: the kills of an unchanged tree were not reused"
cp "$MUTATE" "$T/mutate.py"
echo "# another version of the tool" >>"$T/mutate.py"
python3 -B "$T/mutate.py" --config "$T/cache/cfg.json" --workdir "$T/work" --jobs 2 >"$T/out" 2>&1
grep -q " 0 from the cache" "$T/out" || fail "cache: kills of another mutate.py reused"
printf '#include "src/c.cpp"\nint main() { return c(1) > 0 ? 0 : 1; }\n' >"$T/cache/proj/test_c.cpp"
mutate cache --jobs 2
grep -q " 0 from the cache" "$T/out" || fail "cache: kills reused after the test changed"
[ "$(report cache "sum(m['status'] == 'survived' for m in M)")" -eq 2 ] || fail "cache: the weaker test killed $(report cache "[m['status'] for m in M]")"
entries="$(python3 -B -c "import json, sys; print(len(json.load(open(sys.argv[1]))))" "$T/cache/cfg.cache.json")"
[ "$entries" -eq 2 ] || fail "cache: $entries entries, expected the 2 kills of the current tree"
ok "a changed test or mutate.py invalidates the cached kills; the cache keeps the current tree only"

# --- --changed-since needs git; without it (the native image) it exits 2 and says so
mkdir -p "$T/nogit"
PY3="$(command -v python3)"
PATH="$T/nogit" "$PY3" -B "$MUTATE" --config "$T/cache/cfg.json" --changed-since HEAD >"$T/out" 2>&1
RC=$?
[ "$RC" -eq 2 ] && grep -q "needs git" "$T/out" || fail "--changed-since without git: exit $RC"
ok "--changed-since without git exits 2"

# --- every config validates against the one schema; unknown and missing keys are errors
for c in "$HERE"/*.json; do
  case "$c" in *.cache.json | *.report.json) continue ;; esac
  python3 -B -c "import sys; sys.path.insert(0, sys.argv[1]); import mutate; mutate.load_config(sys.argv[2])" \
    "$HERE" "$c" >"$T/out" 2>&1 || fail "$(basename "$c") does not validate"
done
config basic '{"timeout": 60}'
mutate basic --list
[ "$RC" -eq 2 ] && grep -q "unknown key 'timeout'" "$T/out" || fail "unknown key accepted"
python3 -B -c "import json, sys; c = json.load(open(sys.argv[1])); del c['stillborn']; json.dump(c, open(sys.argv[1], 'w'))" "$T/basic/cfg.json"
mutate basic --list
[ "$RC" -eq 2 ] && grep -q "missing key 'stillborn'" "$T/out" || fail "missing key accepted"
ok "configs validate: $(cd "$HERE" && ls ./*.json | grep -v 'cache\|report' | xargs -n1 basename | tr '\n' ' ')"

[ -d "$HERE/__pycache__" ] && fail "__pycache__ written next to mutate.py"
echo "mutate.py selftest OK"
