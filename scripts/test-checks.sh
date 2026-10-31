#!/usr/bin/env bash
# Proves the repository checks fail when they should. Run by `just check`.
set -euo pipefail
cd "$(dirname "$0")/.."
pass=0; fail=0

# crate graph: an empty allowed-edge file must fail and list every edge.
empty="$(mktemp)"; : > "$empty"
if CRATE_GRAPH_FILE="$empty" scripts/check-crate-graph.sh >/dev/null 2>&1; then
  echo "FAIL: crate-graph check accepted an empty graph" >&2; fail=$((fail+1))
else pass=$((pass+1)); fi

# crate graph: one removed edge must fail and name it.
one="$(mktemp)"; grep -v 'squint-eval -> squint-providers' scripts/crate-graph.txt > "$one"
out="$(CRATE_GRAPH_FILE="$one" scripts/check-crate-graph.sh 2>&1 || true)"
if grep -q 'unexpected prod edge: squint-eval -> squint-providers' <<< "$out"; then pass=$((pass+1));
else echo "FAIL: crate-graph check did not name the missing edge" >&2; fail=$((fail+1)); fi
rm -f "$empty" "$one"

# check-filter: a filter that selects nothing must fail; an unknown package must fail loudly.
if scripts/check-filter.sh squint-core no_such_test_name >/dev/null 2>&1; then
  echo "FAIL: check-filter accepted an empty selection" >&2; fail=$((fail+1)); else pass=$((pass+1)); fi
if scripts/check-filter.sh no-such-package foo >/dev/null 2>&1; then
  echo "FAIL: check-filter accepted an unknown package" >&2; fail=$((fail+1)); else pass=$((pass+1)); fi

# check-packaging: an include that reaches outside its crate must fail and name it; an embedded
# file the manifest excludes must fail as "not packaged".
probe="$(mktemp -d)"; mkdir -p "$probe/crate/src" "$probe/crate/assets"
printf 'x = 1\n' > "$probe/outside.toml"; printf 'y = 2\n' > "$probe/crate/assets/a.toml"
printf '[package]\nname = "probe"\nversion = "0.0.0"\nedition = "2024"\nexclude = ["assets/"]\n' > "$probe/crate/Cargo.toml"
printf 'pub const A: &str = include_str!("../../outside.toml");\n' > "$probe/crate/src/lib.rs"
out="$(CRATE_DIRS="$probe/crate" scripts/check-packaging.sh 2>&1 || true)"
if grep -q 'reaches outside the crate' <<< "$out"; then pass=$((pass+1));
else echo "FAIL: check-packaging accepted an include outside the crate: $out" >&2; fail=$((fail+1)); fi
printf 'pub const A: &str = include_str!("../assets/a.toml");\n' > "$probe/crate/src/lib.rs"
out="$(CRATE_DIRS="$probe/crate" scripts/check-packaging.sh 2>&1 || true)"
if grep -q 'embedded but not packaged' <<< "$out"; then pass=$((pass+1));
else echo "FAIL: check-packaging accepted an excluded asset: $out" >&2; fail=$((fail+1)); fi
printf '[package]\nname = "probe"\nversion = "0.0.0"\nedition = "2024"\n' > "$probe/crate/Cargo.toml"
if CRATE_DIRS="$probe/crate" scripts/check-packaging.sh >/dev/null 2>&1; then pass=$((pass+1));
else echo "FAIL: check-packaging rejected a packaged asset" >&2; fail=$((fail+1)); fi
rm -rf "$probe"

# archive-smoke: an archive with no squint binary must fail and say so; a bad checksum file must
# fail before anything is unpacked.
probe="$(mktemp -d)"; mkdir -p "$probe/squint-x"; printf 'readme\n' > "$probe/squint-x/README.md"
tar cJf "$probe/squint-x.tar.xz" -C "$probe" squint-x
out="$(scripts/archive-smoke.sh "$probe/squint-x.tar.xz" 2>&1 || true)"
if grep -q 'no squint binary inside' <<< "$out"; then pass=$((pass+1));
else echo "FAIL: archive-smoke accepted an archive without the binary: $out" >&2; fail=$((fail+1)); fi
printf '%s  squint-x.tar.xz\n' "$(printf '0%.0s' $(seq 64))" > "$probe/squint-x.tar.xz.sha256"
out="$(scripts/archive-smoke.sh "$probe/squint-x.tar.xz" 2>&1 || true)"
if grep -q 'checksum mismatch' <<< "$out"; then pass=$((pass+1));
else echo "FAIL: archive-smoke accepted a wrong checksum: $out" >&2; fail=$((fail+1)); fi
rm -rf "$probe"

# check-contract-tag: a newer table-manners tag is a warning naming it, and never a failure.
tags="$(mktemp)"; printf 'v0.1.0\nv0.10.0\nv0.2.0\nnot-a-version\n' > "$tags"
if out="$(CONTRACT_TAGS_FILE="$tags" scripts/check-contract-tag.sh 2>&1)" && grep -q 'table-manners v0.10.0 is newer' <<< "$out"; then pass=$((pass+1));
else echo "FAIL: check-contract-tag did not warn about v0.10.0: $out" >&2; fail=$((fail+1)); fi
printf 'v0.1.0\n' > "$tags"
if out="$(CONTRACT_TAGS_FILE="$tags" scripts/check-contract-tag.sh 2>&1)" && grep -q 'is the newest release' <<< "$out"; then pass=$((pass+1));
else echo "FAIL: check-contract-tag warned with no newer tag: $out" >&2; fail=$((fail+1)); fi
rm -f "$tags"

