#!/usr/bin/env bash
# Self-test of mutate.py: an infinite-loop mutant must time out, be counted as killed,
# leave no orphaned process behind, and the source file must be restored byte-exactly.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
mkdir -p "$T/src"
echo 'int f(int x){ int i=0; while (i < x) { i++; } return i; }' > "$T/src/f.cpp"
cp "$T/src/f.cpp" "$T/orig.cpp"
printf '#include "src/f.cpp"\nint main(){ return (f(3)==3 && f(0)==0) ? 0 : 1; }\n' > "$T/t.cpp"
cat > "$T/cfg.json" <<JSON
{"repo": ".", "root": ".", "files": ["src/f.cpp"], "test": "g++ -O0 t.cpp -o t && ./t", "timeout": 3, "threshold": 100}
JSON
python3 "$HERE/mutate.py" --config "$T/cfg.json" > "$T/out.txt" || { cat "$T/out.txt"; echo "selftest: expected 100% score"; exit 1; }
grep -q '"status": "timeout"' "$T/cfg.report.json" || { echo "selftest: no timeout mutant observed"; exit 1; }
cmp -s "$T/src/f.cpp" "$T/orig.cpp" || { echo "selftest: source not restored"; exit 1; }
sleep 1
if pgrep -f "^$T/t$|^./t$" >/dev/null; then echo "selftest: orphaned test process"; exit 1; fi
echo "mutate.py selftest OK"
