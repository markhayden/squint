#!/usr/bin/env bash
# Publishes a reviewed set (docs/evalsets.md). The owner has confirmed every draft label (or
# struck items that were then rebuilt and confirmed) and gives the attestation text. Steps: mark
# the drafts confirmed with today's date, promote, copy the images into the evalsets clone and
# commit them there, pin that commit in the manifest, place the set under evalsets/, seal with the
# attestation, check and verify. For a blind set the seal moves the labels out to owner storage
# (`<owner>/<set id>/labels.toml`) and the manifest keeps only their hash. Pushing the evalsets
# clone and committing here are separate steps.
#
#   scripts/sets-publish.sh <use-case>/<set> "<attestation>"
set -euo pipefail
cd "$(dirname "$0")/.."
set_id="${1:?usage: sets-publish.sh <use-case>/<set> \"<attestation>\"}"
attest="${2:?attestation text}"
owner="${SQUINT_OWNER_DIR:-$HOME/squint-owner}"
clone="${SQUINT_EVALSETS_DIR:-$(cd .. && pwd)/wafflery-evalsets}"
build="$owner/build/evalsets/$set_id"
draft="$owner/$set_id/labels-draft.toml"
[ -d "$build" ] || { echo "no built set at $build" >&2; exit 1; }
[ -d "$clone/.git" ] || { echo "no evalsets clone at $clone" >&2; exit 1; }
today="$(date +%F)"
sed -i.bak "s/status = \"draft\"/status = \"confirmed\"/g; s/adjudicated_on = \"\"/adjudicated_on = \"$today\"/g" "$draft" && rm -f "$draft.bak"
(cd scripts/sets && uv run --frozen --offline --no-sync squint-sets promote --draft "$draft" --set "$build")
# Images into the evalsets clone, committed there.
mkdir -p "$clone/$set_id"
(cd "$build" && find . -name '*.png' | while IFS= read -r f; do mkdir -p "$clone/$set_id/$(dirname "$f")"; cp "$f" "$clone/$set_id/$f"; done)
(cd "$clone" && git add -A "$set_id" && git commit -q -m "$set_id: $(find "$build" -name '*.png' | wc -l | tr -d ' ') images" && git log --oneline -1)
rev="$(cd "$clone" && git rev-parse HEAD)"
# The set into this repository, pinned to that commit.
mkdir -p "evalsets/$(dirname "$set_id")"
rm -rf "evalsets/$set_id"
cp -R "$build" "evalsets/$set_id"
python3 - "evalsets/$set_id/set.toml" "$rev" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1]); s = p.read_text()
line = f'images_rev = "{sys.argv[2]}"\n'
if "images_rev" in s:
    s = "\n".join(line.rstrip("\n") if l.startswith("images_rev") else l for l in s.splitlines()) + "\n"
else:
    s = s.replace('images_repo = "github.com/markhayden/wafflery-evalsets"\n', 'images_repo = "github.com/markhayden/wafflery-evalsets"\n' + line)
p.write_text(s)
PY
if grep -q '^split = "blind"' "evalsets/$set_id/set.toml"; then
  cargo run -q -p squint-cli -- sets seal --set "evalsets/$set_id" --attest "$attest" --labels-out "$owner/$set_id"
else
  cargo run -q -p squint-cli -- sets seal --set "evalsets/$set_id" --attest "$attest"
fi
cargo run -q -p squint-cli -- sets check evalsets
cargo run -q -p squint-cli -- sets verify-seal --set "evalsets/$set_id"
echo "published $set_id at $rev; push the evalsets clone, add the row to evalsets/README.md, commit here"
