# Architecture

The authoritative description is `SPEC.md`; this page is the short version kept current as code
lands. Filled progressively from M1 onward; the crate graph is live from task 0.1.

## Shape

```
            config.toml   rules/*.toml   job (images, requirements, brief)
                 │             │              │
                 ▼             ▼              ▼
┌──────────────────────────────────────────────────────────────┐
│ squint-core                                                 │
│  config → obligations → instances per target → checklist      │
│  prompt → decision (1..n runs of the state machine)           │
│  validation → aggregation → verdict → journal + report        │
│  cache (decision boundary) · ledger · redaction · frames      │
└───────────┬───────────────────────────────┬──────────────────┘
            │ Provider trait                 │ Tool trait
            ▼                                ▼
┌───────────────────────┐        ┌──────────────────────────────┐
│ squint-providers     │        │ squint-tools                │
│  anthropic · gemini    │        │  built-ins (Rust): view,      │
│  openai adapters       │        │   resize, edge, change_map,   │
│  endpoint profiles     │        │   align                       │
│  model catalog         │        │  mcp client (2025-11-25) ─────┼──▶ tools/python (squint-tools)
│  wire schemas          │        │  manifest registry, envelope  │        regions, classes, later ocr…
│  injected HTTP client  │        └──────────────────────────────┘
└───────────────────────┘
            ▲
            │ recorded fixtures replace HTTP in tests
┌───────────────────────┐        ┌──────────────────────────────┐
│ squint-eval          │        │ squint (crates/squint-cli)│
│  sets, seal, matrix    │        │  check · eval · tools · sets  │
│  descriptor, ledger    │        │  report · catalog · setup     │
│  metrics, gates        │        └──────────────────────────────┘
└───────────────────────┘
```

## Principles

- **One authority.** The verdict is computed in code from the model's structured submission and
  the code-owned checklist. Tool outputs are evidence. The model's opinion is recorded, never used.
- **Tools point, the model decides.** No tool output is a hard gate.
- **Operational failure is error.** A provider or tool failure, a budget overrun or an unrepaired
  model-output failure invalidates the run; it never becomes a verdict.
- **Nothing silent.** No silent resize, model fallback, cache hit in an evaluation, or change of
  auth or upstream path.
- **Trust boundary.** Config, rules, catalog, profiles and approved manifests are trusted. Image
  content, filenames, labels, the brief and tool observations are data.
- **The core is a library.** It takes an HTTP client and a tool transport as inputs.

## Crate graph

The allowed edges are in `scripts/crate-graph.txt` and enforced by `just check`. `squint-core`
depends on no workspace crate; `squint-testkit` is a production dependency of the binary only
behind the `testkit` feature, and a dev-dependency anywhere.

## Status

