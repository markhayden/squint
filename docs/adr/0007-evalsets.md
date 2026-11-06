# 0007: Evaluation sets: manifest in the repo, images elsewhere by hash

Date: 2026-10-25. Status: accepted.

## Context

An item is a candidate plus zero or more references, a label, optional defect regions and a
split. The repo is Apache-2.0 and ships only reviewed, private-data-free assets. The method needs
the same sets rerun with and without tools, and honest blind gates.

## Decision

A set is a directory with `set.toml` under `evalsets/<use-case>/<set>/`; every image is verified
by SHA-256 before a run. Public images live in a separate `wafflery-evalsets` repository fetched
by hash; private sets are referenced by path. Blind labels are committed as a hash before any
model output exists. Evaluation runs never read the cache. First sets are public, generated with
image models with defects planted by construction and labelled by hand: 20 items per use case,
growing toward 50 to 100. The generating models and licence are decided with the first set and
recorded per item.

## Alternatives

- **Git LFS in this repo.** Quotas, slow clones, and publication on commit removes review.
- **A repo per set.** Clean licensing, but fetch steps and manifest drift.
- **Private lab set first.** Rejected: it would shape the harness around one use case.

## Consequences

- A set-building milestone precedes the baseline.
- `squint sets check` and `squint sets fetch` are first-milestone commands.

## Refined by spec reviews 1 to 4 (2026-10-25)

Items are complete jobs plus separately stored labels; splits are `dev`, `heldout` and `blind`
with family separation; arms are A, C (zoom plus `align` under a `visual_only` projection), D_X
(C plus one tool) and D (bundle). Gates freeze `on_unsure = pass`, `on_error = review`, an error
ceiling, margins, a family-clustered paired bootstrap with a minimum of five differing families and
Bonferroni-adjusted percentiles; `inconclusive` is an outcome. First admission on a sealed blind
set, confirmation on a distinct heldout set of at least 50. Owner approved the defaults as
provisional on 2026-10-25.
