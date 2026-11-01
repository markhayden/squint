# Implementation plan: squint harness

Status: draft 4, 2026-10-26, written from `SPEC.md` draft 5 (approved) and revised against Codex
plan reviews 1 to 3 (`docs/reviews/plan-review-*.md`). Review 3 judged draft 3 ready with
changes and split them into before-M0 (all made here) and fix-in-the-owning-task items (noted in
the tasks that own them). The task checklist that tracks progress
is `tasks/todo.md`; this file holds the reasoning, the order and the commit strategy. When the
spec changes, this plan changes in the same commit.

## Overview

Build a Rust binary that runs a vision model as a judge over an image job, with deterministic
tools as evidence and the verdict computed in code, then prove each tool earns its place on
public labelled sets. The order follows the brief's method: a skeleton that runs end to end
offline, real provider adapters with recorded fixtures, public evaluation sets and a tool-less
baseline, the built-in zoom and alignment tools as the equal-visual-access control, then the first
ML tool scored by a predeclared gate on a sealed set, and a confirmation stage before any tool is
called admitted.

| Milestone | Delivers | Tag |
|---|---|---|
| M0 Scaffold | workspace, tool bootstrap, CI, lint, docs skeleton, README sections, owner readiness | `m0` |
| M1 Skeleton | `squint check` end to end against a scripted fake provider, every state, offline; minimal MCP client, minimal provisioning proof, frozen statistics | `m1` |
| M2 Adapters | privacy enforcement; Anthropic, Gemini, OpenAI adapters with fixtures; catalog; image prep; live smoke; packaging and distribution | `m2` |
| M3 Sets and baseline | set tooling, five public dev sets plus five adversarial sets, eval runner with execution descriptor and durable ledger, metrics, gate lifecycle, viewer, arm A baseline | `m3` |
| M4 Built-in tools | alignment spike, Tool registry, `view`/`align`/`resize`/`edge_similarity`/`change_map`, full MCP client, arm C, MODEL.md | `m4` |
| M5 First ML tool | Python pack, provisioning bundle, `regions` and `classes`, sealed blind set, initial gate | `m5` |
| M7 Contract conformance | `squint serve`, `describe --json`, `run.json`, contract locations and image references, shared exit codes, `squint playground` against `table-manners` v0.1.0 (ADR 0018, SPEC "External interface") | `m7` |

Deferred past M7 with their own ADRs: an HTTP transport for `squint serve`, the mask handoff
from rascal (region-scoped `allowed` obligations), Bedrock and Vertex profiles, the ChatGPT-plan
OAuth scheme, scheduled canaries in CI, Vercel and Cloudflare profiles if their fixtures do not
pass during M2.

## Crate dependency graph

```
squint-core        no dependency on other workspace crates; owns job, images, frames, obligations,
                     checklist, prompt, state machine, submission, aggregation, decision, journal,
                     report, ledger, cache, and the Provider and Tool traits with their request,
                     response, manifest and envelope types
squint-providers   depends on core; adapters, wire schemas, profiles, catalog, auth, fixtures player
squint-tools       depends on core; built-ins, MCP client, provisioning, projections
squint-eval        depends on core, providers, tools; sets, seal, matrix, descriptor, eval ledger,
                     metrics, bootstrap, gates
squint-testkit     depends on core; scripted fake provider, fake tool, fake MCP server binary
squint (package)   directory crates/squint-cli; package name and binary name are both `squint`;
                     every selector in this plan is `-p squint-cli`; depends on core, providers, tools,
                     eval; optional dependency on testkit behind the non-default feature `testkit`
```

