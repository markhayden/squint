#!/usr/bin/env bash
# Warns, never fails, when table-manners has a release newer than the one squint pins
# (schemas/contract/SOURCES.toml; ADR 0018). Run in CI; an update is a deliberate commit.
# CONTRACT_TAGS_FILE replaces the network listing with one tag per line (for tests).
set -euo pipefail
cd "$(dirname "$0")/.."
pinned="$(sed -n 's/^tag = "\(.*\)"$/\1/p' schemas/contract/SOURCES.toml)"
if [[ -n "${CONTRACT_TAGS_FILE:-}" ]]; then
  tags="$(cat "$CONTRACT_TAGS_FILE")"
elif ! tags="$(git ls-remote --tags --refs https://github.com/markhayden/table-manners.git 2>/dev/null | sed 's#.*refs/tags/##')"; then
  echo "::warning::could not list table-manners tags; pinned $pinned"
  exit 0
fi
newest="$(grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' <<< "$tags" | sort -V | tail -1 || true)"
if [[ -n "$newest" && "$newest" != "$pinned" && "$(printf '%s\n%s\n' "$pinned" "$newest" | sort -V | tail -1)" == "$newest" ]]; then
  echo "::warning::table-manners $newest is newer than the pinned $pinned"
else
  echo "contract tag ok: $pinned is the newest release"
fi
