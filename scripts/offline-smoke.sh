#!/usr/bin/env bash
# The offline smoke for an installed `squint` binary: version, the two inspection commands and a
# live check with no credential, which must stop `not ready`, naming the key, before anything
# leaves the machine. Runs from a scratch directory, never from the repository tree.
#
#   scripts/offline-smoke.sh /path/to/squint
set -euo pipefail
bin="${1:?usage: offline-smoke.sh <squint binary>}"
bin="$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"
cat > smoke.toml <<'TOML'
[provider]
profile = "anthropic"
model = "claude-haiku-5-5"

[privacy]
cloud_calls = true
account = "anthropic:smoke"

[rules]
base = "source-fidelity"

[output]
dir = "./out"
TOML

version="$("$bin" --version)"
case "$version" in squint\ *) ;; *) echo "unexpected --version output: $version" >&2; exit 1 ;; esac
"$bin" tools list --config smoke.toml > /dev/null
# Captured first: `grep -q` stops reading at its first match, and a binary still writing then
# fails on the broken pipe.
rules="$("$bin" rules show --config smoke.toml)"
grep -q '^SF1 ' <<<"$rules" || { echo "rules show did not print the shipped base" >&2; exit 1; }
set +e
err="$(env -u ANTHROPIC_API_KEY "$bin" check --config smoke.toml --candidate a.png --source b.png --use-case source-fidelity 2>&1 >/dev/null)"
code=$?
set -e
if [ "$code" -ne 64 ] || ! grep -q 'not ready: ANTHROPIC_API_KEY missing' <<< "$err"; then
  echo "check without a credential: expected exit 64 not ready for ANTHROPIC_API_KEY, got exit $code: $err" >&2; exit 1
fi
# The report viewer, run as a host runs a server render (table-manners 3.2): its stdin held open
# by a pipe from a sleeping writer (a background job's stdin is /dev/null, and end of file stops
# the viewer at once), its only stdout the URL line, framed for the one origin given, and ended by
# SIGTERM with exit 0. Neither render may change the run directory.
mkdir -p view-dir
echo '{"report_version":1}' > view-dir/report.json
snapshot() { (cd view-dir && find . -print | LC_ALL=C sort && find . -type f -exec cksum {} + | LC_ALL=C sort); }
before="$(snapshot)"
mkfifo view.in
sleep 600 > view.in &
hold_pid=$!
"$bin" report view view-dir --port 0 --frame-ancestor http://127.0.0.1:4801 < view.in > view.out 2> view.err &
view_pid=$!
# The viewer and the writer are waited for before the work directory goes: removing it while the
# viewer is still exiting can find the directory not yet empty.
trap '{ kill "$view_pid" "$hold_pid"; wait "$view_pid" "$hold_pid"; } 2>/dev/null || true; rm -rf "$work"' EXIT
for _ in $(seq 1 50); do
  [ "$(wc -l < view.out)" -ge 1 ] && break
  sleep 0.1
done
[ "$(wc -l < view.out)" -ge 1 ] || { echo "report view printed no URL: $(cat view.out view.err)" >&2; exit 1; }
url="$(head -n1 view.out)"
[[ "$url" =~ ^http://127\.0\.0\.1:[0-9]+/$ ]] || { echo "report view printed more than its URL: $(cat view.out)" >&2; exit 1; }
curl -fsS "$url" | grep -q '<title>squint report</title>' || { echo "report view did not serve the viewer at $url" >&2; exit 1; }
curl -fsS "${url}report.json" | grep -q '"report_version":1' || { echo "report view did not serve report.json" >&2; exit 1; }
curl -fsSI "$url" | grep -qi '^content-security-policy:.*frame-ancestors http://127\.0\.0\.1:4801' || { echo "report view did not allow framing by the origin given" >&2; exit 1; }
[ "$(wc -l < view.out)" -eq 1 ] || { echo "report view printed more than its URL: $(cat view.out)" >&2; exit 1; }
kill -TERM "$view_pid"
set +e
wait "$view_pid"
code=$?
set -e
[ "$code" -eq 0 ] || { echo "report view on SIGTERM: expected exit 0, got exit $code" >&2; exit 1; }
{ kill "$hold_pid"; wait "$hold_pid"; } 2>/dev/null || true
# The export, as a host runs a document render: one self-contained page outside the run
# directory (the cache unless --out says otherwise), its absolute path the only stdout.
page="$(XDG_CACHE_HOME="$work/cache" "$bin" report export view-dir 2> export.err)" || { echo "report export failed: $(cat export.err)" >&2; exit 1; }
case "$page" in /*) ;; *) echo "report export printed no absolute path: $page" >&2; exit 1 ;; esac
[ "$(printf '%s\n' "$page" | wc -l)" -eq 1 ] && [ -f "$page" ] || { echo "report export printed more than the path of a page: $page" >&2; exit 1; }
case "$page" in "$work/view-dir"/*) echo "report export wrote inside the run directory: $page" >&2; exit 1 ;; esac
grep -q 'globalThis.SQUINT_BUNDLE = ' "$page" || { echo "report export wrote no self-contained page" >&2; exit 1; }
[ "$(snapshot)" = "$before" ] || { echo "a render changed the run directory" >&2; exit 1; }
echo "offline smoke ok: $version"