The `testkit` feature is the one declared production edge from the CLI to testkit, because an
optional dependency is still a dependency for publication: `squint-testkit` is a published
package, and the publication order is `squint-core`, `squint-testkit`, `squint-providers`,
`squint-tools`, `squint-eval`, `squint`. Ordinary and release builds do not enable the
feature and reject `--provider-script` as an unknown flag; that rejection is tested against a
separately built feature-off executable, since a feature-on binary cannot prove it. Dev-dependency
edges (every crate's tests may use testkit) are declared separately from production edges, and the
crate-graph check runs with all features enabled. `just check` runs both feature configurations:
`cargo nextest run --workspace --features squint-cli/testkit` and, for the only crate with a
feature, `cargo nextest run -p squint-cli` without it, plus Clippy the same two ways; every filtered test invocation in this plan must enumerate and execute at least one test
(`--list` compared in the `just` recipe). Availability of all six package names on crates.io is
confirmed in 0.1 and a fallback recorded by ADR if needed.

## Task dependency edges

Each task's **Dependencies** line is the authority; this list is generated from those lines and
must match them. Milestone barriers apply on top: no task in M(n+1) starts before `m(n)` is
tagged.

```
M0  0.1 ← none        0.2 ← 0.1        0.3 ← none        0.4 ← none
M1  1.1 ← 0.1         1.2 ← 1.1        1.3 ← 1.1         1.4a ← 1.3
    1.4b ← 1.4a, 1.2  1.5a ← 1.1       1.5b ← 1.2, 1.4b, 1.5a
    1.6a ← 1.5b       1.6b ← 1.6a      1.7a ← 1.6b       1.7b ← 1.7a
    1.8 ← 1.7b        1.9 ← 1.8        1.10a ← 1.9       1.10b ← 1.10a
    1.11 ← 1.10b      1.12 ← 1.11      1.13 ← 1.12, 1.5a 1.14 ← 1.13      1.15 ← 0.1
M2  2.1 ← 1.3, 0.4    2.2 ← 1.6a, 2.1  2.3 ← 1.2, 2.1    2.4 ← 1.3, 1.10a
    2.5a ← 2.2, 2.3, 2.4, 1.8           2.5b ← 2.5a
    2.6a ← 2.5a       2.6b ← 2.6a      2.7a ← 2.5a       2.7b ← 2.7a
    2.8 ← 2.5b, 2.6b (optional)         2.9 ← 2.5b, 2.6b, 2.7b
    2.10a ← 2.5b, 2.6b, 2.7b            2.10b ← 2.10a
M3  3.1 ← 1.4b        3.2a ← 3.1, 0.4  3.2b ← 3.2a       3.3a..d ← 3.2b    3.3e ← 3.2b
    3.4a ← 1.9, 1.11, 3.1               3.4b ← 3.4a       3.4c ← 3.4b
    3.5a ← 3.4b, 1.15 3.5b ← 3.5a, 3.1 3.6 ← 1.10b       3.7 ← 3.4b
    3.8 ← 2.5b, 2.6b, 2.7b, 3.3a, 3.3b, 3.3c, 3.3d, 3.3e, 3.4c, 3.5a
M4  4.0 ← 1.2, 0.4    4.1 ← 1.5a, 1.7a 4.2a ← 4.1, 1.2   4.2b ← 4.2a, 3.6
    4.3a ← 4.0, 4.1   4.3b ← 4.3a, 4.2a                  4.3c ← 4.3a
    4.4a ← 1.13, 4.1  4.4b ← 4.4a      4.5 ← 4.2a, 4.3b, 4.3c, 3.8        4.6 ← 0.4
M5  5.1 ← 4.4b, 1.14  5.3 ← 4.6, 5.1   5.4a ← 5.3        5.4b ← 5.3
    5.2 ← 5.3, 5.4a, 5.4b               5.5 ← 3.2a, 3.1   5.6 ← 5.4a, 5.5, 4.5, 3.5b, 5.2
M7  7.1 ← table-manners v0.1.0 on crates.io            7.2 ← 7.1         7.3 ← 7.2
    7.4 ← 7.2, 7.3    7.5 ← 7.1        7.6 ← 7.5         7.7a ← 7.5, 7.6
    7.7b ← 7.7a, 7.4  7.8 ← 7.7b       7.9 ← 7.1, wafflehouse H1
    7.10 ← 7.4, 7.5, 7.8                7.11 ← 7.7a
M8  8.1 ← table-manners v0.2-contract    8.2 ← 8.1    8.3 ← 8.2    8.4 ← 8.1
    8.5 ← 8.3                            8.6 ← 8.2–8.5, then v0.2.0 published
M9  9.1 ← table-manners v0.3 branch      9.2 ← 9.1    9.3 ← 9.2    9.4 ← 9.3
    9.5 ← 9.4         9.6 ← 9.3          9.7 ← 9.1    9.8 ← 9.1    9.9 ← 9.1
    9.10 ← 9.2–9.9                       9.11 ← 9.10, then v0.3.0 published
FU  F1 ← none (or 9.5)   F2 ← none   F3 ← none (or 9.6)   F4 ← none
    F5 ← none (or 9.6)   F6 ← none
```

Every M1 task names a CLI scenario that demonstrates its behaviour through the binary using only
what precedes it.

## Commit strategy

**Initial commit.** Before M0, one documentation commit on `main` with `SPEC.md`, `PLAN.md`,
`tasks/todo.md`, `docs/adr/`, `docs/reviews/` and `.gitignore`; `docs/brief.md` stays excluded
and `git check-ignore docs/brief.md` is the test.

**Branches.** `main` holds only reviewed checkpoints. Each milestone is one branch `mN-<name>`
from the previous checkpoint tag. Tasks are committed on the milestone branch, or on short task
branches `mN/<task>` merged into the milestone branch when two tasks are in flight. Code work on
a later milestone never starts before the earlier milestone merges; owner labelling happens
outside git and may overlap. A milestone merges into `main` with a merge commit (no squash) after
the owner's review and the checkpoint checklist passes. Nothing is pushed by the agent; the owner
pushes. The owner's preference (2026-10-26): branches stay on the owner's machine and `main` is
pushed only at tagged checkpoints, so the public repository shows reviewed, working states; CI is
run locally with `just ci-local` before any push.

**Commits.** Conventional Commits with a scope. Every commit on a milestone branch leaves
`just check` green, which runs `cargo fmt --all -- --check`, Clippy with `-D warnings` and the
tests (`cargo nextest run`) over the workspace with `squint-cli/testkit` and over `squint-cli`
without it, and `cargo deny check`; a commit that cannot
is not made. `Cargo.lock` is committed. The
spec, plan, README and docs are updated in the same commit as the change they describe.
Attribution lines follow the repository's conventions. Secrets, private images and
`docs/brief.md` never enter a commit.

**Checkpoints and tags.** Every later checkpoint also requires `just ci-local` green. When a
milestone's checkpoint checklist passes and the owner approves,
the merge commit on `main` is tagged `mN` (annotated, with the checklist and the versions of
external artifacts: evalsets repository commit, model artifact hashes, published package
versions). Intermediate tags `mN.k` mark task groups on the milestone branch.

**Rollback.**

1. Revert one task with `git revert`; when later tasks depend on it, revert the dependents together
   in reverse dependency order and verify the suite before committing the revert.
2. Reset a milestone branch to an intermediate tag only with a clean tree and after
   `git branch rescue/<branch>-<date>`; never on `main`.
3. Abandon an unmerged milestone by deleting the branch after the rescue reference exists.
4. Undo a merged milestone with a reviewed `git revert -m 1 <merge commit>` on `main`; never a
   force push.

External state is not rolled back by git: published set images, released packages and model
artifacts are versioned and recorded in checkpoint tags; a blind set whose labels were exposed is
never reused; spent money is recorded in results documents. `eval-out/` and run directories are
never committed; results live under `docs/results/`.

**Reviews.** This plan is reviewed by Codex before M0 begins. Each milestone's diff is reviewed
with the review and test skills (`/agent-skills:review`, `/agent-skills:test`) available in the
owner's environment; where they are not available, `docs/review-checklist.md` (written in 0.3)
is applied by hand. The owner reviews every checkpoint.

## Milestone M0: Scaffold

### Task 0.1: Workspace, crate skeletons and tool bootstrap

**Description.** The workspace with the six crates from the crate graph, each with a `lib.rs`
or `main.rs`, a crate-level `thiserror` enum and one trivial test; `rust-toolchain.toml` pinning
stable with the `clippy` and `rustfmt` components; `docs/bootstrap.md` giving the one manual step
(`cargo install just --version <pinned>`), then `just bootstrap` installing pinned `cargo-deny`
and `cargo-dist` (and `uv` if absent) and `just check` running fmt, Clippy in both feature
configurations, `cargo test --workspace`, `cargo test --workspace --features squint-cli/testkit`,
deny, the crate-graph check with all features, and a `--list` check that every named filtered
suite executes at least one test; `deny.toml` with the allowed-licence list; the `testkit` feature
wiring; availability of all six package names on crates.io checked and recorded (ADR 0012 if a
fallback is needed).

**Acceptance.**
- [ ] After the documented `just` install, `just bootstrap && just check` passes on a clean clone.
- [ ] The crate-graph check distinguishes production and dev edges and fails CI on any edge
      outside the graph.
- [ ] `cargo run -p squint-cli -- --version` prints version and short commit; a separately built
      feature-off executable rejects `--provider-script`; the feature-on build lists it.

**Verification.** `cargo install just --version <pinned> && just bootstrap && just check`.

**Dependencies.** None. **Files.** `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `deny.toml`,
`justfile`, `scripts/check-crate-graph.sh`, `crates/*/`. **Size.** M.

### Task 0.2: CI, lint and pre-commit

**Description.** GitHub Actions running `just check`, the crate-graph check, and the Python checks
once `tools/python` exists. A pre-commit hook installed by `just hooks` that runs fmt and clippy
and refuses staged content matching credential-shaped patterns (full-shape regexes such as
`sk-ant-[A-Za-z0-9_-]{24,}`, `AIza[0-9A-Za-z_-]{35}`, `sk-proj-[A-Za-z0-9_-]{24,}`,
`AKIA[0-9A-Z]{16}`, `ghp_[A-Za-z0-9]{36}`). No file is exempt; instead `scripts/synthetic-tokens.txt`
enumerates the exact synthetic token values the fixtures and the hook test use, and only those
values are allowed anywhere. `.gitignore` for `target/`, `eval-out/`, `squint-out/`,
`*.local.toml`, `.venv/`.

**Acceptance.**
- [ ] CI is green on the M0 branch.
- [ ] Hook test: accepts the hook's own source, the documentation examples and the enumerated
      synthetic tokens; rejects a realistic-shaped credential in an ordinary staged file and the
      same token inside the hook test or the fixtures directory; accepts a clean commit.
- [ ] `git check-ignore docs/brief.md` exits 0 and `git ls-files docs/brief.md` prints nothing.

**Verification.** `just hooks && scripts/test-hooks.sh`; draft PR with CI.

**Dependencies.** 0.1. **Files.** `.github/workflows/ci.yml`, `scripts/hooks/pre-commit`,
`scripts/scan-secrets.sh`, `scripts/test-hooks.sh`, `scripts/test-checks.sh`,
`fixtures/synthetic-secrets/`, `.gitignore`. **Size.** S.

### Task 0.3: Docs skeleton, review checklist and README sections

**Description.** `docs/architecture.md` from the spec; stubs for `docs/tools.md`, `docs/rules.md`,
`docs/evalsets.md`, `docs/viewer.md`, `docs/provider-data.md`, `docs/results/README.md`, each
naming its filling task; `docs/review-checklist.md` (the five-axis review and the test-coverage
checks, for use when the skills are unavailable); README "How it works" and "How it is built"
from the spec with links to SPEC, PLAN, ADRs and reviews.

**Acceptance.**
- [ ] README reads correctly for an empty harness and says nothing is measured yet.
- [ ] Every stub names its filling task; the review checklist exists.

**Dependencies.** None. **Files.** `README.md`, `docs/*.md`. **Size.** S.

### Task 0.4: Owner readiness

**Description.** `docs/owner-decisions.md` recording, before any money is spent: the initial
judge-model subset (recommendation: one model per vendor at first); which Anthropic account
(enforced-thinking or not) records fixtures; spend caps and approval points for fixture recording,
release and gateway smoke tests, image generation, the baseline, arm C, the initial gate and the
confirmation gate; the labelling plan (worksheet format, batch size, adjudicator); which prototype
images, outputs and artifacts are approved for reuse.
Generators and licence are deferred to 3.2a. Blank entries name the task they block.

**Acceptance.**
- [ ] Every entry filled or marked "decide before task X"; each paid task in this plan cites its
      entry.

**Dependencies.** None. **Files.** `docs/owner-decisions.md`. **Size.** S; owner time.

### Checkpoint M0
- [ ] `just check` green locally and on a fresh clone; `just ci-local` (the GitHub workflow run in
      Docker with act) green; hooks installed; README accurate; owner decisions recorded.
- [ ] Owner review; merge; tag `m0`.

## Milestone M1: Skeleton

### Task 1.1: Walking slice

**Description.** The thinnest valid end-to-end path: a job with a candidate and a source by path,
decoded without bounds checks (1.2 adds them); one hard-coded mandatory obligation `SF2` with
scope `candidate` resolving to one instance; a one-item checklist; a minimal system prompt; the
`Provider` trait exactly as the spec writes it with a scripted fake in `squint-testkit`
returning a `submit_verdict` whose single item is `held` or `violation`; schema-light validation;
aggregation of that one row; a journal with `resolved`, `turn_sent`, `turn_received`,
`submission`, `verdict`; a minimal `report.json`; exit codes 0 and 1; the `testkit` feature and
`--provider-script` flag.

**CLI scenario.** `cargo run -p squint-cli --features testkit -- check --config fixtures/config/minimal.toml --job fixtures/jobs/pair.toml --provider-script fixtures/scripts/held.json` exits 0; with `violation.json` exits 1; both write a journal and report; a build without the feature rejects the flag.

**Acceptance.**
- [ ] Both scenarios and the feature-off rejection pass in tests; the report shows the instance,
      disposition and row; the journal is append-only JSONL and the report is derived from it.

**Verification.** `cargo test -p squint-cli --features testkit --test walking_slice`.

**Dependencies.** 0.1. **Files.** `crates/squint-core/src/{job,provider,run,journal,report}.rs`,
`crates/squint-testkit/src/provider.rs`, `crates/squint-cli/src/main.rs`, fixtures.
**Size.** M. Tag `m1.1`.

### Task 1.2: Canonical images and frames registry

**Description.** Decode bounds, EXIF orientation, sRGB 8-bit, original and canonical SHA-256,
projection from field or GPano with the 2:1 warning, base64 sources; the frames registry with
`raw`, `aligned`, `crop` types, the location record and validity table, flat mapping to the root
with clipping and the `partial` flag (equirectangular maths in 4.2a).

**CLI scenario.** `check` on a job with an EXIF-rotated candidate reports upright dimensions and
hashes equal to `shasum -a 256`; a 60 MP image is refused with `error: resolve` naming the bound.

**Acceptance.**
- [ ] Oversized images refused before full decode; EXIF applied; hashes match; validity table
      enforced by a table test; crop-to-raw round trip within 1e-6; wholly outside is an error and
      partly outside is flagged.

**Verification.** `cargo test -p squint-core -- image frames`.

**Dependencies.** 1.1. **Files.** `crates/squint-core/src/{image,frames}.rs`, `fixtures/images/`.
**Size.** M.

### Task 1.3: Config loading, validation and the shorthand job

**Description.** The full `squint.toml` schema, `schemas/config-v1.json`, `${VAR}` from the
environment only, CLI overrides as repeatable `--set path=value`, credential-name refusal in pack
`env`, the `cloud_calls` gate,
redacted resolved config in the report, a built-in `fake` profile for tests, and the shorthand
`check --candidate … --ref name=path … --brief …` producing the same resolved job as the
equivalent `--job` file.

**CLI scenario.** `check --config full.toml --set budget.tool_turns=2` shows the override; the
same with `--set privacy.cloud_calls=false` refuses; the shorthand and `--job` forms produce
byte-identical resolved-job sections in their reports.

**Acceptance.**
- [ ] Every key in the spec example parses; unknown keys fail; the schema file regenerated by a
      test equals the committed one; a secret in the environment appears in no output path;
      shorthand equivalence fixture passes.

**Verification.** `cargo test -p squint-core -- config` and `cargo test -p squint-cli --features testkit --test shorthand`.

**Dependencies.** 1.1. **Files.** `crates/squint-core/src/config.rs`, `crates/squint-cli/src/args.rs`,
`schemas/config-v1.json`. **Size.** M.

### Task 1.4a: Obligation model, merge and validation

**Description.** Obligation types and fields, rules files with `extends` and profiles, consumer
files, job requirements, merge order, every validation rule in the spec except those needing the
tool registry (evidence-kind existence is checked in 1.5b).

**CLI scenario.** `squint rules show --config …` prints the merged list with provenance; a
consumer file with a bad `waives` id fails naming the rule.

**Acceptance.**
- [ ] Table test with one failing fixture per validation rule; the spec's generic and
      architectural files merge to the documented result.

**Verification.** `cargo test -p squint-core -- obligations`.

**Dependencies.** 1.3. **Files.** `crates/squint-core/src/obligations.rs`,
`crates/squint-cli/src/rules.rs`. **Size.** M.

### Task 1.4b: Instances, rendering and shipped rule sets

**Description.** Scope and evidence resolution to targets, instances, the empty-expansion,
no-mandatory-instance and unconsulted-reference errors, the Obligations section rendering, the
five base rule sets and the architectural profile embedded, and a rendering fixture for the
architectural profile covering `waives = []`, SF4 reporting and the `allowed_change` assertions.

**CLI scenario.** `check --dry-run` on the Pip job prints the rendered Obligations section and
the instance list.

**Acceptance.**
- [ ] Each error case tested; rendering snapshots committed and byte-stable; all six shipped
      files load and validate.

**Verification.** `cargo test -p squint-core -- instances render`.

**Dependencies.** 1.4a, 1.2 (the Pip job uses a base64 source). **Files.**
`crates/squint-core/src/{instances,render}.rs`, `rules/`. **Size.** M.

### Task 1.5a: Manifest and envelope types

**Description.** The `Tool` trait, manifest type, model schema versus input schema, `_squint`
type, result envelope with `returns` bounds, evidence kinds, projection type, and a scripted fake
tool in `squint-testkit`; `squint tools list`.

**CLI scenario.** `squint tools list --config …` prints the fake tool's manifest as the model
will see it.

**Acceptance.**
- [ ] Manifest missing a required field refused; envelope over any bound is `error: tool`; model
      argument containing `_squint` is `invalid_args`; `schemas/{manifest,envelope}-v1.json`
      regenerated equal committed.

**Verification.** `cargo test -p squint-core -- manifest envelope`.

**Dependencies.** 1.1. **Files.** `crates/squint-core/src/tool/{manifest,envelope}.rs`,
`crates/squint-testkit/src/tool.rs`, `crates/squint-cli/src/tools.rs`. **Size.** M.

### Task 1.5b: Checklist and evidence binding

**Description.** Static items, evidence items by `evidence_kind` and root image, the
non-empty-dependency invariant, drops journaled, the evidence budget error, call-order ordering,
overlap cross-references, and the evidence-kind validation deferred from 1.4a. Exercised through
unit tests and `--dry-run` only; the tool-driven CLI scenario belongs to 1.7a.

**CLI scenario.** `check --dry-run` on the Pip job prints the static checklist items and their
dependencies.

**Acceptance.**
- [ ] Pip example yields the documented items; unit tests: three regions returned, one dropped
      for empty dependencies, two presented as items; the 25th region is `error: budget`; overlap
      cross-reference rendered.

**Verification.** `cargo test -p squint-core -- checklist`.

**Dependencies.** 1.2, 1.4b, 1.5a. **Files.** `crates/squint-core/src/checklist.rs`. **Size.** M.

### Task 1.6a: Submission schema and semantic validation

**Description.** Canonical submission types, `schemas/submission-v1.json`, `jsonschema`
validation, every semantic rule with a failing fixture each. Unit-level; the repair-driven CLI
scenario belongs to 1.8.

**CLI scenario.** `check` with a script whose submission is valid passes; a script whose
submission omits an item exits 3 with `error: submission_invalid` (repair not yet wired, so the
first failure is terminal until 1.8).

**Acceptance.**
- [ ] One failing fixture per semantic rule (unknown item, missing item, violation without
      location, inapplicable exemption, `observed` citing a mandatory id, evidence from a failed
      call, wrong location kind for the projection, mapping wholly outside).

**Verification.** `cargo test -p squint-core -- submission validate`.

**Dependencies.** 1.5b. **Files.** `crates/squint-core/src/{submission,validate}.rs`,
`schemas/submission-v1.json`. **Size.** M.

### Task 1.6b: Aggregation and decision tables

**Description.** Per-instance rows, strictest-row combination, verdict, decision table with
evidence matching, `[policy]` on the exit code only.

**CLI scenario.** `check` with `decision_runs = 2` and two scripts that disagree on a static
item exits 2 with both runs in the report.

**Acceptance.**
- [ ] Exhaustive test over disposition × confidence × (`matters`, `hidden_ok`) matches the spec
      table row by row; all 16 decision pairs tested; matching eligibility, ordering and the
      `held` exception tested.

**Verification.** `cargo test -p squint-core -- aggregate decision`.

**Dependencies.** 1.6a. **Files.** `crates/squint-core/src/{aggregate,decision}.rs`. **Size.** M.

### Task 1.7a: State machine: tool turns

**Description.** Resolve, prepare (stub sizing), reserve (stub ledger), tool turns with batch
validation before execution, call budget exhaustion with error results, serial execution (concurrent dispatch for `concurrent = true` packs lands in 4.4a)
execution, status handling, the versioned prompt template in `prompts/v1.md` with the Tools
section from manifests.

**CLI scenario.** `check` with a script of two tool turns (fake tool returning three regions,
one dropped) then a submission shows both turns, all calls and two evidence items in the report.

**Acceptance.**
- [ ] Scenarios: first look then submit; two tool turns; batch over budget answered with error
      results; fake tool crash is `error: tool`.
- [ ] Provider-history invariant test: accepted blocks, opaque payloads, system prompt and tool
      definitions unchanged across requests; new content appended in provider-valid order;
      request-level controls change only as the catalog permits; tool definitions identical on
      every request.

**Verification.** `cargo test -p squint-core -- run::tool_turns`.

**Dependencies.** 1.6b. **Files.** `crates/squint-core/src/{run,prompt}.rs`, `prompts/v1.md`.
**Size.** M.

### Task 1.7b: State machine: final turn and terminal states

**Description.** The `refuse` final-turn mechanism, stray-call error results, `no_submission`,
every `error:` class reachable, operational error irreversibility. Repair is a single terminal
failure here; 1.8 adds the retry of each class.

**CLI scenario.** A script that keeps calling tools exhausts `tool_turns`, receives the final
instruction, calls once more, gets the error result, and ends `error: no_submission`; a script
whose tool crashes and then submits validly still ends `error: tool`.

**Acceptance.**
- [ ] Both scenarios tested; every error class implemented by this task (`resolve`, `tool`,
      `provider`, `budget` stub, `no_submission`, `submission_invalid` as single failure) has a
      scripted path; journal order asserted. Classes added by later tasks are tested there.

**Verification.** `cargo test -p squint-core -- run::final_turn run::errors`.

**Dependencies.** 1.7a. **Files.** `crates/squint-core/src/run.rs`. **Size.** M. Tag `m1.2`.

### Task 1.8: Repair protocol and retries

**Description.** Both repair forms, `turn_excluded`, reissue policies per response class,
`repair_output_tokens`, the repair budget, `error: model_output` and `error: submission_invalid`
on the second failure, transport retries with backoff, jitter, `retry-after`, counted as billable
attempts.

**CLI scenario.** A script with a truncated first response then a valid submission exits 0 with
one excluded turn and one reissue in the report; a script with an invalid submission then a valid
one exits 0 with one repair.

**Acceptance.**
- [ ] Each repair case reaches the documented outcome once and the documented error twice;
      reissue assertions by class (truncation changes only the output allowance; refusal and
      call-less text append the discarded-output instruction; excluded responses never enter
      provider history); retries on a mocked clock (429 with `retry-after: 2` waits 2 s; stops
      after `retries`; 400 fails immediately).

**Verification.** `cargo test -p squint-core -- repair retry`.

**Dependencies.** 1.7b. **Files.** `crates/squint-core/src/{repair,retry}.rs`. **Size.** M.

### Task 1.9: Cost ledger

**Description.** Decision envelope from a stub catalog entry including the reissue allowance,
attempt holds transferred from the envelope, settlement, charge at bound on unknown usage,
release, the per-job ledger, a `Ledger` trait the evaluation ledger implements in 3.4a, refusal
when a bound cannot be computed.

**CLI scenario.** `check` with `spend_cap_usd = 0.01` exits 3 with `error: budget` before any
request; the report shows the envelope.

**Acceptance.**
- [ ] Table test: total reserved never exceeds the envelope and never increases; settlement
      returns the difference; unknown usage charges the bound; release at the end; two concurrent
      decisions against one shared ledger cannot overshoot (barrier test).

**Verification.** `cargo test -p squint-core -- ledger`.

**Dependencies.** 1.8. **Files.** `crates/squint-core/src/ledger.rs`. **Size.** M.

### Task 1.10a: Journal and artifact durability

**Description.** Artifacts fsynced before their events, content-addressed store, redaction, 2 MB
body truncation with full-body hash, SIGINT and SIGTERM to `cancelled`, file mode 0600.

**CLI scenario.** Kill a running `check` with SIGTERM; the journal ends in `cancelled` with the
state reached.

**Acceptance.**
- [ ] SIGINT and SIGTERM tested; truncation with correct hash; no secret in any artifact (grep
      test); every referenced artifact exists before its event (crash test with an injected fault
      between write and append).

**Verification.** `cargo test -p squint-core -- journal artifacts`.

**Dependencies.** 1.9. **Files.** `crates/squint-core/src/{journal,artifacts}.rs`. **Size.** M.

### Task 1.10b: Report, schema, rebuild and prune

**Description.** `report.json` with every spec field, `schemas/report-v1.json`, a test that every
boundary type derives the three traits with `deny_unknown_fields` and a version field,
`squint report rebuild` from a partial journal, `squint report prune`.

**CLI scenario.** `report rebuild` on the cancelled run from 1.10a yields terminal status
`cancelled`; `report prune --older-than 0s` removes it.

**Acceptance.**
- [ ] Rebuild from each truncation point of a journal yields a report with the right terminal
      status; boundary-type test passes; prune respects age.

**Verification.** `cargo test -p squint-core -- report` and `cargo test -p squint-cli --features testkit --test report_cli`.

**Dependencies.** 1.10a. **Files.** `crates/squint-core/src/report.rs`, `schemas/report-v1.json`,
`crates/squint-cli/src/report.rs`. **Size.** S.

### Task 1.11: Cache at the decision boundary

**Description.** Cache key over the spec's list after prepare, decision-boundary semantics,
pruned-hit miss, evaluation mode never reads.

**CLI scenario.** The same `check` twice shows `cache = "hit"` the second time; with
`decision_runs = 2` the second run inside the first decision does not hit.

**Acceptance.**
- [ ] Table test flips each key input and each excluded input; cache spy for decision-boundary
      semantics; pruned artifacts make a miss.

**Verification.** `cargo test -p squint-core -- cache`.

**Dependencies.** 1.10b. **Files.** `crates/squint-core/src/cache.rs`. **Size.** S.

### Task 1.12: Isolation, environment filter and exit codes

**Description.** The pure environment-allowlist function, adversarial fixtures (instruction text
in pixels, filenames, labels, brief, tool results), registry-only id resolution, `_squint` from
the model rejected, the exit code matrix including 64.

**CLI scenario.** `check` on the adversarial job produces a prompt whose sections and tool list
equal the clean job's; a fake tool call with `image = "../../etc/passwd"` is rejected before any
filesystem access.

**Acceptance.**
- [ ] Prompt structure comparison per fixture; path-traversal test; environment filter table test
      (declared credential names still removed); exit codes 0, 1, 2, 3, 64 each produced.

**Verification.** `cargo test -p squint-cli --features testkit --test isolation --test exit_codes`.

**Dependencies.** 1.11. **Files.** `crates/squint-core/src/env.rs`, `crates/squint-cli/tests/`,
`fixtures/adversarial/`. **Size.** M.

### Task 1.13: Minimal MCP client against a fake server

**Description.** An MCP 2025-11-25 stdio client proving the lifecycle contract: spawn with the
environment filter; `initialize` with exact version and `tools` capability check;
`notifications/initialized`; single-page `tools/list`; `tools/call` with `_squint`; per-call
deadline; `notifications/cancelled` with tombstones; the exclusive lease whose single
`startup_timeout_s` deadline covers waiting for quiescence, `squint/reset` and its
acknowledgement, or shutdown and reinitialisation; no reuse after a cancelled call; shutdown
escalation. Pagination, concurrency, image blocks, `isError` precedence and the `rmcp` decision
are 4.4a/b.

**CLI scenario.** `check` with `[tools.packs.fake] command = "<path>"` lists and calls the fake
tool through MCP, where `<path>` is the executable built from the testkit target declared as
`[[bin]] name = "squint-testkit-mcp", path = "src/bin/mcp.rs"`; a test helper builds that
target and reads its absolute path from Cargo's JSON output, so no test depends on `PATH` or a
previous build.

**Acceptance.** Fake-server fixtures for: wrong version refused; missing `tools` capability
refused; successful reset after which a probe shows no prior-run state survives; a server without
reset support is restarted per run; timeout cancels, tombstones, marks the process unreusable and
the next lease replaces it; the total acquisition deadline is honoured when reset hangs (one
clock); shutdown escalates on a server ignoring stdin close.

**Verification.** `cargo test -p squint-tools -- mcp::minimal`.

**Dependencies.** 1.12, 1.5a. **Files.** `crates/squint-tools/src/mcp/`,
`crates/squint-testkit/src/bin/mcp.rs`. **Size.** M.

### Task 1.14: Minimal provisioning bundle proof

**Description.** A tiny `tools/python/` uv project with one MCP tool (`echo_region`) and the
provisioning layout from the spec; `squint tools setup` (verify digests, unpack,
`uv sync --frozen --no-dev` with the declared Python); offline launch with `UV_OFFLINE=1` and
`UV_PYTHON_DOWNLOADS=never`. Two separate verifications: provisioning, which uses the network and
runs only under `SQUINT_LIVE=1` or against a pre-built local bundle and a pre-seeded uv cache;
and runtime network denial, proven in the Linux CI job by launching the pack inside
`unshare -rn` (no network namespace) and asserting a successful call, with the pack-side
socket-failing test mode kept as a development aid, not proof.

**CLI scenario.** `squint tools setup --bundle tools/python/dist/bundle-0.0.1/` into an empty
`XDG_DATA_HOME`, then `check` with the pack enabled calls `echo_region` over MCP.

**Acceptance.**
- [ ] Clean-directory install yields a working executable whose installed versions match the
      lock; tampered digest fails; the `unshare -rn` launch succeeds in CI.

**Verification.** `cargo test -p squint-tools --test provisioning -- --ignored` (pre-seeded
bundle in CI); `cd tools/python && uv run --frozen --offline --no-sync pytest`.

**Dependencies.** 1.13. **Files.** `tools/python/`, `scripts/build-bundle.sh`,
`crates/squint-tools/src/provision.rs`, `.github/workflows/python.yml`. **Size.** M.

### Task 1.15: Statistics contract frozen

**Description.** In `squint-eval`: the paired bootstrap with a pinned RNG (ChaCha20 via
`rand_chacha`, dependency by ADR 0016, seed as a 64-bit unsigned decimal in the matrix), canonical
ordering (families sorted by id, items sorted by id within family), integer sampling by
`rand::Rng::gen_range` over the family index, a stored resample-index fixture for a small case
with unequal family sizes, type-7 quantiles, the detection-indicator family predicate, the
Bonferroni-adjusted endpoints and insufficient-tail rule, Wilson intervals, each against
hand-computed fixtures.

**CLI scenario.** `squint eval gate-table --fixture fixtures/stats/case-1.toml` prints the interval
and outcome for a synthetic result table.

**Acceptance.**
- [ ] The stored resample indices and interval are reproduced exactly from the stored seed; four
      differing families yield `inconclusive`; a tail with fewer than 20 resamples yields
      `inconclusive`; Wilson matches a reference table to 1e-9.

**Verification.** `cargo test -p squint-eval -- stats`.

**Dependencies.** 0.1. **Files.** `crates/squint-eval/src/{bootstrap,wilson,gate_rules}.rs`,
`fixtures/stats/`, `docs/adr/0016-rng.md`. **Size.** M.

### Checkpoint M1
- [ ] `cargo test --workspace` under five minutes, offline; transition and row coverage report in
      the PR.
- [ ] `docs/architecture.md`, `docs/rules.md`, `docs/tools.md` filled for what exists.
- [ ] Review and test skills (or the checklist) applied; findings fixed or recorded.
- [ ] Owner review; merge; tag `m1`.

## Milestone M2: Provider adapters

### Task 2.1: Model catalog and capability validation

**Description.** `providers/catalog.toml` with every spec field; values read from provider
documentation and model lists with `verified` date and source URL; resolve-time rejection of
unsupported settings; `squint catalog show`. Models: the owner's initial subset from 0.4 first.

**Acceptance.** Catalog self-test: every entry complete with sources; unsupported-setting
rejections tested; `catalog show` prints an entry.

**Verification.** `cargo test -p squint-providers -- catalog`.

**Dependencies.** 1.3, 0.4. **Files.** `providers/catalog.toml`,
`crates/squint-providers/src/catalog.rs`. **Size.** M.

### Task 2.2: Wire schema transformations

**Description.** Per-adapter transformations and normalisation back to canonical; resolve-time
representability check; snapshot fixtures.

**Acceptance.** For each adapter: transformed submission schema accepts the spec example; a
payload violating a removed constraint still fails canonical validation; an unsupported construct
is refused with the reason.

**Verification.** `cargo test -p squint-providers -- wire`.

**Dependencies.** 1.6a, 2.1. **Files.** `crates/squint-providers/src/wire.rs`, `fixtures/schemas/`.
**Size.** M.

### Task 2.3: Image preparation and preflight

**Description.** Deterministic preparation with `prep_version`, metadata stripping, preflight over
the whole provider history including tool images.

**Acceptance.** Identical bytes on repeat; exact native size; PNG kept with alpha; no EXIF or XMP;
a scripted conversation reaching the many-image threshold fails preflight before sending.

**Verification.** `cargo test -p squint-providers -- prepare preflight`.

**Dependencies.** 1.2, 2.1. **Files.** `crates/squint-providers/src/prepare.rs`. **Size.** M.

### Task 2.4: Privacy enforcement before any live call

**Description.** Consent bound to profile id, account and upstream allowlist; recipient chain in
the report; `error: auth` on a missing credential; cross-host redirects refused by the HTTP
client; no filenames or paths in requests; redaction re-tested with real adapter request shapes;
a fixture-directory test that greps every recorded request for paths and EXIF markers;
`docs/provider-data.md` columns defined.

**Acceptance.** Run refused without `account`; redirect to another host is `error: provider`;
missing key is `error: auth`; the fixture grep test exists and runs on every fixture directory.

**Verification.** `cargo test -p squint-providers -- privacy`.

**Dependencies.** 1.3, 1.10a. **Files.** `crates/squint-providers/src/{http,privacy}.rs`,
`crates/squint-core/src/report.rs`, `docs/provider-data.md`. **Size.** S.

### Task 2.5a: Anthropic adapter implementation and replay harness

**Description.** Serde types for the Messages API subset; pairing; thinking blocks echoed;
`classify`; the three final-turn mechanisms gated by the catalog; the `anthropic` profile; the
fixture replay harness shared by all adapters (canonical request comparison minus declared
volatile transport fields; opaque payloads byte-exact).

**Acceptance.** Replay harness tested with hand-written fixtures; dropping a thinking block fails
replay; changing a volatile header passes.

**Verification.** `cargo test -p squint-providers -- anthropic replay`.

**Dependencies.** 2.2, 2.3, 2.4, 1.8. **Files.** `crates/squint-providers/src/anthropic/`,
`crates/squint-providers/src/replay.rs`, `providers/profiles/anthropic.toml`. **Size.** M.

### Task 2.5b: Anthropic recordings

**Description.** Record live fixtures under `SQUINT_LIVE=1` on the account named in 0.4 for
every capability combination: first look, parallel batch with two same-name calls, tool error,
multimodal tool result, each repair case in both forms, final-turn transition with a stray call
under each supported mechanism, the largest permitted image conversation; strip secrets; commit;
replay. One recording designated the profile-conformance fixture. Spend recorded against the 0.4
cap.

**Acceptance.** Every listed fixture exists and replays; live `squint check` on a trivial job
returns a verdict with resolved model, API version, usage and cost.

**Verification.** `cargo test -p squint-providers -- anthropic`; `SQUINT_LIVE=1 cargo test -p squint-providers --test record -- --ignored anthropic`.

**Dependencies.** 2.5a. **Files.** `fixtures/adapters/anthropic/`, `fixtures/profiles/anthropic/`.
**Size.** M; real money. Tag `m2.1`.

### Task 2.6a: Gemini adapter implementation

**Description.** `functionCall` ids with the catalog exception, thought signatures as original
parts, function calling mode, `parametersJsonSchema`, `classify`, the `google-ai-studio` profile;
a replay test that a moved or altered thought signature fails.

**Acceptance.** As 2.5a plus the signature test.

**Verification.** `cargo test -p squint-providers -- gemini`.

**Dependencies.** 2.5a. **Files.** `crates/squint-providers/src/gemini/`,
`providers/profiles/google-ai-studio.toml`. **Size.** M.

### Task 2.6b: Gemini recordings

**Description.** As 2.5b for Gemini.

**Dependencies.** 2.6a. **Files.** `fixtures/adapters/gemini/`, `fixtures/profiles/google-ai-studio/`.
**Size.** M; real money.

### Task 2.7a: OpenAI adapter implementation

**Description.** Responses API: strict wire schema, `call_id` matching, reasoning items echoed,
`store = false` on every request (tested), `incomplete` as non-replayable, `classify`, the
`openai` profile.

**Acceptance.** As 2.5a plus the `store = false` test.

**Verification.** `cargo test -p squint-providers -- openai`.

**Dependencies.** 2.5a. **Files.** `crates/squint-providers/src/openai/`,
`providers/profiles/openai.toml`. **Size.** M.

### Task 2.7b: OpenAI recordings

**Description.** As 2.5b for OpenAI.

**Dependencies.** 2.7a. **Files.** `fixtures/adapters/openai/`, `fixtures/profiles/openai/`.
**Size.** M; real money.

### Task 2.8: Gateway profile candidates (optional)

**Description.** Profiles for `vercel-ai-gateway-anthropic`, `cloudflare-ai-gateway-anthropic`,
`cloudflare-ai-gateway-google` with upstream contracts; fixtures including routing body and
provenance; `unshipped` marking with reasons when provenance cannot be read. Only for gateways the
owner holds accounts for; spend against the 0.4 gateway-smoke entry. Nothing depends on this task.

**Acceptance.** Each shipped gateway profile has a routing and provenance fixture; a disallowed
upstream fails; unshipped candidates are refused by `check` with the reason.

**Verification.** `cargo test -p squint-providers -- profiles`.

**Dependencies.** 2.5b, 2.6b. **Files.** `providers/profiles/`, `fixtures/profiles/`. **Size.** M.

### Task 2.9: provider-data.md

**Description.** Fill enforced, attested and unverifiable columns for the three direct profiles
and any shipped gateway, with citations; a test that every shipped profile id appears.

**Dependencies.** 2.5b, 2.6b, 2.7b. **Files.** `docs/provider-data.md`. **Size.** S.

### Task 2.10a: Packaging for the registry

**Description.** Versioned registry dependencies between all six workspace crates; the assets
that exist by M2 (rules, prompts, profiles, catalog, schemas) packaged with the crates that embed
them, verified by `cargo package` and a test that lists every embedded asset from the packaged
crate (the viewer is added to this check in 3.6); a publication script in the order `core`,
`testkit`, `providers`, `tools`, `eval`, `squint` that the owner runs; a CI job that packages
every crate, stages the `.crate` files in a temporary local registry, and installs `squint` from
that registry in a clean container without the repository tree, then runs the offline smoke
(`squint --version`, `tools list`, `rules show`, `check` without a key yielding `error: auth`).
The workspace-level publish dry run supported by the pinned Cargo version runs in CI.

**Acceptance.** Staged-registry install smoke passes in CI; the workspace publish dry run passes.
After the owner's first real publication (scheduled at the M5 checkpoint), from outside the
repository `cargo install squint-cli --version <released> --locked` succeeds and passes the smoke.

**Verification.** `.github/workflows/package.yml`; `scripts/publish.sh --dry-run`;
`scripts/registry-install-test.sh`.

**Dependencies.** 2.5b, 2.6b, 2.7b. **Files.** `scripts/publish.sh`, `.github/workflows/package.yml`,
crate manifests. **Size.** M.

### Task 2.10b: Binary distribution and live smoke

**Description.** `cargo-dist` producing macOS arm64 and Linux x86_64 and arm64 archives on a tag;
archive smoke on each architecture in CI (offline smoke as above); one shipped example config per
use case under `examples/`; a capped, owner-approved live smoke through all three direct profiles
from an installed archive, recorded in the PR.

**Acceptance.** Release workflow dry run produces three archives that pass the offline smoke on
their architectures; live smoke reports attached.

**Verification.** `.github/workflows/release.yml` dry run; `scripts/archive-smoke.sh`.

**Dependencies.** 2.10a. **Files.** `.github/workflows/release.yml`, `dist-workspace.toml`,
`examples/`. **Size.** M; small real money.

### Checkpoint M2
- [ ] Live smoke on all three direct profiles; reports and costs in the PR.
- [ ] Fixture replay covers every capability combination; one profile-conformance fixture per
      shipped profile.
- [ ] SPEC open question 1 closed.
- [ ] Review and test skills (or checklist); owner review; merge; tag `m2`.

## Milestone M3: Evaluation sets and baseline

### Task 3.1: Set manifests, check, fetch and seal

**Description.** Manifest parsing; `sets check` (hashes, roles, families across splits, label
presence per split, public manifests and jobs contain no answer labels, jobs resolve); `sets
fetch` by hash; `sets seal` writing `seal.toml` over sorted membership, families, canonical jobs,
image hashes, rules content and version, and the external label file's hash, with timestamp and
owner attestation text; `sets verify-seal`.

**Acceptance.**
- [ ] Each failure mode of `check` has a fixture; `fetch` refuses a mismatch; a test mutates each
      seal component in turn (membership, a family, a job field, an image, rules, labels) and
      `verify-seal` rejects each.

**Verification.** `cargo test -p squint-eval -- sets seal`.

**Dependencies.** 1.4b. **Files.** `crates/squint-eval/src/{sets,seal}.rs`,
`crates/squint-cli/src/sets.rs`, `fixtures/evalsets/`. **Size.** M.

### Task 3.2a: Set-building toolkit and ADR 0013

**Description.** `scripts/sets/` helpers: generate sources and candidates with the chosen image
models, plant defects by construction, write `job.toml`, draft labels and regions from the
construction record only, write the owner's worksheet (`labels-draft.toml` plus contact sheet) to
owner storage outside the repository, record generator and licence per item. ADR 0013 names
generators, licence and the review procedure. Create the `squint-evalsets` repository.
`docs/evalsets.md` documents the procedure.

**Acceptance.** A two-item dry run yields a valid set directory and an external worksheet; ADR
written; evalsets repository exists with README and licence.

**Dependencies.** 3.1, 0.4. **Files.** `scripts/sets/`, `docs/evalsets.md`,
`docs/adr/0013-set-generation.md`. **Size.** M.

### Task 3.2b: First dev set adjudicated and published

**Description.** Reference consistency, 20 items, at least two failure types, exemption-adjacent
items, families recorded; owner adjudicates the worksheet; images reviewed and pushed to the
evalsets repository by the owner; manifest and labels (dev sets publish labels) committed here.

**Acceptance.** `sets check` passes; the review log lists every image; the owner attests in the
PR that no judge or tool ran on the items (journal search of local output directories attached as
supporting evidence).

**Dependencies.** 3.2a. **Files.** `evalsets/reference-consistency/pets-v1/`. **Size.** M; owner
time.

### Tasks 3.3a to 3.3d: Remaining dev sets

**Description.** One task per use case: source fidelity (exercising the generic base and the
architectural profile), reality checks, brief compliance, product fidelity; 20 items each, roughly
balanced, at least two failure types, exemption-adjacent items, families recorded; built with the
accepted procedure and adjudicated by the owner.

**Acceptance.** As 3.2b per set; `evalsets/README.md` tabulates items, failure types and families.

**Dependencies.** 3.2b. **Files.** `evalsets/<use-case>/`. **Size.** M each; owner time.

### Task 3.3e: Adversarial sets

**Description.** Five two-item sets, one per use case under the existing schema, whose images,
labels or briefs carry instruction-like text aimed at the judge; expected verdicts from
construction; same review log and acceptance as 3.2b; the baseline reports them separately.

**Dependencies.** 3.2b. **Files.** `evalsets/<use-case>/adversarial-v1/`. **Size.** S; owner time.
Tag `m3.1`.

### Task 3.4a: Execution descriptor and durable evaluation ledger

**Description.** `matrix.toml` parsing; the execution descriptor over every answer-affecting
input (matrix, set manifests, jobs, image hashes, rules content, prompt bytes, tool manifests and
lock digests, profiles, catalog version, harness version); a single-writer lock on the evaluation
directory; the durable ledger: each attempt's hold persisted before send, settlement at most once,
unresolved sent-or-possibly-sent attempts charged at bound after restart, completed errors kept;
per-decision admission against the durable remaining cap; the versioned admission-record type
(`evalsets/admissions.toml`: tool, use case, stage, receipt id), the receipt-reference type, their
reader, and synthetic records for tests (scoring and writing them belong to 3.5b).

**Acceptance.** Crash tests with injected faults immediately before and after hold persistence,
send, settlement and completion marking each recover to a consistent ledger; a changed descriptor
refuses resume; two writers are refused; cumulative spend across restarts never exceeds the cap;
the admission record round-trips.

**Verification.** `cargo test -p squint-eval -- descriptor eval_ledger admissions`.

**Dependencies.** 1.9, 1.11, 3.1. **Files.** `crates/squint-eval/src/{matrix,descriptor,eval_ledger,admissions}.rs`.
**Size.** M.

### Task 3.4b: Matrix runner, arms and resume

**Description.** `eval run` executing every cell with cache disabled, immutable per-item run
directories, recovery states for partial items, resume skipping complete items, exact A/C/D_X/D
membership from the admission record (D includes only `confirmed` tools), prompt equivalence
across arms except the Tools section, projection application, summary JSON, repetition 1 flagged
as the decision repetition.

**Acceptance.** Fake-provider matrix runs every cell, never touches the cache (spy), stops at the
cap with `error: budget` per unstarted item, resumes after SIGINT without redoing complete items
and marks partial items; arm membership and prompt-equivalence tests pass; against the 3.4a
synthetic admission records, a `provisional` tool is excluded from D and a `confirmed` one
included.

**Verification.** `cargo test -p squint-eval -- runner arms`.

**Dependencies.** 3.4a. **Files.** `crates/squint-eval/src/{runner,arms}.rs`. **Size.** M.

### Task 3.4c: Pre-run estimate

**Description.** `squint eval estimate --matrix …` prints decisions, per-decision envelopes,
their sum and a cost range, and warns when the sum exceeds the cap (completion not guaranteed);
it never refuses, because admission is per decision.

**Acceptance.** Each estimated decision envelope equals the ledger's reservation for that
decision on a fake run; totals equal their sum; a matrix whose sum exceeds the cap can still
complete when settled costs permit (test).

**Dependencies.** 3.4b. **Files.** `crates/squint-eval/src/estimate.rs`. **Size.** S.

### Task 3.5a: Metrics, localisation and report regeneration

**Description.** Confusion, missed and detected failures under the frozen policy, false rejects,
unsure and error rates, localisation with eligibility rules, cost totals and medians, wall time
totals and medians, Wilson intervals from 1.15, headline numbers from repetition 1 only,
`squint eval report` regenerating the Markdown summary from saved results.

**Acceptance.** Hand-computed fixtures match to 1e-9 for proportions and exactly for counts,
totals and medians; localisation fixtures cover each eligibility rule; a fixture where later
repetitions reverse the apparent winner leaves the headline unchanged; `eval report` on saved
results reproduces the stored Markdown byte for byte.

**Verification.** `cargo test -p squint-eval -- metrics summary`.

**Dependencies.** 3.4b, 1.15. **Files.** `crates/squint-eval/src/{metrics,summary}.rs`. **Size.** M.

### Task 3.5b: Gate lifecycle

**Description.** `eval gate` with stages: verify the seal against supplied labels, verify results
against the descriptor and frozen matrix identity, refuse repeat scoring without a new ADR
reference, write the receipt, write label publication and split change as files for the owner to
review and commit (never commits), resume an interrupted scoring from the receipt without
rescoring; the confirmation stage requires the initial receipt, the identical frozen judging
configuration, at least 50 items from disjoint families, and yields `confirmed` only when both
stages are admitted; `evalsets/admissions.toml` records tool, use case, stage, receipt id.

**Acceptance.** Table tests for every ceiling, margin, family minimum, multiplicity boundary and
outcome precedence; label-hash mismatch, descriptor mismatch and repeat scoring refused; the
initial stage refuses a non-blind set or fewer than 20 items; the confirmation stage refuses a
missing initial receipt, fewer than 50 items, a family overlapping any earlier set, or a changed
judging configuration; interrupted scoring resumes; written admission records are read back by
the 3.4a reader.

**Verification.** `cargo test -p squint-eval -- gate`.

**Dependencies.** 3.5a, 3.1. **Files.** `crates/squint-eval/src/gate.rs`. **Size.** M.

### Task 3.6: Static report viewer

**Description.** `crates/squint-cli/viewer/` (embedded; the packaging check keeps assets inside
their crate) with no build step; `squint report view <run-dir>` serves on loopback; shows images, tool results, checklist, flat locations in raw
coordinates, aggregation rows and decision table; declares supported `report_version`s; no
requests except to loopback.

**Acceptance.** Devtools check on a fake run: no console errors, no non-loopback requests; a
crop-frame location is drawn at the stored overlay position (pixel compare); the viewer assets are
added to the 2.10a embedded-asset packaging check and the staged-registry smoke opens a report.

**Verification.** Browser devtools script; `cargo test -p squint-cli --features testkit --test viewer_serve`.

**Dependencies.** 1.10b. **Files.** `crates/squint-cli/viewer/`, `crates/squint-cli/src/{viewer,report}.rs`, `docs/viewer.md`.
**Size.** M.

### Task 3.7: Canary and requalify

**Description.** `eval canary`: pinned canary set, uncached, compared to the stored baseline
identity; `suspect` on one deviation, `recommend_disable` on two consecutive against the same
baseline; never disables; `eval requalify` runs the full development set and records a new
baseline on owner approval.

**Acceptance.** Fake-provider tests for `clear`, `suspect`, `recommend_disable` and baseline
reset after requalification.

**Dependencies.** 3.4b. **Files.** `crates/squint-eval/src/canary.rs`. **Size.** S.

### Task 3.8: Arm A baseline and publication

**Description.** `eval estimate` approved by the owner against the 0.4 cap; run the matrix (five
dev sets plus five adversarial sets × the initial model subset × arm A × `decision_runs = 1`,
`repetitions = 2`); publish `docs/results/baseline-<date>.md` with every metric, resolved model
ids and API versions, costs, adversarial results reported separately, the matrix and summary JSON,
stating these are development sets.

**Acceptance.** Results document and matrix committed; spend under cap and recorded.

**Dependencies.** 2.5b, 2.6b, 2.7b, 3.3a, 3.3b, 3.3c, 3.3d, 3.3e, 3.4c, 3.5a. **Files.**
`docs/results/`, `evalsets/matrix/baseline.toml`. **Size.** M; real money.

### Checkpoint M3
- [ ] Ten public sets published after review; baseline published; viewer works offline.
- [ ] Review and test skills (or checklist); owner review; merge; tag `m3`.

## Milestone M4: Built-in tools and MCP client

### Task 4.0: Alignment feasibility spike and ADR 0014

**Description.** A bounded spike (two days): flat homography estimation with the `image` crate
plus candidate dependencies, on synthetic pairs with known transforms, unrelated pairs,
low-texture pairs and degenerate transforms; tolerances (translation within 0.5 px at 2048 width,
rotation within 0.1°, inlier fraction reported); failure fixtures; equirectangular rotation
estimation scoped the same way on newly generated or CC-licensed panoramas (owner decision 11:
nothing from the prototype is reused).

**Acceptance.** ADR 0014 with measured tolerances and the chosen approach; failure fixtures
committed.

**Dependencies.** 1.2, 0.4. **Files.** `docs/adr/0014-alignment.md`, `fixtures/images/align/`.
**Size.** M.

### Task 4.1: Registry in production

**Description.** Built-in registration, `[tools] enabled` and `order`, Tools section with
`requires` ordering, projection mechanism applied before the checklist, `squint tools list` over
real built-ins.

**Acceptance.** Ordering and `requires` tests; byte-stable rendering snapshot; projection applied
before the checklist sees the envelope.

**Dependencies.** 1.5a, 1.7a. **Files.** `crates/squint-tools/src/{registry,projection}.rs`.
**Size.** S.

### Task 4.2a: `view`, `resize` and equirectangular geometry

**Description.** Crop by bbox; square perspective view by yaw, pitch, fov (port of the
prototype's projection, verified against stored views); one image or two; `crop` frames;
`resize`; equirectangular registry maths (view to raw, seam rules, clipping).

**Acceptance.** Crop equals stored crop within 1 grey level per channel; perspective view within 2
grey levels of the stored view; view-to-raw round trip within 0.5 px at 2048 width;
seam-straddling bbox refused; `fov = 130` is `invalid_args`.

**Verification.** `cargo test -p squint-tools -- view resize` and `cargo test -p squint-core -- frames::sphere`.

**Dependencies.** 4.1, 1.2. **Files.** `crates/squint-tools/src/builtin/{view,resize}.rs`,
`crates/squint-core/src/frames.rs`. **Size.** M.

### Task 4.2b: Viewer equirectangular overlay

**Description.** The viewer draws `view` locations and equirectangular polygons on panoramas
through the frame registry.

**Acceptance.** Stored-overlay pixel compare for a `view` location on the fixture panorama.

**Dependencies.** 4.2a, 3.6. **Files.** `viewer/`. **Size.** S.

### Task 4.3a: `align` for flat pairs and `visual_only`

**Description.** From the spike: homography, `aligned` frame with lossless warped pixels,
integrity measurements, `not_applicable` for unrelated pairs, the `visual_only` projection.

**Acceptance.** Synthetic transforms recovered within ADR tolerances; unrelated pair is
`not_applicable`; `visual_only` strips `quality`, `measurements`, `regions` and fixes `summary`.

**Dependencies.** 4.0, 4.1. **Files.** `crates/squint-tools/src/builtin/align.rs`. **Size.** M.

### Task 4.3b: `align` for equirectangular pairs

**Description.** Rotation on the sphere, warped pixels, the same envelope.

**Acceptance.** Known rotations recovered within ADR tolerances on stored panoramas.

**Dependencies.** 4.3a, 4.2a. **Files.** `crates/squint-tools/src/builtin/align_sphere.rs`.
**Size.** M.

### Task 4.3c: `edge_similarity` and `change_map`

**Description.** Edge similarity over an aligned pair; change map over an aligned pair returning
one image and no regions.

**Acceptance.** Identical images give 1.0 within 1e-6; blank versus textured gives below 0.05;
the change map for a fixture pair equals the stored golden image within 1 grey level.

**Dependencies.** 4.3a. **Files.** `crates/squint-tools/src/builtin/compare.rs`.
**Size.** S. Tag `m4.1`.

### Task 4.4a: Full MCP client: ADR 0015, pagination, concurrency, images

**Description.** ADR deciding `rmcp` versus the in-house client after a bounded spike; paginated
`tools/list`; concurrent calls where declared; frames in `_squint`; image content blocks with
exact correspondence; `isError` precedence as an exhaustive truth table; `outputSchema` validation.

**Acceptance.** Fake-server tests for each; the truth table test enumerates every combination.

**Dependencies.** 1.13, 4.1. **Files.** `docs/adr/0015-mcp-client.md`, `crates/squint-tools/src/mcp/`.
**Size.** M.

### Task 4.4b: MCP failure modes

**Description.** Process death mid-call, malformed frames, unknown ids, late responses after
cancellation, reset failure restart, shutdown escalation on an unresponsive server, lease
contention between two decisions.

**Acceptance.** One fake-server test per mode; lease contention shows exclusive access.

**Dependencies.** 4.4a. **Files.** `crates/squint-tools/src/mcp/`,
`crates/squint-testkit/src/bin/mcp.rs`. **Size.** M.

### Task 4.5: Arm C on the dev sets

**Description.** `eval estimate` approved; matrix with arms A and C on the ten sets for the
initial models; publish `docs/results/arm-c-<date>.md` with every metric per set and model and the
identity fields. Paired flips are computed between repetition 1 of A and C within this matrix,
matching item, model and judging settings, with a check that answer-affecting differences are
limited to the declared arm; the M3 baseline is shown separately as historical context, never as
the comparison arm.

**Acceptance.** Results document and matrix committed with identity fields and within-matrix
paired flips; spend under cap.

**Dependencies.** 4.2a, 4.3b, 4.3c, 3.8. **Files.** `docs/results/`, `evalsets/matrix/arm-c.toml`.
**Size.** S; real money.

### Task 4.6: Segmenter artifact and MODEL.md

**Description.** Export a segmenter from publicly licensed upstream weights (candidate EoMT-L
ADE20K; licence check first, alternative by ADR if it fails) with this project's own export
script, publish it as a release asset, and write `tools/python/MODEL.md` (upstream source and
licences, export procedure and versions, URL, SHA-256, preprocessing version, onnxruntime version
and execution provider, parity tolerance, golden inputs generated here). Nothing from the
prototype is reused (owner decision 11). M4 exit prerequisite.

**Acceptance.** `MODEL.md` complete; the export script is committed and reproduces the artifact
hash; the asset downloads and matches.

**Dependencies.** 0.4. **Files.** `tools/python/MODEL.md`, `tools/python/scripts/export_segmenter.py`.
**Size.** M.

### Checkpoint M4
- [ ] `docs/tools.md` explains writing a tool with `view` as the worked example; `MODEL.md`
      complete; alignment ADR recorded.
- [ ] Review and test skills (or checklist); owner review; merge; tag `m4`.

## Milestone M5: First ML tool

### Task 5.1: Python pack skeleton and MCP server

**Description.** Grow the 1.14 pack into the real server: `_squint` images and frames, envelope
as `structuredContent` plus image blocks, `squint/reset`, `fetch` entry point, offline pytest
and ruff.

**Acceptance.** Rust client calls `echo_region` with an image and an aligned frame and receives a
valid envelope with one image block; offline tests pass.

**Dependencies.** 4.4b, 1.14. **Files.** `tools/python/src/squint_tools/`. **Size.** M.

### Task 5.3: Segmenter fetch and parity

**Description.** `fetch` downloads and verifies the artifact from `MODEL.md`; a `segmenter`-marked
parity test against stored golden outputs within the documented tolerance.

**Acceptance.** Parity passes; `fetch` refuses a hash mismatch.

**Dependencies.** 4.6, 5.1. **Files.** `tools/python/src/squint_tools/segmenter.py`. **Size.** S.

### Task 5.4a: `regions` tool

**Description.** Segmentation-based change-region finder over an aligned pair, evidence kind
`change_region`, accepts `hints.regions`, bounded regions with labels and scores, one overlay
image, the prototype's decision tables stripped to high-recall pointers; manifest with
`requires = ["align"]`.

**Acceptance.** On newly generated synthetic pairs with planted changes, every planted region is
found with IoU above 0.3 and at most `max_regions` are returned; the manifest passes all three
adapters' wire transformations.

**Dependencies.** 5.3. **Files.** `tools/python/src/squint_tools/regions.py`. **Size.** M.

### Task 5.4b: `classes` tool

**Description.** Segmenter classes before and after in a region, evidence kind
`class_difference`.

**Acceptance.** Named fixtures return the exact expected class lists; manifest passes wire
transformations.

**Dependencies.** 5.3. **Files.** `tools/python/src/squint_tools/classes.py`. **Size.** S.

### Task 5.2: Provisioning bundle for the real pack

**Description.** Bundle build from the final pack sources including `regions`, `classes` and the
segmenter fetch; `tools setup` end to end; lock digest in provenance and cache key; clean-install
CI job; `unshare -rn` runtime check. Numbered 5.2 for continuity with draft 2 but scheduled after
the tools it packages.

**Acceptance.** As 1.14 for the real pack, with `regions` and `classes` listed by the installed
server; cache key changes when the lock digest changes.

**Dependencies.** 5.3, 5.4a, 5.4b. **Files.** `tools/python/bundle/`, `scripts/build-bundle.sh`.
**Size.** S. Tag `m5.1`.

### Task 5.5: Sealed blind set for source fidelity

**Description.** Before any judge model or tool runs on these items: build 20 source-fidelity
items with families disjoint from the dev set, including exemption-adjacent items; the owner
adjudicates from construction records only; labels in owner storage from creation; `sets seal`;
images published with the manifest carrying `seal.toml` and `labels_sha256` only.

**Acceptance.** `sets check` and `verify-seal` pass; public manifest and jobs carry no answer
labels (test); owner attestation of no prior exposure recorded in the seal; local output
directories searched as supporting evidence.

**Dependencies.** 3.2a, 3.1. **Files.** `evalsets/source-fidelity/blind-v1/`. **Size.** M; owner
time.

### Task 5.6: Initial gate D_regions versus C

**Description.** Freeze the matrix (model, prompt, obligations version, budgets, policy, error
ceiling, hypothesis count, family minimum, seed); `eval estimate` approved; run arms C and
D_regions on the sealed set; `eval gate --stage initial` (seal verification, receipt, label and
split files); the owner reviews and commits the publication; publish
`docs/results/gate-regions-initial-<date>.md` with the outcome, marked provisional.

**Acceptance.** Receipt, publication and results committed; spend under cap; outcome stated as
`admitted (provisional)`, `rejected` or `inconclusive`; `admissions.toml` updated.

**Dependencies.** 5.4a, 5.5, 4.5, 3.5b, 5.2. **Files.** `docs/results/`,
`evalsets/matrix/gate-regions.toml`, `evalsets/admissions.toml`. **Size.** M; real money.

### Checkpoint M5
- [ ] Spec success criteria 1 to 7 checked one by one; the full offline suite rerun under five
      minutes with every fixture and tool present.
- [ ] Final artifacts rebuilt from the final sources: Rust packages (staged-registry install
      smoke), platform archives (architecture smoke), Python bundle (clean install); the owner
      publishes to crates.io and `cargo install squint-cli --version <released> --locked` passes
      from outside the repository.
- [ ] README "How it is built" updated with measured results and links.
- [ ] Review and test skills (or checklist); owner review; merge; tag `m5`.

The confirmation of `regions` (formerly M6) is out of the plan: both initial gates were rejected
because the control caught every planted failure, and `regions` stays an optional tool. Retesting
it under a cheaper model is tracked in GitHub issue #3.

## Milestone M7: Contract conformance

squint conforms to `table-manners` v0.1.0, the core and the images profile (ADR 0018, SPEC
"External interface"). Field names are the contract's; tasks name squint's side only. The branch
`m7-contract` starts from `main`. What M7 finds wrong in the
contract goes into table-manners v0.2 before rascal's conformance merges.

### Task 7.1: The contract pinned

**Description.** Depend on the `table-manners` crate at `=0.1.0` from `squint-core`; copy its
schemas to `schemas/contract/` with a `SOURCES.toml` (tag, path, sha256) and a test that the copy
matches both the hashes and the crate's embedded `SCHEMAS`; a CI step that warns, without
failing, when a newer `table-manners` tag exists. The crate graph check and `cargo deny` accept the
new edge.

**Acceptance.** Editing a copied schema fails the test; the warn step prints the newer tag against
a stubbed tag list and exits 0.

**Dependencies.** table-manners v0.1.0 published to crates.io (owner). **Files.**
`crates/squint-core/Cargo.toml`, `schemas/contract/`, `.github/workflows/ci.yml`,
`scripts/check-contract-tag.sh`. **Size.** S.

### Task 7.2: Image references in and verified

**Description.** squint computes the contract's image reference for every image a job names:
`file_sha256` of the bytes it was given, width and height as stored, projection and topology from
the job, and `pixel_sha256` by the contract's recipe. A job may carry an `expect` reference per
image; a mismatch in digest or dimensions ends the run as `invalid` with code `input` before any
provider call.

**Acceptance.** The contract's twelve pixel vectors reproduce through squint's own decode path;
an `expect` with a wrong digest or a wrong width refuses the job and spends nothing; a JPEG whose
`pixel_sha256` differs while `file_sha256` matches passes with a note.

**Dependencies.** 7.1. **Files.** `crates/squint-core/src/image.rs`, `crates/squint-core/src/job.rs`.
**Size.** M.

### Task 7.3: Contract locations out

**Description.** Every finding's location is mapped through the frames registry to the raw frame
of its root image, then through the inverse of its EXIF orientation to the file as stored, and
written as a contract location: edge fractions at quarter-pixel precision, outward snapping
documented, `parts` with `straddles_wrap` (a view footprint cut at the seam becomes two parts, up
to 256 vertices each), kinds `bbox`, `polygon`, `whole_image` and `absent` (with a reason), the
finding's words as `label`, its disposition and confidence, and `source` (`squint`, version, run
id, item id). No crop, view or aligned frame leaves squint.

**Acceptance.** Property tests: a box drawn in a crop or a view maps back onto the raw pixels it
covers; a footprint across the seam becomes two parts on a `wrap` image and never on a `bounded`
one; every emitted location validates against the contract's schema; a finding with no geometry
is `absent` with a reason, never an invented box.

**Dependencies.** 7.2. **Files.** `crates/squint-core/src/frames/`, `crates/squint-core/src/contract.rs`.
**Size.** M.

### Task 7.4: `run.json`, the result header and exit codes

**Description.** `check` writes `run.json` last into every decision directory on every surface,
atomically (a temporary name, then a rename): the contract header (status, error with code,
`retryable` and reason, run, tool, timestamps, duration, input identity, spend with basis
`measured`, cache, images in with slots and roles, images out, locations used and found, notes for
repairs and tool refusals) and squint's body (verdict, policy outcome, findings, the report's
path). squint's errors map to the contract's codes in one table. Exit codes across commands: 0,
1 fail, 2 unsure, 3 error, 64 usage or input, 130 on SIGINT and 143 on SIGTERM; a cancelled run
writes `run.json` with status `cancelled` and spend to date.

**Acceptance.** Every terminal state of the M1 scenarios produces a `run.json` that validates
against the contract's result schema; a killed write leaves the previous file or none, never half
of one; SIGINT and SIGTERM exit 130 and 143 with `cancelled`; a provider 529 after retries is
`error`, `provider`, `retryable: true`; a fail verdict is `completed`.

**Dependencies.** 7.2, 7.3. **Files.** `crates/squint-core/src/run.rs`,
`crates/squint-core/src/contract.rs`, `crates/squint-cli/src/main.rs`, `docs/`. **Size.** M.

### Task 7.5: `squint describe --json`

**Description.** The contract's description document from the shipped use cases, rule sets and
catalog: `profiles: ["images"]`, one action per use case with its image slots, location slots and
`params_schema`, `estimate`, the renders (`report view`, `report export`), cost with
`needs_env`, duration, `vocabulary` (every disposition, confidence and evidence word squint
emits), `limits.inline_bytes`, `progress_interval_s: 10` and the resources. squint's own
self-description commands (`rules`, `tools list`, `catalog`) stay and are referenced under
`x-squint`.

**Acceptance.** The document validates against the contract's schema; a test fails when squint can
emit a vocabulary word the document does not list; no environment variable value appears in it.

**Dependencies.** 7.1. **Files.** `crates/squint-cli/src/describe.rs`. **Size.** S.

### Task 7.6: `estimate` for a check

**Description.** The worst-case cost of one check from its job, config and the catalog's prices
(turns, tool calls, retries, repairs at their caps), with the per-call cap from config that a call
may lower and never raise. Exposed as `squint estimate` and, in 7.7a, the `estimate` action; the
session budget left is added by the server.

**Acceptance.** The estimate bounds the measured spend of every recorded fixture run; lowering the
cap below the estimate makes `check` refuse with `budget` before any provider call.

**Dependencies.** 7.5. **Files.** `crates/squint-core/src/ledger.rs`, `crates/squint-cli/src/`.
**Size.** S.

### Task 7.7a: `squint serve`: the server, `describe` and `estimate`

**Description.** An MCP server over stdio on `rmcp` `=3.8.0` at revision `2025-11-25` only,
refusing other revisions at `initialize`; tools with pagination and resources capabilities; the
`describe` and `estimate` tools; JSON-RPC errors only before a run exists. An independent MCP test
client in `squint-testkit`, sharing no code with the server or the SDK, after rascal's
`rascal_testkit::mcp_client`.

**Acceptance.** The test client completes `initialize`, lists the tools and calls `describe`, whose
result equals `describe --json`; a client asking for another revision is refused; an unknown tool
is a JSON-RPC error; nothing but the protocol reaches stdout.

**Dependencies.** 7.5, 7.6. **Files.** `crates/squint-cli/src/serve/`, `crates/squint-testkit/src/mcp_client.rs`,
`docs/`. **Size.** M.

### Task 7.7b: `check` over `serve`

**Description.** The `check` tool: images in any slot by absolute path (local transport only,
inside the client's roots narrowed by `[serve] roots`) or inline bytes up to
`limits.inline_bytes`, each with an optional `expect`; images out by the locator the call asks
for (`return_images`), image blocks only with `image_blocks: true`; the result as
`structuredContent` with `isError` true for `invalid` and `error`; the per-call cap a call may
lower, the session budget across the server's life (`[serve] session_budget_usd`), calls serial
by default (`[serve] concurrency`).

**Acceptance.** Against the scripted fake provider, the test client runs a check by path and by
inline bytes with identical results; a path outside roots is `invalid`, `input`; an exhausted
session budget is `error`, `budget`, before any provider call; image blocks appear only when
asked.

**Dependencies.** 7.7a, 7.4. **Files.** `crates/squint-cli/src/serve/`, `crates/squint-core/src/config.rs`.
**Size.** M.

### Task 7.8: Progress, cancellation and runs as resources

**Description.** With a progress token, a notification on every journal event and at least every
ten seconds while waiting, `progress` strictly increasing (steps, or elapsed seconds with no
total), the first naming the run resource; `notifications/cancelled` stops the provider call,
writes `run.json` as `cancelled` and returns nothing; resources `squint://runs` (newest first,
including `running`), `squint://run/<id>/run.json`, `…/report.json` and `…/journal`; an identical
retry within the cache policy answers from the decision cache with `cache: "hit"` and no spend.

**Acceptance.** A scripted slow provider produces heartbeats no more than ten seconds apart and
never a repeated value; a cancel mid-call leaves a readable `cancelled` run with spend to date; a
run is found through `squint://runs` after the client drops its connection; a repeated call costs
nothing.

**Dependencies.** 7.7b. **Files.** `crates/squint-cli/src/serve/`, `crates/squint-core/src/run.rs`.
**Size.** M.

### Task 7.9: `squint playground`

**Description.** Hands off to Wafflehouse by the launcher agreement (wafflehouse H1, accepted
2026-10-31 with squint's amendments): finds `wafflehouse` (`WAFFLEHOUSE_BIN`, `PATH`,
`~/.local/bin`), runs it with exactly `--tool squint` and keeps its exit code; when it is missing,
offers to install on a terminal and otherwise exits 69 with instructions. Production install is
unavailable until a release artifact is approved, so the installer is a testkit-only seam
(`--installer`, like `--provider-script`). squint carries no UI.

**Acceptance.** Fake executables show: the exact argv, a path with spaces run without a shell,
child exit codes and signals kept (128 + N), an invalid override failing with 69 and no
fall-through, the lookup order, a symlinked program accepted. Missing: no exits 0 and changes
nothing; yes installs into `~/.local/bin` and runs that path; installer failure exits 74 and runs
nothing; off a terminal exits 69; a release build exits 69 with no offer; extra arguments exit 64.

**Dependencies.** 7.1; wafflehouse H1. **Files.** `crates/squint-cli/src/playground.rs`,
`crates/squint-cli/tests/it/playground_cli.rs`. **Size.** S.

### Task 7.10: Conformance fixtures

**Description.** squint's fixtures in table-manners under `fixtures/squint/` (a describe
document and recorded `run.json` files for completed, fail, invalid, error and cancelled runs,
replayable offline), and a squint CI job that regenerates them against the pinned release and
fails on a difference. Findings that the contract got wrong are written up as table-manners v0.2
changes.

**Acceptance.** The fixtures validate in table-manners' CI; squint's job fails when a field
drifts; the v0.2 list is committed to table-manners, or recorded as empty.

**Dependencies.** 7.4, 7.5, 7.8. **Files.** table-manners `fixtures/squint/`, `scripts/`,
`.github/workflows/`. **Size.** S.

### Task 7.11: The MCP client on `rmcp` (bounded)

**Description.** With the server on `rmcp`, a time-boxed look at moving squint's in-house pack
client (ADR 0015) to `rmcp`'s client: the 4.4b failure modes (pagination, concurrency, a dying
pack, cancellation) against the SDK. An ADR records the outcome either way; a move is its own
change after M7.

**Acceptance.** ADR committed with the failure modes tried and the decision.

**Dependencies.** 7.7a. **Files.** `docs/adr/`. **Size.** S.

### Checkpoint M7
- [ ] Every conformance item of table-manners v0.1.0, core (7) and images (12), checked one by one.
- [ ] `docs/` and README describe `serve`, `describe`, `estimate`, `run.json` and the exit codes.
- [ ] Review and test skills (or checklist); owner review; merge; tag `m7`.

## Milestone M8: table-manners v0.2 (run association and budget sessions)

squint conforms to `table-manners` v0.2.0: wafflehouse's C4 as agreed by rascal and squint
(table-manners `PROPOSALS.md`, CONTRACT.md 2.3 and 4.1–4.6). The branch `m8-v0.2` starts from
`main`. Until the owner tags and publishes v0.2.0, squint builds against the crate at
`../table-manners` (its `v0.2-contract` branch, rascal the only writer) and hands its fixtures to
rascal as a directory; the tag waits on both tools' fixtures. After publication the dependency
becomes `=0.2.0` from crates.io.

### Task 8.1: The contract at v0.2

**Description.** `table-manners` at version `=0.2.0` with a path to `../table-manners` until it is
published; `schemas/contract/` and `SOURCES.toml` refreshed to the v0.2 schemas (`run-ref`,
`session`, `refusal` new); code moved to the v0.2 types with every new field set to its honest
v0.1-equivalent value (`session_id` and `client_run_id` null, `unresolved_usd` 0, one page of runs).

**Acceptance.** The schema copy test passes against the v0.2 crate; every existing contract test
validates against the v0.2 schemas.

**Dependencies.** table-manners `v0.2-contract` checked out at `../table-manners`. **Size.** S.

### Task 8.2: The session

**Description.** A random UUID (v4) generated at `serve` start; a `squint://session` resource (4.6)
from the session ledger: budget, spent, unresolved, reserved, remaining, never stale; every run a
server makes carries `session_id`, every CLI run null. `estimate` drops `session_budget_left_usd`.

**Acceptance.** The resource validates and is listed; two servers have different ids; a read
while a check holds the permit answers at once; spent and reserved move across a check.

**Dependencies.** 8.1. **Size.** M.

### Task 8.3: Correlation and the run reference

**Description.** `_meta["table-manners/client_run_id"]` read and validated, claimed atomically
when the request is accepted (before the permit wait), held for the session even if cancelled
before allocation; a reused id is `-32602` with the refusal data and the earlier run's reference
(null while it has none). The association (session id, client run id, action) is written into
the decision directory when it is allocated, before paid work, so a `running` runs entry carries
it. Every progress notification from allocation carries `_meta["table-manners/run"]`; the run's
URI leaves the message text.

**Acceptance.** Accepted, reused (queued and finished) and malformed ids; first and later
progress carry the run reference, heartbeats included; a `running` entry carries the pair; the
result's `run` carries it.

**Dependencies.** 8.2. **Size.** M.

### Task 8.4: Unresolved spend

**Description.** Each provider call is not sent (refused before it left: no credential, no
connection), known (an answer: charge or usage priced per unit; an auth rejection counts zero) or
unresolved (sent with no settling answer: cancelled or killed mid-call, timed out, a failure
without usage), counted at its worst case. `spend.unresolved_usd` in `run.json`, the session's
`unresolved_usd`, `spend.basis` describing the settled part.

**Acceptance.** Scripted cases for each class: a not-sent call costs nothing and releases its
hold; a call cancelled mid-flight counts its bound as unresolved; a run's `usd` never falls below
its `unresolved_usd`.

**Dependencies.** 8.1. **Size.** M.

### Task 8.5: Runs by page

**Description.** `squint://runs{?cursor}`: newest first by (started, id), a page of the newest N
read from disk (names sort by time, so older run files are never opened), an opaque base64url
cursor without padding, `next_cursor` null on the last page.

**Acceptance.** Three pages walk every run once; a run added during the walk is missed and then
seen by re-reading page one; a malformed cursor is a resource error.

**Dependencies.** 8.3. **Size.** S.

### Task 8.6: Fixtures, docs and the hand-off

**Description.** `fixtures/contract/squint/` regenerated for v0.2 and extended (a runs page, the
session, a reuse refusal, unresolved spend); written as a directory for rascal to copy into
table-manners `fixtures/squint/`; SPEC "External interface", `docs/agents.md`, the architecture
row and table-manners MAPPING (by rascal) updated. After v0.2.0 is published: the dependency
moves to crates.io `=0.2.0`.

**Acceptance.** table-manners' fixtures test passes on squint's files; `just check` passes.

**Dependencies.** 8.2–8.5. **Size.** S.

### Checkpoint M8
- [ ] Every v0.2 change in CONTRACT.md "Changes from v0.1" checked one by one on squint's side.
- [ ] Review and test skills; owner review; merge; tag `m8` once table-manners v0.2.0 is published.

## Milestone M9: table-manners v0.3 (readiness, bound estimates, renders)

squint conforms to `table-manners` v0.3.0: readiness without configuration (CONTRACT.md 3.1),
renders a host can embed (3.2), claims released when a request gets no run (4.4), estimates bound
to their inputs and checked caps (4.5), configuration owned by the session (4.6), list image
slots (10.1, 11.2) and the `credential`, `cap` and `stale_estimate` codes (2.2). The work list is
MAPPING.md's v0.3 table, squint column; every row is a task below. The branch `m9-v0.3` starts
from `main` at `m8` or later. Two behaviours go away rather than gain a second shape: an estimate
is never a run (no run directory, no runs entry, no `run.json`), and `cost.needs_env` is replaced
by per-action `readiness`. squint's own choices the contract leaves open are recorded in ADR 0018
(amendment of 2026-11-01).

**Building against the unpublished contract.** Until the owner tags and publishes v0.3.0, squint
builds against `table-manners = { version = "=0.3.0", path = "../table-manners" }` with
`../table-manners` a checkout of the contract's `v0.3` branch (the coordinating session the only
writer), as M8 did with `v0.2-contract`. The copied schemas under `schemas/contract/` are refreshed
from that checkout with `UPDATE_CONTRACT=1 cargo test -p squint-core --test contract_schemas`
(`SOURCES.toml` records the branch and the commit it was copied from); squint's fixtures are
regenerated with `UPDATE_CONTRACT_FIXTURES=1` and handed over as a directory for table-manners
`fixtures/squint/`. The tag waits on both tools' fixtures. A path dependency never reaches `main`:
the last task (9.11) moves to `=0.3.0` from crates.io, and the checkpoint requires it.

**Commit strategy for M9.** One commit per task on `m9-v0.3`, each leaving `just check` green,
Conventional Commits with the scope of the surface (`contract:`, `serve:`, `describe:`,
`estimate:`, `report:`). Rollback checkpoints are intermediate tags on the branch:

| tag | after | the branch then |
|---|---|---|
| `m9.1` | 9.1 | builds on the v0.3 types; behaviour as at `m8` apart from the shape changes v0.3 forces |
| `m9.2` | 9.4 | no configuration needed; readiness; estimates are controls, never runs |
| `m9.3` | 9.9 | every v0.3 behaviour on squint's side |
| `m9` (on `main`) | 9.11 and the checkpoint | on `=0.3.0` from crates.io |

Reverting follows the general rules (Commit strategy, Rollback). 9.4 and 9.5 revert together
(the binding check needs the estimate that issues bindings); 9.3 and 9.6 together when 9.6's
`invalid` credential state is in use; after any revert below `m9.3` the fixtures are regenerated
(9.10) before the next commit, since their test fails on drift. Resetting to `m9.1` abandons the
behaviour work and keeps the type migration. External state is the contract's: a fixture
directory handed over is superseded by the next one, never edited in place.

### Task 9.1: The contract at v0.3

**Description.** `table-manners` at `=0.3.0` by path to `../table-manners` (branch `v0.3`);
`schemas/contract/` and `SOURCES.toml` refreshed with `UPDATE_CONTRACT=1` (the new
`estimate.json`; `description.json` with `readiness`, render `kind` and `many`; `result.json` with
the new codes). Code moved to the v0.3 types with each new field at its honest v0.2-equivalent
value: `cost.needs_env` gone and every action's `readiness` built from what `executor` already
knows (the profile's credential variable `present` or `missing` by a local lookup of the name,
never the value); `report` render `kind: "server"` with `ready_timeout_s: 10`, `export` render
`kind: "document"`; `estimate` leaves `actions` (it stays a served tool in its v0.2 shape until
9.4, since v0.3 forbids the name in `actions`); `contract: "0.3.0"` everywhere. Fixtures
regenerated at 0.3.0, with `run-served-estimate.json` still produced until 9.4.

**Acceptance.** The schema copy test passes against the v0.3 checkout and fails on an edited
copy; every existing contract test (`contract_schemas`, `description`, `run_json`,
`contract_fixtures`, `contract_fixtures_served`) validates against the v0.3 schemas; `describe`
validates, including the schema's rule that a paid action declares `max_spend_usd` with
`exclusiveMinimum: 0`; no description names `needs_env`.

**Verification.** `just check`; `cargo tree -i table-manners` shows the path source.

**Dependencies.** table-manners `v0.3` checked out at `../table-manners`. **Files.** `Cargo.toml`,
`Cargo.lock`, `schemas/contract/`, `crates/squint-core/src/contract.rs`,
`crates/squint-cli/src/main.rs` (`executor`), `crates/squint-core/tests/it/{description,contract_fixtures}.rs`,
`crates/squint-cli/tests/it/{describe_cli,serve_check_cli,contract_fixtures_served}.rs`,
`fixtures/contract/squint/`. **Size.** S.

### Task 9.2: No configuration needed

**Description.** `--config <path>` becomes optional on `describe`, `serve`, `estimate` and `check`.
One lookup, `Config::locate`: the given path, else `./squint.toml`, else
`$XDG_CONFIG_HOME/squint/squint.toml` (`~/.config/squint/squint.toml`); the first that exists wins
and none is not an error (ADR 0018 amendment). `contract::description` takes an optional config:
with none it describes every shipped use case with the default image limits. `describe --json`
exits 0 with a missing or unparsable config (the config requirement says which, 9.3); `serve`
starts without one, its actions `needs_setup`, and a call to one is answered `invalid`, `usage`,
naming what is missing, with no provider call. Run directories of an unconfigured server go to
`--out`, else `$XDG_STATE_HOME/squint/runs` (`~/.local/state/squint/runs`), the place the config
would otherwise name (pending the coordinator's answer to question 2 below). `serve` still reads its
configuration once, at start (4.6); nothing reloads it.

**Acceptance.** `describe --json` with no config file anywhere on the lookup, with an unreadable
one and with one that does not parse: exit 0, a valid document, every action `needs_setup`; with
`--config` naming a missing file, the same, naming that path; `serve` with no config completes
`initialize`, lists the tools, answers `describe`, and refuses a check as `invalid`, `usage`; a
`./squint.toml` is found from the working directory and `--config` wins over it; `check` with no
config exits 64 naming the lookup.

**Verification.** `crates/squint-cli/tests/it/{describe_cli,serve_cli,config_cli}.rs` cases for
each row; `just check`.

**Dependencies.** 9.1. **Files.** `crates/squint-core/src/config.rs`,
`crates/squint-core/src/contract.rs`, `crates/squint-cli/src/{describe.rs,main.rs}`,
`crates/squint-cli/src/serve/mod.rs`, `docs/agents.md`. **Size.** M.

### Task 9.3: Readiness per action

**Description.** A `readiness` module computes each action's requirements by local inspection
only, never a network call, a paid step or a run:

| kind | name | state |
|---|---|---|
| `config` | the path the lookup found or would use | `missing`; `invalid` when it does not parse or a `${VAR}` it expands is unset (the setup sentence names the variable, never a value); `present` when it loads |
| `credential` | the catalog profile's `auth.env` (`ANTHROPIC_API_KEY`, …) | `missing` or `present` by name lookup; `valid` or `invalid` only from this session's provider answers |
| `model` | `[provider] model` as configured | `present` when the catalog has it (an unknown model makes the config `invalid`); `valid` after a known answer in this session that was not a rejection |
| `file` | the segmenter artifact path (`$XDG_CACHE_HOME/squint/models/<name>/<artifact>` from the record shipped with the pack) | `missing` or `present` (exists at the recorded size; never hashed here); listed only when the config enables a pack tool that needs it (`regions`, `classes`) |

With no config only the `config` requirement is listed: the credential and model are the
config's choice (ADR 0018 amendment). The fake profile lists no credential. `state` follows the
contract (`ready`, `needs_setup`; squint has no `unavailable` action today). The server keeps the
session's answers per credential name and model: `Services` reports each settled provider answer
(known and not a rejection: `valid`; 401 or 403: `invalid`), and `describe` is computed per call
from the startup description plus those answers, no longer cached whole at start. No requirement
ever carries a value; the secret scan covers `describe` output.

**Acceptance.** Each row of the table has a case; a fresh server shows `present` for a set key, a
scripted success then shows `valid` and a scripted 401 `invalid` on the next `describe`;
`describe` makes no provider call and writes nothing: a test runs `describe` (command and tool)
and `estimate` with a scripted provider whose call counter must stay at zero and with an
unroutable `base_url` and a key set, and asserts the output directory unchanged (CONTRACT 7 item 7's
proof); `describe --json` with a credential set never prints its value.

**Verification.** `crates/squint-core/tests/it/description.rs`,
`crates/squint-cli/tests/it/{describe_cli,serve_session_cli}.rs`, `just check`.

**Dependencies.** 9.2. **Files.** `crates/squint-core/src/readiness.rs` (new),
`crates/squint-core/src/{contract.rs,run.rs,lib.rs}`, `crates/squint-cli/src/{main.rs,serve/mod.rs}`.
**Size.** M.

### Task 9.4: `estimate`, the reserved tool

**Description.** `estimate` is no longer an action and never a run. The MCP tool takes
`{action, arguments}` (`action` an enum of the action names, `arguments` checked against that
action's input schema, images included) and returns `estimate.json` as `structuredContent`:
`effective.model` (the configured model), `effective.config_identity` (sha256 of the redacted
config's canonical JSON), `worst_case_usd` (the envelope `run::envelope` plans, the amount the
limits check reserves), `cap_usd` (the configured cap lowered by `max_spend_usd`), `allowed` and
`refused` in the order readiness, cap, budget, `readiness` (9.3), `session` (the same atomic
observation as `squint://session`), `expires` (one hour), and `binding`: sha256 over the
canonical JSON of the action, the arguments with every image replaced by its `file_sha256` (path
and inline of the same bytes bind equal) and its `expect` kept, the tool version, the config
identity and the session id. The server records each binding it issues with its expiry. An
estimate writes no directory, journal or runs entry, takes no permit, does not claim a
`client_run_id`, and still answers to roots like a check (`m7-review` C1). Errors before an
estimate exists are JSON-RPC errors. `squint estimate --json` prints the same document from the
command line (session per question 1 below); the v0.2 body's `decision_runs`, `requests` and
`attempts` move under `effective["x-squint"]`. Every test, doc and fixture that assumes an
estimate run changes in this commit: `serve_cli.rs::estimate_is_a_run_with_its_own_run_json`
becomes `estimate_is_never_a_run`; `run-served-estimate.json` leaves both generators' file lists
and the fixture directory; `estimate_cli.rs`, `serve_check_cli.rs`, `serve_correlation_cli.rs`,
`serve_session_cli.rs`, `usage_cli.rs`, the journal comment in `serve/mod.rs` (`resource`), SPEC
"External interface" (an estimate run, the `estimate` action, the command line), `docs/agents.md`
and README's contract paragraph.

**Acceptance.** An estimate over MCP and on the command line validates against `estimate.json`;
the output directory, `squint://runs` and the session's `spent_usd` and `reserved_usd` are
unchanged after it; `estimate` is not in `actions` and is in `tools/list`; two estimates of the
same call by path and by inline bytes have equal bindings, and any change to an image, an
argument or `max_spend_usd` changes it; a key unset gives `allowed: false`, `refused: readiness`;
a cap below the worst case gives `refused: cap`; a spent session gives `refused: budget`; every
recorded replay still spends within its `worst_case_usd`.

**Verification.** `crates/squint-cli/tests/it/{serve_cli,estimate_cli,serve_session_cli}.rs`,
`crates/squint-cli/tests/it/contract_fixtures_served.rs`, `just check`.

**Dependencies.** 9.3. **Files.** `crates/squint-core/src/contract.rs` (`estimate`, `binding`),
`crates/squint-cli/src/{main.rs,serve/mod.rs}`, the tests above, `SPEC.md`, `docs/agents.md`,
`README.md`, `fixtures/contract/squint/`. **Size.** M.

### Task 9.5: The limits check: `cap`, `budget`, `max_spend_usd` and the binding

**Description.** `Error::Cap` beside `Error::Budget`: the per-call ledger's refusals (the envelope
over the effective cap, and a retry the envelope can no longer cover) are `cap`; the session
ledger's are `budget` (ADR 0018 amendment). `max_spend_usd` above the configured cap is
`invalid`, `usage`, as a result before any paid step, never silently lowered:
`Config::lower_spend_cap` becomes `Config::call_cap`, which refuses it, on the command line
(`--max-spend-usd`, exit 64) and over MCP; a non-number or a value at or below zero is refused by
the input schema as a JSON-RPC error (and releases its claim, 9.7). Every paid action's
`params_schema` keeps `max_spend_usd` as `{type: number, exclusiveMinimum: 0}`, now pinned by a
test. `_meta["table-manners/estimate"]` is read on a paid call; the server recomputes the binding
as the call arrives and passes the outcome into `Services`; `run::check` fails the run with
`Error::StaleEstimate` at the limits check (after allocation, before the reservation, zero
spend) when the binding differs, was never issued by this session, or has expired. A call that
names no estimate meets the limits alone. `status_of` and the exhaustive mapping test gain `cap`
and `stale_estimate`, both never retryable.

**Acceptance.** A cap below the envelope ends `error`, `cap`, `retryable: false`, before any
provider call; a session that cannot hold it ends `error`, `budget`; `max_spend_usd` of 2 under a
cap of 1 is `invalid`, `usage`, spending nothing, and 0.5 lowers the cap; stale estimates for a
changed image, a changed argument, another session (a second server) and an expired binding
(a testkit-only clock seam) each end `invalid`, `stale_estimate`, with zero spend and a run
directory; a matching binding runs.

**Verification.** `crates/squint-core/tests/it/{ledger,terminal_states,run_json}.rs`,
`crates/squint-cli/tests/it/{budget_cli,serve_check_cli,serve_session_cli}.rs`, `just check`.

**Dependencies.** 9.4. **Files.** `crates/squint-core/src/{lib.rs,ledger.rs,run.rs,config.rs,contract.rs}`,
`crates/squint-cli/src/{main.rs,serve/mod.rs}`. **Size.** M.

### Task 9.6: `credential` failures

**Description.** A credential the call needs and does not have is `Error::Credential` (code
`credential`, never retryable, not sent, zero spend), no longer `Error::Auth` mapped to `usage`;
the other failures `provider_for` reports today under `Auth` (an unknown profile or model, an
unimplemented adapter) become `usage`. A provider's 401 or 403 is `credential`, never retryable,
counting zero (as 8.4 already prices it), and marks the credential `invalid` for the session's
readiness (9.3). `retryable` for the remaining `provider` failures becomes the tool's judgement
per occurrence: a transport failure is retryable when `TransportError::retryable` says so (a 5xx,
429 or timeout after the retries ran out), not for a 400, 404 or 422; a model-output failure stays
retryable.

**Acceptance.** Scripted 401 and 403: `error`, `credential`, `retryable: false`, `spend.usd` 0, no
retry attempted, the next `describe` shows the credential `invalid`; a key unset at call time:
`error`, `credential`, zero spend, no request left; a 529 after retries: `provider`,
`retryable: true`; a 400: `provider`, `retryable: false`; the exhaustive mapping test covers the
new variant.

**Verification.** `crates/squint-core/tests/it/{repair_retry,terminal_states}.rs`,
`crates/squint-cli/tests/it/{exit_codes,serve_check_cli}.rs`, the mapping test in
`contract.rs`, `just check`.

**Dependencies.** 9.3. **Files.** `crates/squint-core/src/{lib.rs,contract.rs,retry.rs,run.rs}`,
`crates/squint-cli/src/{main.rs,serve/mod.rs}`, `docs/agents.md`. **Size.** S.

### Task 9.7: A claim released when the request gets no run

**Description.** The case squint raised in its M8 review (S3): today `claim` holds a
`client_run_id` for the session even when the request ends in a JSON-RPC error before any run.
The claim becomes a guard that releases the id when the request ends in a JSON-RPC error with no
run (arguments `job_from_call` refuses, a malformed `max_spend_usd`, a failed `roots/list`, an
empty `many` slot, a server error) and keeps it once the run is allocated or when the request is
cancelled before allocation (4.4).

**Acceptance.** A request refused for invalid arguments, then the corrected request under the
same id: accepted, and its result carries the id; a request cancelled while it waits for the
permit, then a retry under the same id: refused with `client_run_id_reused` and `run: null`;
concurrent requests with one id still execute at most once; an id refused after allocation keeps
the earlier run's reference.

**Verification.** `crates/squint-cli/tests/it/serve_correlation_cli.rs`, `just check`.

**Dependencies.** 9.1. **Files.** `crates/squint-cli/src/serve/mod.rs`,
`crates/squint-cli/tests/it/serve_correlation_cli.rs`. **Size.** S.

### Task 9.8: `references` as a `many` slot

**Description.** The description's `references` slot carries `many: true`; the MCP input schema is
built from the slot's `many` (an array with `minItems: 1` of images in), not from its name; the
`x-squint` `references_slot` workaround goes (`self_description` stays); an empty list is a
JSON-RPC error that releases its claim; `images.in` lists each reference once, in the order given.

**Acceptance.** `describe` validates with `many: true` on `references` and false (absent) on the
others; a check with three references lists three `images.in` entries with slot `references` in
order; an empty list is refused before any run; no `x-squint` key mentions a list.

**Verification.** `crates/squint-core/tests/it/{description,contract_calls}.rs`,
`crates/squint-cli/tests/it/serve_check_cli.rs`, `just check`.

**Dependencies.** 9.1. **Files.** `crates/squint-core/src/contract.rs`,
`crates/squint-cli/src/serve/mod.rs` (`input_schema`). **Size.** S.

### Task 9.9: Renders a host can embed

**Description.** `squint report view`: `kind: "server"`, `ready_timeout_s: 10`, command
`["squint", "report", "view", "{run.dir}", "--port", "0", "--frame-ancestor", "{frame_ancestor}"]`.
Once listening it prints exactly one stdout line, `http://127.0.0.1:<port>/`, and nothing else
ever (the human sentence moves to stderr); it serves until stdin reaches EOF or it receives
SIGTERM (or SIGINT), then exits 0; `--frame-ancestor <origin>` (an `http` or `https` origin, no
path; anything else exits 64) adds `frame-ancestors <origin>` to every response's
`Content-Security-Policy`, and with no flag the policy is `frame-ancestors 'none'`; the loopback
host check stays. `squint report export`: `kind: "document"`; its only stdout is the absolute path
it wrote (the size moves to stderr); exit 0 means the file exists. `scripts/offline-smoke.sh` and
its fake in `scripts/test-checks.sh` keep the viewer's stdin open (a pipe from a sleeping writer)
and stop it with SIGTERM, since a background job's stdin is `/dev/null` and would now end it at
once; `docs/viewer.md` says so.

**Acceptance.** stdout of `report view` is one URL line and stays one line across requests; EOF on
stdin and SIGTERM each end it with exit 0 within a second; a response framed for
`http://127.0.0.1:4801` carries exactly that `frame-ancestors`, none for loopback in general; a
bad origin exits 64; `report export` stdout is one absolute path that exists; the offline smoke
passes on a built binary and fails on the fakes as before.

**Verification.** `crates/squint-cli/tests/it/{viewer_serve,report_cli}.rs`, `scripts/test-checks.sh`,
`just smoke`, `just check`.

**Dependencies.** 9.1. **Files.** `crates/squint-cli/src/{report.rs,viewer.rs}`,
`crates/squint-core/src/contract.rs` (renders), `scripts/offline-smoke.sh`,
`scripts/test-checks.sh`, `docs/viewer.md`. **Size.** S.

### Task 9.10: Fixtures, docs and the hand-off

**Description.** `fixtures/contract/squint/` regenerated for v0.3 by `contract_fixtures.rs`
(command-line documents) and `contract_fixtures_served.rs` (served documents), held by the same
drift test and rewritten with `UPDATE_CONTRACT_FIXTURES=1`, per CONTRACT 7 item 7:
`describe-unconfigured.json`; `describe-credential-missing.json`, `-present`, `-valid`, `-invalid`
and `describe-ready.json`; `estimate-allowed.json`, `estimate-refused-cap.json`, `-budget`,
`-readiness`; `run-stale-estimate-image.json`, `-argument`, `-configuration`, `-session`,
`-expired`; `run-cap.json`, `run-budget.json`, `run-credential.json`; a request refused with no
run whose id is then accepted (`run-accepted-after-refusal.json`), and one cancelled before
allocation whose id is still refused (`refusal-cancelled-before-allocation.json`); the v0.2 set
kept; `run-served-estimate.json` gone. The tool's own proof that `describe` and `estimate` made no
provider call is 9.3's test, named in `fixtures/contract/README.md`. Written as a directory for
table-manners `fixtures/squint/`, where the old estimate file is removed. SPEC "External
interface", `docs/agents.md` (readiness, the estimate flow with `_meta["table-manners/estimate"]`,
the codes), `docs/viewer.md`, README and the architecture row updated; table-manners MAPPING's
squint column is the coordinator's.

**Acceptance.** table-manners' `tests/fixtures.rs` passes on squint's directory at the `v0.3`
branch head; squint's drift tests pass; every fixture amount has at most six decimals.

**Verification.** `UPDATE_CONTRACT_FIXTURES=1` run once, then `just check` clean; in the
table-manners checkout, `cargo test --test fixtures` with the directory copied in.

**Dependencies.** 9.2–9.9. **Files.** `crates/squint-core/tests/it/contract_fixtures.rs`,
`crates/squint-cli/tests/it/contract_fixtures_served.rs`, `fixtures/contract/`, `SPEC.md`,
`docs/agents.md`, `docs/viewer.md`, `docs/architecture.md`, `README.md`. **Size.** M.

### Task 9.11: On crates.io

**Description.** After the owner tags and publishes table-manners v0.3.0: `table-manners = "=0.3.0"`
from crates.io instead of `../table-manners`; `SOURCES.toml` `tag = "v0.3.0"`, the copy re-checked
against the published crate; fixtures regenerated if the published crate differs from the branch
head they were made against, and re-sent.

**Acceptance.** `cargo tree -i table-manners` shows the registry source; the schema copy test and
the fixture drift tests pass; `scripts/check-contract-tag.sh` reports no newer tag.

**Verification.** `just check`; `just ci-local` before the checkpoint.

**Dependencies.** 9.10; table-manners v0.3.0 published (owner). **Files.** `Cargo.toml`,
`Cargo.lock`, `schemas/contract/SOURCES.toml`. **Size.** S.

### Checkpoint M9
- [ ] Every row of MAPPING.md's v0.3 table, squint column, and every item of CONTRACT.md
      "Changes from v0.2", checked one by one on squint's side.
- [ ] No test, doc or fixture still describes an estimate run or `cost.needs_env`.
- [ ] Review and test skills; owner review; merge; tag `m9` once table-manners v0.3.0 is
      published and 9.11 is in.

### Questions for the coordinator (contract v0.3, not changed here)

1. `estimate.json` requires a non-null `session_id` and a `session`, but a command-line run has
   `session_id: null`. squint plans a one-shot session for `squint estimate --json` (a fresh id,
   the config's budget, nothing spent), so a command-line binding never matches a served call.
   Should the two fields be nullable for the command line instead?
2. "A call to [an unconfigured action] fails with `invalid` (`usage`)" (5): as a result, which is a
   run with a run directory although no output directory is configured, or as a JSON-RPC error
   with no run (which releases the claim)? squint plans a result under `--out`, else
   `$XDG_STATE_HOME/squint/runs`.
3. An estimate with no configuration cannot name a model or price anything, yet
   `effective.config_identity` and `worst_case_usd` are required. JSON-RPC error, or an estimate
   with `refused: readiness`, `worst_case_usd: 0` and the identity of the empty configuration?
4. With configuration owned by the session (4.6), a changed configuration always comes with a new
   session, so `run-stale-estimate-configuration.json` cannot differ from `-session`. Is one
   fixture for both acceptable?
5. With no configuration, squint's credential depends on the profile the config would choose, so
   squint lists only the `config` requirement; the contract's example names `ANTHROPIC_API_KEY`
   and `squint init`, which squint does not have. Is the example illustrative only? And is a config
   that loads `present` (since `valid` is defined by provider answers)?
6. The exit status of a server render after stdin EOF or SIGTERM is unspecified; squint plans 0 for
   both. Worth one sentence in 3.2?

## Follow-ups (before or beside M9)

Open items from the M2 and M4 sections of `tasks/todo.md` that need no decision, and one CI
change. Each is one commit on its own branch `fu/<name>` from `main`, merged to `main` after
review (or onto `m9-v0.3` when it follows an M9 task, as marked). They change no contract field.

| task | parallel with M9? | why |
|---|---|---|
| F1 | before M9, or after 9.5 | `run.rs` resolve and reserve, which 9.5 edits |
| F2 | yes, any time | manifests and one test only |
| F3 | after 9.6 | `retry.rs` and the provider error path, which 9.6 edits |
| F4 | yes, any time | the Python pack only; complements 9.3's `file` requirement |
| F5 | before M9, or after 9.6 | the transport error message and the journal's error path |
| F6 | yes, first | CI configuration only; helps every later task |

### Task F1: Images that can pass `many_image_threshold` handled at resolve

**Description.** The 4.1 follow-up: a run whose tool images can carry the history past a model's
`many_image_threshold` fails preflight mid-run with `image_limit` after money is spent. At resolve,
compute the worst-case image count of the conversation (job images plus each tool's
`returns.max_images` times the tool-call budget); when it can pass the threshold, prepare every
image at `many_image_max_dimension_px` from the start and record it in `prep_version`, or, when
that cannot fit either (`max_per_request`), refuse before the first call with `image_limit`.

**Acceptance.** A scripted run whose tool budget can reach 21 images on a model with threshold 20
prepares at the smaller dimension from the first request and completes; one that would exceed
`max_per_request` is refused with zero spend and no request sent; runs that cannot reach the
threshold prepare exactly as before (recordings replay byte for byte).

**Verification.** `crates/squint-providers/tests/it/{prepare,replay}.rs`,
`crates/squint-core/tests/it/tool_turns.rs`, `just check`. **Dependencies.** none (sequence against 9.5). **Files.**
`crates/squint-providers/src/prepare.rs`, `crates/squint-core/src/run.rs`. **Size.** S.

### Task F2: Image arguments say what they take

**Description.** The `align` follow-up from the arm C run: the model named frames (`raw:candidate`)
where the manifest took image ids. Since 2026-10-29 an image argument accepts either and `align`'s
descriptions say so; the other image and frame arguments (`edge_similarity`, `change_map`, `view`,
`resize`, the pack's `echo_region`) are checked for the same, and a test pins that every
`image_args` property's description names both forms (an image id such as `candidate`, or its raw
frame `raw:candidate`). A manifest change changes the evaluation descriptor; noted in the commit.

**Acceptance.** The test fails when an image argument's description omits either form; the
manifests pass the three adapters' wire transformations as before.

**Verification.** a new case in `crates/squint-tools/tests/it/builtin.rs` and
`crates/squint-cli/tests/it/python_pack_manifests.rs`, `just check`. **Dependencies.** none. **Files.** `crates/squint-tools/manifests/*.toml`,
`tools/python/src/squint_tools/tools.py`, one test. **Size.** S.

### Task F3: One retry of a provider 400 that names no cause

**Description.** From the arm C run: Gemini answered `HTTP 400 INVALID_ARGUMENT` twice, minutes
apart, to request shapes that served 108 other items. Each adapter classifies a 400 body as naming
a cause (a field, a parameter, a limit) or not; one that names none is retried once, inside
`budget.retries` and the per-run retry reserve, and every other 400 stays final. The second 400 is
final.

**Acceptance.** A scripted bare 400 then a success completes with one retry; two bare 400s end
`provider`, `retryable: false`, after exactly one retry; a 400 naming a field is not retried; the
retry is accounted like any other (8.4).

**Verification.** the unit tests in `crates/squint-core/src/retry.rs`,
`crates/squint-providers/tests/it/{anthropic,gemini,openai}.rs`, `just check`. **Dependencies.** 9.6 when M9 is in flight, else none. **Files.**
`crates/squint-core/src/retry.rs`, `crates/squint-providers/src/{anthropic,gemini,openai}/`.
**Size.** S.

### Task F4: The segmenter pack fails clearly at `initialize` when its model is missing

**Description.** The M5 review's open item: with the segmenter artifact absent, each tool call that
needs it fails with a tool error. The pack checks the artifact at `initialize` (exists at the
recorded size) and, when a tool that needs it is enabled, answers with a JSON-RPC error naming the
missing file and the fetch command, so the harness ends the run with `error: resolve` at resolve
time, before any spend.

**Acceptance.** With the models directory empty, a run whose tools include `regions` ends at
resolve with `resolve` naming the file and spends nothing; with the artifact present, nothing
changes; a pack serving only `echo_region` initializes without it.

**Verification.** `tools/python/tests/test_server.py`, `crates/squint-tools/tests/it/python_pack.rs`,
`just pack-test`. **Dependencies.** none. **Files.** `tools/python/src/squint_tools/{server.py,models.py}`,
`tools/python/tests/test_server.py`. **Size.** S.

### Task F5: Provider error excerpts redacted before the journal and stderr

**Description.** From the M2 review: up to 2 KB of a provider's error body reaches the journal and
stderr without the redaction pass. The transport error's message goes through the run's redaction
list (raw and JSON-escaped, `Artifacts::redact`) before it is journaled or written into
`run.json`, and the command line's stderr uses the same function rather than its own raw-only
`redacted`.

**Acceptance.** A scripted error body containing the expanded secret, raw and JSON-escaped, never
appears in the journal, `run.json`, `report.json` or stderr; the secret scan test covers the case.

**Verification.** `crates/squint-core/tests/it/journal_artifacts.rs`,
`crates/squint-cli/tests/it/isolation.rs`, `just check`. **Dependencies.** 9.6 when M9 is in
flight, else none. **Files.** `crates/squint-core/src/{run.rs,artifacts.rs}`,
`crates/squint-cli/src/main.rs`. **Size.** S.

### Task F6: CI runs every test even after a failure

**Description.** A fail-fast nextest run on Linux stopped at test 95 of 623 and hid a second
failure. A nextest profile `ci` with `fail-fast = false` in `.config/nextest.toml`, selected in CI
by `NEXTEST_PROFILE=ci`, so CI reports every failing test (`--no-fail-fast` in effect) while local
runs keep failing fast; the two `just test` invocations run as separate CI steps so a failure in
the testkit pass does not skip the pass without the feature.

**Acceptance.** A deliberately failing test pushed to a scratch branch (or under `just ci-local`)
shows both it and every later test run, and both test passes report.

**Verification.** `just ci-local` with a planted failure, then without. **Dependencies.** none.
**Files.** `.config/nextest.toml`, `.github/workflows/ci.yml`, `justfile`. **Size.** S.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Provider APIs differ from the documentation read at M2 | High | Record fixtures early; every catalog entry cites source and date; preflight fails loudly; re-verify before `m2`. |
| Preserved-thinking prefix check rejects a request believed append-only | High | Provider-history invariant test in 1.7a; frozen tool list; `refuse` mechanism; record on the enforced account named in 0.4. |
| Owner labelling time dominates M3 and M5 | Medium | Toolkit drafts labels from construction; worksheets before each set; batches of 20 to 25; labelling overlaps M2 outside git. |
| Generated sets easier than real failures | Medium | Generator recorded per item; real failures added over time; stated in every results document. |
| Model artifact unavailable, incompatible or not licence-clean | Medium | 4.6 is an M4 exit prerequisite; fallback is an alternative segmenter with the same contract, by ADR. |
| Alignment infeasible at the needed accuracy | Medium | 4.0 spike with tolerances and failure fixtures; `align` may return `not_applicable` honestly. |
| Too few independent families for a conclusive gate | Medium | Family minimum frozen before unsealing; `inconclusive`, never post-hoc expansion. |
| `rmcp` does not fit | Low | 1.13 proves the lifecycle in-house; 4.4a decides by ADR. |
| Rust velocity on orchestration | Medium | Plain types, boxed traits, every task S or M, walking slice first. |
| Evaluation and recording spend | Medium | Caps and approval points in 0.4; `eval estimate` before every paid run; durable ledger with holds persisted before send. |
| Blind-set contamination | High | Labels external from creation; seal over the whole experiment before any run; gate verifies the seal; owner attestation; contaminated sets never reused. |
| Packaged crate missing embedded assets | Medium | 2.10a installs from packaged `.crate` files in a clean container and lists every asset. |
| Gateway provenance unavailable | Low | Candidates unshipped; nothing depends on them. |

## Parallelisation

Within a milestone, task branches may run in parallel when their dependencies allow: in M1, 1.2
and 1.3 after 1.1; 1.4a after 1.3 alongside 1.5a; 1.15 at any time after 0.1; 1.13 and 1.14 only
after 1.12. In M2, 2.6a and 2.7a after 2.5a; recordings after their implementations. In M3, 3.6
alongside 3.4 and 3.5; owner adjudication of 3.3a to 3.3e outside git while 3.4 and 3.5 are
built. In M4, 4.0 alongside 4.1; 4.4a after 4.1 alongside 4.2a. In M9, 9.7, 9.8 and 9.9 after 9.1
alongside the chain 9.2 to 9.5 (9.7 and 9.8 share `serve/mod.rs` with it, so they merge first,
small); 9.6 after 9.3 alongside 9.4 and 9.5. The follow-ups F2, F4 and F6 run beside M9 with no
shared files; F1 goes before M9 or after 9.5, F3 and F5 after 9.6 (table in "Follow-ups"). No code
work on a later milestone begins before the earlier milestone merges.

## Owner decision points (from 0.4)

| Before task | Decision |
|---|---|
| 2.1 | initial judge-model subset |
| 2.5b, 2.6b, 2.7b | recording cap; which Anthropic account (enforced thinking) |
| 2.8, 2.10b | gateway accounts and the release and gateway smoke cap |
| 3.2a | generators, licence, reuse of prototype assets |
| 3.2b, 3.3a to 3.3e, 5.5 | labelling capacity and batch schedule |
| 3.8, 4.5, 5.6 | spend cap per run, approved against `eval estimate` |
| 4.0, 4.6 | reuse approval for the prototype's views and segmenter export |
| 7.1 | publish table-manners v0.1.0 to crates.io |
| 7.9 | decided 2026-10-31: `wafflehouse --tool squint` by wafflehouse H1; the release installer is a later subgate |
| 9.1 | the coordinator's answers to the M9 contract questions (they may move fixtures, not tasks) |
| 9.2 | the default config lookup (`./squint.toml`, then `$XDG_CONFIG_HOME/squint/squint.toml`; ADR 0018 amendment) |
| 9.11 | publish table-manners v0.3.0 to crates.io |

## Open questions

1. Generators and licence for public sets (3.2a, ADR 0013).
2. Decided 2026-10-26: nothing from the prototype is reused (owner decision 11); 5.4a uses new
   synthetic pairs and 4.6 exports its own segmenter.