# check-packaging: the shipped assets are listed as embedded by the crates that own them.
out="$(scripts/check-packaging.sh 2>&1)"
for want in "squint-core: embeds .*prompts/v1.md" "squint-core: embeds .*rules/source-fidelity.toml" "squint-providers: embeds catalog.toml" "squint-cli: embeds viewer/geometry.js, viewer/index.html, viewer/timeline.js, viewer/viewer.css, viewer/viewer.js"; do
  if grep -Eq "$want" <<< "$out"; then pass=$((pass+1));
  else echo "FAIL: check-packaging did not list '$want': $out" >&2; fail=$((fail+1)); fi
done

# offline-smoke: a binary that answers every command but exits 3 instead of 64 on the missing
# credential must fail, one that prints no SF1 from `rules show` must fail, one whose
# `report view` serves nothing must fail, and one whose `report export` writes no bundle must fail.
probe="$(mktemp -d)"
cat > "$probe/squint" <<'STUB'
#!/usr/bin/env bash
case "$1" in
  --version) echo "squint 0.0.0 (stub)" ;;
  tools) ;;
  rules) echo "${STUB_RULES-SF1 must_hold candidate}" ;;
  check) echo "auth: set ANTHROPIC_API_KEY" >&2; exit "${STUB_CHECK_EXIT-64}" ;;
  report)
    # `report export <dir> --out <page>`: write a page carrying the bundle, as the binary does.
    if [ "$2" = export ]; then
      [ "${STUB_EXPORT-bundle}" = bundle ] && echo 'globalThis.SQUINT_BUNDLE = {};' > "$5"
      exit 0
    fi
    # `report view <dir> --port 0`: serve the directory on loopback and print a URL line, as the
    # binary does; python's server prints its own URL in the same shape.
    [ "${STUB_VIEW-serve}" = serve ] || exit 0
    echo '<title>squint report</title>' > "$3/index.html"
    exec python3 -I -u -m http.server --bind 127.0.0.1 --directory "$3" 0 ;;
esac
STUB
chmod +x "$probe/squint"
if scripts/offline-smoke.sh "$probe/squint" >/dev/null 2>&1; then pass=$((pass+1));
else echo "FAIL: offline-smoke rejected a conforming stub" >&2; fail=$((fail+1)); fi
out="$(STUB_CHECK_EXIT=3 scripts/offline-smoke.sh "$probe/squint" 2>&1 || true)"
if grep -q 'expected exit 64' <<< "$out"; then pass=$((pass+1));
else echo "FAIL: offline-smoke accepted exit 3 on the missing credential: $out" >&2; fail=$((fail+1)); fi
out="$(STUB_RULES="nothing" scripts/offline-smoke.sh "$probe/squint" 2>&1 || true)"
if grep -q 'did not print the shipped base' <<< "$out"; then pass=$((pass+1));
else echo "FAIL: offline-smoke accepted a rules show without SF1: $out" >&2; fail=$((fail+1)); fi
out="$(STUB_VIEW=none scripts/offline-smoke.sh "$probe/squint" 2>&1 || true)"
if grep -q 'printed no URL' <<< "$out"; then pass=$((pass+1));
else echo "FAIL: offline-smoke accepted a report view that serves nothing: $out" >&2; fail=$((fail+1)); fi
out="$(STUB_EXPORT=none scripts/offline-smoke.sh "$probe/squint" 2>&1 || true)"
if grep -q 'wrote no self-contained page' <<< "$out"; then pass=$((pass+1));
else echo "FAIL: offline-smoke accepted a report export that wrote no bundle: $out" >&2; fail=$((fail+1)); fi
rm -rf "$probe"

# check-attribution: a file with an assistant trailer or a "Generated with" line fails; a plain
# co-author passes. The patterns are assembled at run time so this file never contains a match.
probe="$(mktemp)"; who="Cla""ude"; addr="noreply@anthro""pic.com"; generated="Generated ""with ["
printf 'feat: x\n\nCo-Authored-By: %s Example <%s>\n' "$who" "$addr" > "$probe"
if scripts/check-attribution.sh "$probe" >/dev/null 2>&1; then echo "FAIL: check-attribution accepted an assistant trailer" >&2; fail=$((fail+1)); else pass=$((pass+1)); fi
printf 'feat: x\n\n%sTool](https://example.invalid)\n' "$generated" > "$probe"
if scripts/check-attribution.sh "$probe" >/dev/null 2>&1; then echo "FAIL: check-attribution accepted a Generated-with line" >&2; fail=$((fail+1)); else pass=$((pass+1)); fi
printf 'feat: x\n\nCo-Authored-By: A Person <person@example.invalid>\n' > "$probe"
if scripts/check-attribution.sh "$probe" >/dev/null 2>&1; then pass=$((pass+1)); else echo "FAIL: check-attribution rejected a human co-author" >&2; fail=$((fail+1)); fi
rm -f "$probe"

# results-doc: the generator at least compiles.
if python3 -I -m py_compile scripts/results-doc.py 2>/dev/null; then pass=$((pass+1));
else echo "FAIL: scripts/results-doc.py does not compile" >&2; fail=$((fail+1)); fi

echo "check tests: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