| Area | State | Filled by |
|---|---|---|
| Workspace, checks, CI | done | 0.1, 0.2 |
| Core domain: job, images, frames, config, obligations, checklist, submission, aggregation, journal, report | done (1.1 to 1.6) | M1 |
| Run state machine: tool turns, final turn (`refuse`), terminal error classes, repairs, retries, cost ledger, artifacts, cancellation, report rebuild and prune, decision cache, isolation and the environment allowlist | done (1.7a to 1.12) | M1 |
| Provider adapters, catalog, profiles | catalog with 7 models and 3 API-key profiles, resolve-time capability validation, `catalog show|list` (2.1); wire schema transformations per adapter with resolve-time representability and limit checks (2.2); deterministic image preparation (`prep_version` 1) and request preflight against catalog limits (2.3); consent binding, credential resolution (`error: auth`), the HTTPS client refusing cross-host redirects, request hygiene scan over recorded fixtures (2.4); the transport seam with the replay player and recorder, and the Anthropic Messages adapter with thinking echoed, strict tools and the three final-turn mechanisms (2.5a); the Gemini generateContent adapter with function-call pairing and verbatim part echo (2.6a) and the OpenAI Responses adapter with strict tools, `store = false` and reasoning items echoed (2.7a); fifteen Anthropic, thirteen Gemini and thirteen OpenAI conversations recorded and replaying (2.5b, 2.6b, 2.7b; tag `m2.1`); provider-data rows for every shipped profile, checked by a catalog test (2.9); embedded assets packaged with their crates, publish dry run, install from the packaged crates and the offline smoke in the `package` workflow (2.10a); `dist` release archives for three architectures with the archive smoke gating the GitHub release, the release dry run in the `package` workflow, one example config per use case, and the live smoke through the three direct profiles from an unpacked archive (2.10b) | M2 |
| Evaluation sets, runner, metrics, gates, viewer | statistics contract frozen (1.15: bootstrap, Wilson, gate rules, ADR 0016); set manifests, families and labels parsed strictly, `sets check` over a tree, `sets fetch` by hash from a directory or URL, `sets seal` and `verify-seal` by component (3.1); the set-building toolkit with scene and subject families, draft labels from construction, rebuild and promote (3.2a); five dev sets and five adversarial sets built, reviewed and sealed (3.2b, 3.3a to 3.3e, tag `m3.1`); matrix parsing with the frozen identity, the execution descriptor and single-writer lock, the durable evaluation ledger with crash recovery, the admission record (3.4a); `eval run`: every cell with the cache bypassed, immutable decision directories, resume skipping complete decisions and marking partial ones, exact arm membership, `summary.json` (3.4b); `eval estimate` from the run's own envelope planning (3.4c); metrics per cell with Wilson intervals, localisation under the eligibility rules, `metrics.json` and `report.md` written by every run and regenerated byte for byte by `eval report` (3.5a); the gate lifecycle: seal and descriptor verification, receipts, repeat-scoring refusal under an ADR reference, label publication and split change for the owner to commit, provisional and confirmed admissions, resume from the receipt (3.5b); the static viewer served on loopback by `report view`, locations drawn in raw coordinates through the frame tree the report now records (3.6); `eval canary` against the baseline `eval requalify` records: clear, suspect, recommend_disable, never disabling (3.7); baseline pending | M3 |
| Built-in tools, MCP client | MCP client lifecycle done (1.13); the alignment estimators for flat and equirectangular pairs with measured tolerances and failure fixtures, ADR 0014 (4.0); evaluation projections declared in manifests and applied by the registry before the run sees an envelope, recorded on the report's calls (4.1); the `view` and `resize` built-ins with the sphere geometry that maps view frames back to a panorama, registered and listed (4.2a); the viewer drawing view crops and `view` locations as footprints on a panorama through the same geometry (4.2b); the `align` built-in for flat and equirectangular pairs with aligned frames the registry maps through the inverse transform, `visual_only` declared (4.3a, 4.3b); `edge_similarity` and `change_map` over frames in one grid, with the stored golden map (4.3c, tag `m4.1`); the multiplexed MCP client with paginated listing guards, concurrent calls where declared, `outputSchema` validation, exact content blocks, the frame chain in the context and concurrent batch dispatch in the run, ADR 0015 (4.4a); the failure modes each under test and packs shared across an evaluation under leases keyed by decision (4.4b); the segmenter export with its provenance record, MODEL.md complete (4.6); arm C on the dev sets pending | M4 |
| Python tool pack | provisioning bundle, setup and offline launch proven with `echo_region` (1.14); the server reads `_squint` images and frames with their pixels, returns envelopes with images as MCP image blocks, answers `squint/reset`, and the Rust client drives it in a test (5.1); `fetch` downloads and verifies the segmenter and its goldens, the parity test holds the artifact to the recorded tolerance (5.3); `regions` finds where an aligned pair differs from photometric change and class disagreement, boxed, labelled and scored, with every manifest passing the adapters' wire checks (5.4a); `classes` names what the segmenter sees in a box before and after, with a `class_difference` region when the dominant class changes (5.4b); the bundle built from the real pack, `tools setup` running the pack's `fetch` after the sync, and the lock digest in the cache key (5.2, tag `m5.1`); the source-fidelity blind set built from a seeded plan in owner storage, reviewed by the owner and sealed with its labels held out (5.5); the initial regions gate scored against those labels and rejected, with the control detecting everything, the receipt and results published and the set now heldout (5.6) | M5 |
| Contract conformance (table-manners v0.1.0) | the contract pinned and its schemas hash-checked (7.1); image references and `expect` (7.2); findings as contract locations on the file as stored, split at the seam (7.3); `run.json` on every decision, the error mapping, exits 64, 130, 143 (7.4); `describe --json` (7.5); `estimate` and `--max-spend-usd` (7.6); `squint serve` on rmcp with an independent test client (7.7a), checks over MCP with roots and a session budget (7.7b), progress, cancellation and runs as resources (7.8); fixtures held in CI and in table-manners (7.10); the pack client kept in-house, ADR 0019 (7.11); `squint playground` hands off to `wafflehouse --tool squint` (7.9, wafflehouse H1). [`docs/agents.md`](agents.md) | M7 |
