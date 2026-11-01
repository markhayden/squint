# Spec: squint harness

Status: draft 5, 2026-10-25. Draft 1 came from the interview over `docs/brief.md`; drafts 2 to 5
answer Codex reviews 1 to 4 (`docs/reviews/`). Review 4 judged draft 4 "ready with changes" and
split the remainder into changes before planning (all made in this draft) and M1 deliverables
(also folded in where they were text). Nothing is built. The plan is
`PLAN.md`. Decisions are recorded in `docs/adr/`. This file is the source of truth for what is
being built; when a decision changes, this file changes first.

## Assumptions

Items 7 to 17 were positions taken on the reviewers' questions for the owner. The owner confirmed
7, 8, 9 and 13 on 2026-10-25 and approved draft 5 for planning; the rest stand as stated.

1. squint is self-contained. It ships its own tools; no consumer's existing tooling shapes it.
2. Consumers are the owner's own image-generation pipelines first, then anyone who installs the
   binary, then a hosted service on existing infrastructure.
3. Nothing needs to run in a browser except a static report viewer.
4. Backwards compatibility is not a goal before 1.0. Schemas, config, rules and reports carry
   versions so old reports stay readable, but no shims are kept for old shapes.
5. The owner reviews every image before it is published and every commit before it is pushed.
6. Provider facts cited here are as of 2026-10-25 and are re-verified against the providers'
   documentation and model lists in milestone M2 before being pinned into the model catalog.
7. **Hidden means unsure.** A protected obligation hidden behind something else is `unsure`
   unless its rule sets `hidden_ok = true`. The architectural profile sets it on structure, for
   any occlusion whatever caused it, because the owner's call in the prototype was "covered, not
   changed". Occlusion is never evidence that the structure behind it held; it only stops the
   item from forcing `unsure`.
8. **Evaluation policy.** Gates are scored with `on_unsure = "pass"` (the owner's pipelines let
   `unsure` through) and `on_error = "review"`, with errors counted separately under a ceiling.
   The harness never softens a confident violation into `unsure`.
9. **Admission gates.** A tool is admitted for a use case when, on a sealed set, paired against
   the visual-access control: missed failures and errors do not increase, detected failures
   increase by at least the predeclared margin with a positive paired lower bound from enough
   independent families, false rejects, unsure rate and cost stay under ceilings. Defaults are
   in "Gates" and are approved provisionally; the evaluation budget is set per matrix.
10. **Blind labels** are held by the owner outside the public repository; a blind set's manifest
    carries image hashes and one sealed label hash until the gate is scored.
11. **Source fidelity is generic.** The base rules protect named subjects and features and the
    job's requirements say what may change; the prototype's architectural rules ship as a named
    profile.
12. **First release scope.** Direct API-key access to Anthropic, Google AI Studio and OpenAI is
    the commitment. A gateway profile ships only when its routing and privacy conformance fixtures
    pass; otherwise it is deferred. Bedrock, Vertex and ChatGPT-plan OAuth are later milestones.
13. **Retention and trust.** Local run directories are kept until the operator prunes them. Tool
    packs are trusted code the operator installs; the harness excludes credentials from their
    environment, and the no-network rule for runs is an operator contract, not a sandbox.
14. **Tools see canonical images.** Deterministic tools always receive full-resolution,
    orientation-normalised, lossless image data and the derived frames they name; provider
    preparation applies only to what the vision model sees.
15. **MCP revision.** The first release implements MCP `2025-11-25` only. Later revisions are
    separate implementations with their own fixtures.
16. **Final turn without editing history.** The tool list sent to a provider is frozen for the
    life of a conversation. The final turn is enforced by the harness refusing to execute anything
    but `submit_verdict`, which needs no provider beta. Anthropic's append-only `tool_removal`
    block (a beta) is an optional cataloged mechanism, not a first-release dependency.
17. **Exemptions are model assertions.** The harness verifies that a cited exemption is enabled,
    waives the obligation and covers the target. Whether the exemption's condition actually
    holds and whether anything outside it changed is the model's judgement, stated in the
    disposition's reason, and measured by evaluation items with planted exemption-adjacent
    defects.

## Objective

An agentic harness for evaluating the integrity of images. A vision model makes the call; a set
of deterministic tools gives it better evidence than its eye alone; the harness supplies the
plumbing. It ships as one Rust binary plus an optional Python tool pack, configured by a TOML file.

**Inputs.** A job: one candidate image; zero or more reference images, each with a role and a
label; a use case; a rules file per use case, optionally extended by the consumer; structured
requirements and a free-text brief for per-run specifics.

**Output.** A JSON report: the verdict `pass | fail | unsure`, or an `error` with its class; a
disposition for every checklist item; findings with reason and location; every model turn and
tool call with the model-visible artifacts stored next to the report; the resolved model,
endpoint, prompt, rules, tool and schema versions; usage and cost.

**Use cases** (none privileged; each is a rules file, a tool set and evaluation sets):

1. Source fidelity. An edited or re-generated image kept what mattered in the original.
2. Reference consistency. A subject from one or more references appears faithfully in a new scene.
3. Reality checks. Extra or missing limbs, fused fingers, impossible shadows, garbled text.
4. Brief compliance. The image contains what was asked for and nothing it should not.
5. Product fidelity. A product shot keeps the exact label, logo, shape and colour.

**Users.**

- A pipeline that calls `squint check` as a subprocess or, later, over HTTP, and acts on the
  verdict and exit code.
- The owner, running evaluation matrices to decide which models and tools earn their place, and
  publishing the results.
- A tool author adding a deterministic tool and expecting the model to use it without prompt
  surgery.

**Success looks like** a baseline measured per model on public labelled sets for all five use
cases, each tool admitted, rejected or left inconclusive by a predeclared gate on a sealed set,
and a consumer able to go from `cargo install` to a verdict with one config file and one
environment variable.

## Decisions

| # | Decision | Choice | Why |
|---|---|---|---|
| 1 | Language and packaging | Rust. Cargo workspace: core library, thin CLI, room for server and Lambda crates. Release binaries for macOS arm64 and Linux x86_64/arm64, plus `cargo install`. | Small static binary for sidecars; the `image` crate for deterministic pixel tools; WASM stays open. TypeScript was the runner-up for hosting velocity. |
| 2 | Tool protocol | One registry, two transports: in-process Rust built-ins behind a `Tool` trait, and MCP over stdio for external packs. | Pixel tools stay cheap; ML tools stay Python; MCP gives listing and schemas for free. |
| 3 | Tool self-description | One manifest for every tool, rendered twice: schemas to the provider as native tool definitions, guidance into a Tools section of the system prompt by a versioned template. | Adding a tool is all it takes for the model to use it. |
| 4 | Config | TOML, one file per job type. `--set path=value` overrides single values. Secrets only from the environment. | Rust-native, comments, the prototype already used it. |
| 5 | Rules | Numbered plain-language obligations in TOML with id, kind, `matters`, `hidden_ok`, scope and, for exemptions, what they waive. Base sets ship in the binary; consumer files override by id, disable, or append. | Findings cite ids so code can aggregate by kind. |
| 6 | Verdict schema | Checklist plus findings. Every checklist item gets a disposition; the verdict is computed in code from code-owned obligation instances. | Silence cannot hide a missing disposition. |
| 7 | Evaluation sets | Manifest in repo, images elsewhere by SHA-256. Public images in a `squint-evalsets` repository; private sets by path. Blind labels sealed and held by the owner. | Review before publishing; honest gates. |
| 8 | Model access | Native wire adapters (Anthropic Messages, Gemini generateContent, OpenAI Responses) over direct HTTPS with recorded fixtures; endpoint profiles as data; a pinned model catalog as data. | Gateways and clouds become profiles, not adapters. |
| 9 | Layout and milestones | Monorepo. Skeleton, adapters, public sets, baseline, zoom, first ML tool. | The method needs a baseline before any tool. |

Sub-decisions:

- Images are accepted by path and by base64 in every input position. Over a later HTTP transport
  paths are refused ("External interface").
- A job has one candidate and any number of references; tools refer to images and frames by id,
  never path.
- The Python tool pack is published as a versioned provisioning bundle, installed once by
  `squint tools setup` into a locked environment, and launched from that environment offline.
- First evaluation sets are public, generated with image models, with defects planted by
  construction and labelled by hand: 20 items per use case to start, growing toward 50 to 100.
  Which image models generate them, and the licence, is decided at M3 and recorded per item.
- The first ML tool is the change-region finder. The built-in `view` and `align` tools come first
  and support flat and equirectangular images from their first milestone.
- The OpenAI adapter is built in M2 alongside Anthropic and Gemini, with API-key auth.
- Claude subscription OAuth is not supported. Anthropic's legal page restricts it to Anthropic's
  own apps and forbids third-party routing through Free, Pro or Max credentials. OpenAI's
  ChatGPT-plan flow is a later milestone (see "Auth schemes").
- fal is out of scope for the initial judge adapters. Admission of any provider requires verified
  multi-image input, tool calling with paired results, conversation continuity, a stable model
  identity, and documented error semantics.

## Architecture

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
│ squint-eval          │        │ squint-cli                  │
│  set manifests, hashes │        │  check · eval · tools · sets  │
│  matrix runner, gates  │        │  report · catalog · setup     │
│  metrics, bootstrap    │        └──────────────────────────────┘
└───────────────────────┘
```

Principles carried from the brief and the prototype:

- **One authority.** The verdict is computed in code from the model's structured submission and
  the code-owned checklist. Tool outputs are evidence. The model's opinion is recorded, never used.
- **Tools point, the model decides.** Tools are high-recall pointers and measurements, never
  deciders. No tool output is a hard gate.
- **Operational failure is error.** A provider or transport failure, a tool crash or timeout, a
  budget overrun, or an unrepaired model-output failure invalidates the run. A run with an
  operational error cannot produce a verdict, whatever the model submitted afterwards.
- **Nothing silent.** No silent resize, no silent model fallback, no silent cache hit in an
  evaluation, no silent change of auth, billing or upstream path.
- **Trust boundary.** Config, rules, the catalog, endpoint profiles and approved tool manifests are
  trusted. Image content, filenames, labels, the brief, and tool observations are data, delimited
  as such in prompts and never executed as instructions.
- **The core is a library.** It takes an HTTP client and a tool transport as inputs, so it runs
  under a CLI, a server, a Lambda handler or, later, WASM, and tests never touch the network.

## Domain model

### Job

```toml
[job]
use_case = "reference-consistency"
brief = "A golden retriever named Pip on the bridge of a starship, evening light."

# Structured obligations, same semantics as rules (see "Obligations").
[[job.requirements]]
id = "RQ1"
kind = "must_hold"
scope = ["candidate"]            # the image being judged
evidence = ["ref:pip"]           # the images consulted to judge it
text = "Pip's white chest patch is visible and the same shape as in the reference."

[[job.requirements]]
id = "RQ2"
kind = "allowed"
waives = ["RC3"]
scope = ["candidate"]
evidence = ["ref:pip"]
condition = "The clothing is a uniform worn over the body and the body shape under it is unchanged."
text = "Pip may wear a uniform; clothing over the body is an allowed change."

[[job.images]]
id = "candidate"
role = "candidate"
source = { path = "out/pip-bridge.png" }

[[job.images]]
id = "ref:pip"
role = "reference"
label = "Pip, the dog"
source = { base64 = "iVBORw0...", media_type = "image/png" }

[[job.images]]
id = "ref:bridge"
role = "reference"
label = "the bridge set"
source = { path = "refs/bridge.jpg" }
projection = "flat"           # optional override
expect = { file_sha256 = "3f9a…", width = 1600, height = 900, projection = "flat", topology = "bounded" }
```

An image may carry `expect`, the contract's image reference the caller expects the file to have
(see "External interface"): its `file_sha256`, width and height are verified before anything else
runs, and a mismatch, like a mismatched `sha256` pin, is `error: input` with nothing spent; a
differing `pixel_sha256` with the right file is a warning, since decoders disagree on lossy
formats.

Roles: `candidate` (exactly one), `source` (the original an edit was made from; at most one),
`reference` (any number). Image ids are plain tokens (ASCII letters, digits and `_ . : / -`) and the only way a tool or
the model refers to an image; the
harness resolves them through the job registry and never accepts a path or URL from the model.

Each image is decoded once with bounded dimensions (`images.max_decode_pixels`, default 50 MP)
and byte size (default 64 MB), orientation-normalised from EXIF, converted to sRGB 8-bit, and
kept as the **canonical image**: lossless, full resolution, recorded with its SHA-256 (original
bytes), canonical SHA-256 (decoded pixels), the contract's pixel digest (the stored pixels before
orientation), its dimensions both as stored and after orientation, and projection. Projection is `flat` or
`equirectangular`, taken from an explicit `projection` field, else from XMP `GPano` metadata,
else `flat`. A 2:1 aspect ratio alone does not make an image equirectangular; the CLI warns when
a 2:1 image is treated as flat.

Tools receive canonical images and the derived frames they name. The vision model receives
**prepared images** (see "Image preparation"). Every artifact records its canonical parent and
the transformation applied.

### Frames and locations

A **frame** is a coordinate space the model or a tool can point into. The harness keeps a
registry per run:

- `raw:<image-id>` for each job image, the canonical image.
- `aligned:<image-id>:<transform-id>` for an image warped into another image's frame by `align`,
  with the transform stored (homography for flat, rotation on the sphere for equirectangular) and
  the lossless warped pixels. Its coordinates are the other image's; its root is the image whose
  pixels it shows, and a location on it maps back to that image through the inverse transform.
- `crop:<call-id>:<n>` for each image a tool returned, with its parent frame and the mapping back
  to it (bbox or yaw/pitch/fov).

Every frame has a root image id and a projection. A **location** is a tagged record:

| `kind` | requires non-null | allowed on |
|---|---|---|
| `bbox` | `frame`, `bbox = [x0, y0, x1, y1]` normalised, `x0 < x1`, `y0 < y1` | any frame; a normalised box cannot wrap round the seam of an equirectangular frame, and one spanning the full width is the full circle |
| `polygon` | `frame`, `polygon` of 3 to 64 normalised vertices, simple | any frame; on equirectangular frames a polygon cannot wrap round the seam (a tool that needs both sides emits two; the registry cuts a view's footprint that crosses it to the side holding the region's centre and marks it clipped) |
| `view` | `frame`, `yaw` in `[-180, 180)`, `pitch` in `[-90, 90]`, `fov` in `[10, 120]` | equirectangular frames only |
| `whole_image` | `frame` | any frame; a global defect such as a wrong projection or a broken render |
| `absent` | `frame` (the image in which the thing is missing) | any frame |

All other coordinate fields are null. Yaw 0 is the image's centre column increasing to the
right; pitch 0 is the horizon row increasing upward; `fov` is the horizontal field of view of a
square perspective view. Every location maps deterministically back to the raw frame of its root
image through the registry. A mapping that falls partly outside the parent is clipped and flagged
`partial`; one that falls wholly outside is a semantic validation error. The viewer and the
localisation scorer compare in raw coordinates of the root image.

### Obligations

Rules and job requirements share one model, the **obligation**:

| Field | Meaning |
|---|---|
| `id` | two letters plus a number for rules (`SF1`), `RQ` plus a number for requirements; unique after merge |
| `kind` | `must_hold`, `must_not` (mandatory); `allowed` (exemption); `observe` (observational) |
| `text` | plain language, rendered to the model |
| `matters` | controls uncertainty only: a `cant_tell`, `hidden` or tentative result on this obligation yields `unsure`; default true for mandatory kinds, ignored for other kinds |
| `hidden_ok` | mandatory kinds only: `hidden` is recorded instead of unsure; default false |
| `scope` | image roles or ids whose correctness this obligation judges; omitted means `candidate`; an explicit empty list is invalid |
| `evidence` | image roles or ids consulted to judge it (a source, references); omitted means every `source` and `reference` image; consulting an image never makes it a target |
| `evidence_kinds` | tool evidence kinds whose regions bear on this obligation (see "Evidence binding"); omitted means the rules file's `default_evidence_kinds` |
| `waives` | exemptions only: the mandatory obligation ids this exemption may waive; may be empty, meaning the exemption states a permitted kind of change without waiving any protected obligation |
| `condition` | exemptions only: the condition under which the exemption applies, rendered to the model; the model asserts it in every `allowed_change` reason |

Kind determines aggregation; `matters` never makes an obligation mandatory and never makes an
observation count.

**Instances.** At resolve time every mandatory obligation's scope is resolved to explicit target
image ids (roles expanded, duplicates removed) and one **instance** is created per target:
`SF1@candidate`, `RQ1@candidate`. A mandatory obligation whose scope resolves to no target is a
resolve error; a check with no mandatory instance at all is a resolve error; non-applicability is
expressed by disabling the obligation, never by an empty expansion. Exemptions and observations
are resolved to targets the same way. An exemption applies to an instance only if the instance's
obligation id is in the exemption's `waives` and the instance's target is in the exemption's
targets. The target is code-owned; a model-chosen location never widens or narrows it.

**Evidence binding.** Each tool manifest declares the `evidence_kind` of the regions it returns
(`change_region`, `class_difference`, `alignment_failure`, `text_region`, `subject_match`,
`anatomy`). Each mandatory obligation lists the `evidence_kinds` that bear on it, defaulting to
the rules file's `default_evidence_kinds`; consumer requirements inherit the same default unless
they say otherwise. A region returned by a tool becomes evidence for exactly the mandatory
instances whose obligation lists that kind and whose target is the region's root image. The
binding is therefore trusted data from rules and config, covers consumer requirements, and never
depends on fixed ids inside a tool.

```toml
# crates/squint-core/rules/source-fidelity.toml (generic base; ships in the binary)
[rules]
use_case = "source-fidelity"
version = 1
requires_roles = ["candidate", "source"]
default_evidence_kinds = ["change_region", "class_difference", "alignment_failure"]

[[rule]]
id = "SF1"
kind = "must_hold"
scope = ["candidate"]
evidence = ["source"]
text = "Every protected subject or feature named in the job requirements keeps its identity, shape, position and extent relative to the source."

[[rule]]
id = "SF2"
kind = "must_hold"
scope = ["candidate"]
evidence = ["source"]
evidence_kinds = ["alignment_failure"]
text = "The candidate is derived from the source: same scene, same camera, same projection; nothing rotated, stretched or cropped away."

[[rule]]
id = "SF3"
kind = "allowed"
waives = []
text = "Changes the job requirements permit are not violations."

[[rule]]
id = "SF4"
kind = "observe"
text = "Any other change is noted with its location."
```

```toml
# crates/squint-core/rules/profiles/source-fidelity-architecture.toml (the prototype's venue case, a named profile)
[rules]
use_case = "source-fidelity"
profile = "architecture"
extends = "source-fidelity"
version = 1

[[rule]]
id = "SF1"
kind = "must_hold"
scope = ["candidate"]
hidden_ok = true     # dressing may cover structure; owner's decision from the prototype
text = "Permanent structure (walls, floors, ceilings, columns, pools, roads, built landmarks) keeps its shape, position and extent relative to the source."

[[rule]]
id = "SF3"
kind = "allowed"
waives = []          # states what may change; never waives SF1 or SF2
text = "Furniture, people, vehicles, decoration, textiles, lighting and vegetation may change, appear or disappear."

[hints.regions]
preserve = ["wall", "floor", "ceiling", "column", "swimming pool", "building", "road"]
ignore = ["chair", "table", "person", "plant", "curtain", "lamp", "car"]
```

In the architectural profile a change to furniture is reported as an `observed` finding citing
SF4 while SF4 is enabled; it is never an `allowed_change` on SF1, because SF3 waives nothing. An
exemption with `waives = []` supplies permission context only: it neither satisfies a mandatory
item nor overrides an enabled observation instruction. The prompt says so next to the rule, and
a submission that names it in `allowed_by` anyway is rejected with the disposition to use
instead (`held` where the obligation still holds). Models read "allowed" as an invitation to
cite the rule, so the rule text itself never asks them to cite anything. Structure covered by dressing is `hidden`
on SF1, recorded because `hidden_ok` is set. A job requirement of kind `allowed` with
`waives = ["SF1"]` and a `condition` is how a consumer permits a specific structural change.

**What `allowed_change` asserts.** When the model dispositions an instance `allowed_change` it
asserts two things in its reason: that the exemption's `condition` holds, and that nothing about
that instance changed beyond what the exemption permits. The harness verifies only that the
exemption is enabled, waives the obligation and covers the target; the two assertions are the
model's judgement, rendered as an explicit instruction, and every use case's evaluation sets
include items with a planted defect adjacent to an exemption (a uniform on the dog and a changed
muzzle) so misuse is measured.

Merge order: base set, then the `extends` chain, then consumer files in config order, then job
requirements. Same id replaces; `disabled = true` drops; new id appends. Validation after merge:
ids unique; every id in `waives` exists and is mandatory; every exemption with a non-empty
`waives` has a `condition`; every scope and evidence entry is a role or an image id present in
the job; every mandatory obligation resolves to at least one target; every `evidence_kinds` entry
is a kind some enabled tool produces or is dropped with a warning; `requires_roles` satisfied;
the job's use case matches the rules'; every `hints.<tool>` names an enabled tool or is dropped
with a warning. The merged list with provenance per obligation and the resolved instances go in
the report and the cache key.

Precedence: a mandatory instance stands unless a disposition cites an exemption that applies to
it. Exemptions never conflict with each other because each only adds permissions; mandatory
instances never conflict because each is dispositioned separately.

Rendering: a numbered list in a fixed Obligations section of the system prompt, kinds spelled out
in words, rules first and requirements second, each with its targets named, then the brief inside
a delimited data block, then the untrusted-content instruction.

Hints reach tools as a `hints` object the harness injects into the tool call, never the model;
the manifest declares which hints a tool accepts.

### Checklist

Before the first model turn, code builds the **checklist**:

- `chk:ob:<id>@<target>` for every mandatory instance.
- `chk:ref:<image-id>` for every reference image, depending on every mandatory instance that
  lists that reference in its `evidence`. A reference image that no mandatory obligation consults
  is a resolve error: the job passes an image nothing uses.

During the run the checklist grows with **evidence items**, `chk:ev:<call-id>:<n>`, one per
region a tool result presents (envelope `regions`). Crops (`images`) and transforms are views and
frames, not claims, and do not create items; measurements and summaries are context the model
reads. Each evidence item depends on the mandatory instances bound to the tool's `evidence_kind`
whose target is the region's root image (see "Evidence binding"). A region whose dependency set
resolves to empty is removed from the envelope before presentation and journaled as dropped, so
nothing shown lacks an owner.

Every verdict-bearing item therefore has at least one mandatory dependency; this invariant is
checked when the checklist is built and when evidence is ingested, and a breach is `error:
resolve` or `error: tool` respectively.

Growth is bounded by `budget.evidence_items` (default 24). A tool result whose regions would exceed
the bound is not presented: the run ends with `error: budget` before the model sees it. Evidence
items are never merged: every region the model is shown keeps its own item and its own
disposition. Items with the same root image whose raw-coordinate IoU exceeds 0.7 are rendered
with a cross-reference ("overlaps chk:ev:3:1") so the model can relate them; the ordering of items
is call order (batch order as issued, then region order within the result), never completion
order.

Each item carries code-owned fields the model cannot change: dependencies, root image, location,
and per dependency `matters` and `hidden_ok`. The model sees the current checklist in every
turn's user message and in full before the final turn.

A run that completes with every item dispositioned proves that every tracked claim shown to the
model was answered. It does not prove that nothing was missed; undetected defects are what the
evaluation measures.

### Submission schema

`submit_verdict` takes one argument. The **canonical schema** (version 1) is generated by
`schemars` from the Rust types and committed as `schemas/submission-v1.json`; every object has
`additionalProperties: false` and every property is required, with nullable types for optional
values. Each adapter owns a **wire schema** derived from it by a versioned, tested transformation
(see "Wire schemas"); the harness normalises wire payloads back to the canonical shape before
validation. The canonical shape:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["schema_version", "items", "findings", "opinion"],
  "properties": {
    "schema_version": { "const": 1 },
    "items": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["id", "disposition", "confidence", "reason", "allowed_by", "location", "evidence"],
        "properties": {
          "id": { "type": "string", "pattern": "^chk:(ob|ref|ev):" },
          "disposition": { "enum": ["violation", "held", "allowed_change", "hidden", "cant_tell"] },
          "confidence": { "enum": ["confident", "tentative"] },
          "reason": { "type": "string", "minLength": 1, "maxLength": 600 },
          "allowed_by": { "type": ["string", "null"] },
          "location": { "anyOf": [ { "$ref": "#/$defs/location" }, { "type": "null" } ] },
          "evidence": { "type": "array", "items": { "type": "string", "pattern": "^call:" }, "maxItems": 16 }
        }
      }
    },
    "findings": {
      "type": "array",
      "maxItems": 32,
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["obligation_ids", "disposition", "confidence", "reason", "allowed_by", "location", "evidence"],
        "properties": {
          "obligation_ids": { "type": "array", "minItems": 1, "maxItems": 8, "items": { "type": "string" } },
          "disposition": { "enum": ["violation", "allowed_change", "cant_tell", "observed"] },
          "confidence": { "enum": ["confident", "tentative"] },
          "reason": { "type": "string", "minLength": 1, "maxLength": 600 },
          "allowed_by": { "type": ["string", "null"] },
          "location": { "$ref": "#/$defs/location" },
          "evidence": { "type": "array", "items": { "type": "string", "pattern": "^call:" }, "maxItems": 16 }
        }
      }
    },
    "opinion": {
      "type": "object",
      "additionalProperties": false,
      "required": ["verdict", "reason"],
      "properties": { "verdict": { "enum": ["pass", "fail", "unsure"] }, "reason": { "type": "string", "maxLength": 600 } }
    }
  },
  "$defs": {
    "location": {
      "type": "object",
      "additionalProperties": false,
      "required": ["frame", "kind", "bbox", "polygon", "yaw", "pitch", "fov"],
      "properties": {
        "frame": { "type": "string" },
        "kind": { "enum": ["bbox", "polygon", "view", "whole_image", "absent"] },
        "bbox": { "anyOf": [ { "type": "array", "minItems": 4, "maxItems": 4, "items": { "type": "number", "minimum": 0, "maximum": 1 } }, { "type": "null" } ] },
        "polygon": { "anyOf": [ { "type": "array", "minItems": 3, "maxItems": 64, "items": { "type": "array", "minItems": 2, "maxItems": 2, "items": { "type": "number", "minimum": 0, "maximum": 1 } } }, { "type": "null" } ] },
        "yaw": { "type": ["number", "null"], "minimum": -180, "exclusiveMaximum": 180 },
        "pitch": { "type": ["number", "null"], "minimum": -90, "maximum": 90 },
        "fov": { "type": ["number", "null"], "minimum": 10, "maximum": 120 }
      }
    }
  }
}
```

Constraints a provider's wire schema cannot carry (`pattern`, `const`, numeric bounds, lengths,
item counts) are always enforced by canonical validation, so the wire schema is only ever looser
than the canonical one.

Semantic validation after schema validation, against this run's registry:

- Every current checklist item appears exactly once; no unknown item ids.
- Locations follow the table under "Frames and locations": the frame exists, the fields `kind`
  requires are non-null, all others null, geometry allowed on the frame's projection, mapping to
  the root frame not wholly outside.
- `violation` carries a location; `cant_tell` and `hidden` may.
- `allowed_change` on an item carries `allowed_by` naming an enabled exemption that applies to
  every mandatory instance the item depends on (obligation id in `waives`, target in the
  exemption's targets), and a non-empty reason; what the reason must assert is in "What
  `allowed_change` asserts". Any other disposition has `allowed_by = null`.
- Finding `obligation_ids` are known and enabled. A `violation` or `cant_tell` finding cites at
  least one mandatory obligation, and its location's root image must be a target of each cited
  mandatory obligation; the finding's instances are those. An `allowed_change` finding cites the
  mandatory obligations waived and an `allowed_by` that applies to each at that root image. An
  `observed` finding cites only `observe` obligations whose targets include the location's root
  image, has `allowed_by = null`, and never satisfies a checklist item.
- Every `evidence` id is a call from this run that completed with `ok` or `measured_nothing`.
- Items carry no obligation ids: the checklist owns them.

A schema or semantic failure gets one repair (see "Repair protocol"); a second failure is
`error: submission_invalid`.

### Aggregation

One decision run. For every item and finding, code evaluates one row **per mandatory instance it
depends on**, using that instance's own `matters` and `hidden_ok`, and keeps the strictest row
(fail over unsure over recorded). `observed` findings produce `recorded` directly.

| Disposition | Confidence | Instance condition | Row |
|---|---|---|---|
| violation | confident | mandatory | **fail** |
| violation | tentative | mandatory | unsure |
| cant_tell | any | matters | unsure |
| cant_tell | any | not matters | recorded |
| hidden | any | matters and not hidden_ok | unsure |
| hidden | any | otherwise | recorded |
| held | tentative | matters | unsure |
| held | tentative | not matters | recorded |
| held | confident | any | recorded |
| allowed_change | confident | exemption applies | recorded |
| allowed_change | tentative | matters | unsure |
| allowed_change | tentative | not matters | recorded |

Every dependency is mandatory by construction and every disposition, confidence and flag
combination has a row, so the table is total; the exhaustive unit test and the Rust function
cover the same domain. Verdict: `fail` if any row
is fail; else `unsure` if any row is unsure; else `pass`. An `unsure` is never produced from a
confident violation of a mandatory instance, and nothing in the model's `opinion` is read.

### Decisions and runs

A **decision** is what `check` returns. It consists of `budget.decision_runs` (1 or 2)
independent **runs** of the state machine over the same resolved job, aggregated:

| | pass | unsure | fail | error |
|---|---|---|---|---|
| **pass** | pass if agree, else unsure | unsure | fail | error |
| **unsure** | unsure | unsure | fail | error |
| **fail** | fail | fail | fail | fail |
| **error** | error | error | fail | error |

"Agree" compares static items (`chk:ob`, `chk:ref`) by id on disposition, confidence and, for
`allowed_change`, `allowed_by`. Evidence items are matched one-to-one: a pair is eligible only
when both items have the same root image and identical dependency sets; eligible pairs are sorted
by descending IoU in root coordinates with a 0.5 floor, then by run 1 call and region order, then
by run 2 call and region order, and assigned greedily; matched pairs must agree on disposition,
confidence and `allowed_by`; an unmatched evidence item disagrees unless its disposition is
`held`. Both runs are kept in full in the report.

Caching operates only at the decision boundary: on a hit the whole decision with all its runs is
returned; on a miss every run executes with cache reads and writes disabled, and the complete
decision is written once, only if no run ended in error.

The consumer policy `[policy] on_unsure` and `on_error`, each `fail | pass | review`, applies
only to the exit code; the report carries the raw verdict and the policy outcome.

### Report and journal

Every run writes an append-only **journal** (`journal.jsonl`) from the first event, so a failure
at any step leaves a durable record: `resolved`, `prepared`, `reserved`, `turn_sent`,
`turn_received`, `retry`, `attempt_failed`, `turn_excluded`, `tool_called`, `tool_returned`,
`evidence_dropped`, `repair`, `submission`, `verdict`, `error`, `cancelled`. Every artifact a journal event references is
written and fsynced before the event is appended. Artifacts in `artifacts/` by content hash:

- model-visible request and response bodies per turn, redacted (auth headers never stored; image
  bytes replaced by their artifact hash), bounded at 2 MB per body; a larger body is truncated
  with a marker and the SHA-256 of the full body recorded;
- every image sent to the model, exactly as sent, and every image a tool returned;
- every structured tool result in full.

SIGINT or SIGTERM during a run appends `cancelled` with the state reached and exits 130 or 143
("External interface").

A decision directory (`decision-<unix ms>-<pid>/`) holds `decision.json` (the header: everything
the report needs that no run journal carries, including the planned envelope; written before the
first run), one `run-<n>/` per run with its journal, artifacts and frames, `report.json`, and
last `run.json`, the contract's result (see "External interface").

The **report** (`report.json`, `report_version` pinned) is derived from the header and the
journals at the end, or by `squint report rebuild` from a partial journal, whose terminal status
is then `incomplete`: the job as resolved (ids, roles, labels,
hashes, dimensions, projection, prepared sizes); merged obligations with provenance and instances;
checklist with its growth and drops; every turn (stop reason, usage, cost, provider request id,
artifact hashes, whether it entered provider history); every tool call (id, tool, version,
arguments, status, duration, artifact hashes); submission(s); the aggregation rows; verdict and
policy outcome; config as resolved and redacted; adapter, endpoint profile, upstream provenance,
resolved model id and API version; prompt template version; harness version; cache status; wall
time; terminal status.

The static HTML viewer loads a run directory, shows every image the model saw, every tool result,
every location overlaid in raw coordinates through the frame registry, and the aggregation
table. It has no build step and no network access, and declares which `report_version`s it reads.

## Run state machine

Counters, all from `[budget]`: `tool_turns` (default 4), `tool_calls` (default 8),
`evidence_items` (24), `repairs` (1, run-wide), `repair_output_tokens` (the output allowance a
reissued request may use, default 2 × `max_output_tokens`), `retries` (3, per request,
transport only), `decision_runs` (1), `spend_cap_usd`, `request_timeout_s` (300),
`tool_timeout_s` (120).

Two histories exist: the **journal**, append-only, which records everything; and the **provider
history**, the sequence of replayable turns sent to the model. A turn enters provider history only
if it is replayable under the adapter's rules; a non-replayable turn is journaled with
`turn_excluded` and never sent back.

1. **Resolve.** Load config, merge obligations, resolve instances, load and decode images into
   canonical images, build frames, build the checklist, connect tool packs and freeze their
   manifests, render the system prompt. Any failure is `error: resolve` with a journal, except an
   image that is not the one the job names (a `sha256` pin or an `expect` that does not match),
   which is `error: input`.
2. **Prepare.** Produce prepared images for the model (see "Image preparation") and preflight the
   first request against the catalog's limits. Compute the cache key (see "Cache key"). If
   caching is on, this is not an evaluation, and this is the decision boundary, return the cached
   decision with `cache = "hit"`.
3. **Reserve.** Compute the decision envelope (see "Cost ledger") and reserve it. If it does not
   fit under the job cap or the evaluation ledger, `error: budget` before any request.
4. **Tool turn** (repeat while `tool_turns` remain). Send the provider history with all enabled
   tools including `submit_verdict`. The first look is tool turn 1: the system prompt, every job
   image at prepared size labelled with id, role, label, frame and dimensions, the checklist, and
   the instruction to inspect and submit. On the response, classify it first:
   - **Complete with tool calls.** Validate every call against its model schema and the registry
     before executing any. If the batch exceeds the remaining `tool_calls`, execute none, answer
     every call with a provider-native error result saying the call budget is exhausted, and go to
     the final turn. Otherwise reserve the batch's allowance, execute (concurrently only for packs
     declaring `concurrent = true`, serially otherwise) with per-call deadlines, and return all
     results in one message in the order issued. `invalid_args` is answered as a tool error to the
     model; `measured_nothing` and `not_applicable` are evidence; a crash, timeout, malformed
     envelope or bound breach is `error: tool`.
   - **Complete with `submit_verdict` alone** and no pending calls: validate; on success go to
     **Aggregate**. **Mixed** with other calls: answer every call with a provider-native error
     result, then the repair.
   - **Non-replayable** (truncated or incomplete, a refusal, or text with no call): the repair.
   - **Provider error** after retries, or a timeout: `error: provider`.
5. **Final turn.** When `tool_turns` or `tool_calls` are exhausted, send one request with the
   same frozen tool definitions, the full checklist, and an appended instruction that only
   `submit_verdict` will be executed and the model must submit now (see "Final turn mechanics").
   A `submit_verdict` alone is validated as in step 4. Any other tool call is answered with a
   provider-native error result saying the tool is no longer available, and the repair is
   consumed; a repair after the final turn carries the same instruction. Then
   `error: no_submission`.
6. **Aggregate.** Semantic validation (repair if unused), then the aggregation table. Journal the
   submission and verdict.
7. **Record.** Derive the report, write artifacts, release the tool pack lease. The decision
   writes the cache after all runs if none errored.

Repairs cannot erase an operational error: once the journal holds an `error` event the run's
terminal status is that error and no verdict is written, even if a later submission validates.

Provider history is append-only. Opaque blocks (thinking, thought signatures, reasoning items) are
passed back exactly as received by the adapter that produced them. Nothing is edited in an
earlier turn.

### Repair protocol

One repair per run, in one of two forms chosen by the response class:

- **Replayable response rejected** (invalid submission, mixed batch): the response enters provider
  history; the harness appends one provider-native error result per call id in order, stating the
  reason (the validator's message, or "mixed batch"), then the repair instruction in the same user
  message where the adapter allows it, otherwise as the next user message, and sends. The repaired
  turn does not consume a tool turn.
- **Non-replayable response** (truncated output, incomplete status, refusal, text without a
  call): the response is journaled with `turn_excluded` and does not enter provider history. The
  harness reissues the preceding request from the last valid provider history under the
  adapter's reissue policy: for truncation, with the output allowance raised to
  `repair_output_tokens`; for a refusal or a call-less text, with the repair instruction appended
  as a new user message that tells the model its previous output was discarded. The reissue
  allowance is part of the decision envelope.

A second repairable failure in the run is `error: model_output` (stop reason recorded) or
`error: submission_invalid`. Truncated responses are never dispatched, even if a call inside them
parses. Adapter fixtures cover truncated arguments, an invalid submission, a mixed batch, a
refusal and a call-less text, for each form.

### Retries

Transport-level only: connection errors, timeouts before any byte, HTTP 408, 409, 429 and 5xx.
Exponential backoff with full jitter from 1 s, capped at 30 s, honouring `retry-after` when
present, at most `budget.retries` attempts after the first. A 4xx other than those is
`error: provider` immediately. Each attempt is a billable attempt in the ledger.

### Final turn mechanics

The `tools` array sent to a provider is declared on the first request and never changes for the
life of the conversation: Anthropic binds replayed thinking blocks to the system prompt, the tool
definitions and every earlier message, and an edited prefix is rejected for enforced accounts.
Restricting the final turn therefore never edits the tool list. The catalog names the mechanism
per model:

- `final_turn = "refuse"` (default, no beta): the instruction is appended as a mid-conversation
  `role: "system"` message where the catalog says the model supports them, otherwise as a text
  block after the last tool results; the harness executes nothing but `submit_verdict` and answers
  any other call with an error result.
- `final_turn = "forced"`: where `forced_tool_choice = true`, the request additionally sets the
  provider's forced tool choice for `submit_verdict`; `tool_choice` is a request parameter, not
  part of the bound prefix.
- `final_turn = "tool_removal"`: Anthropic only, behind the `mid-conversation-tool-changes`
  beta the catalog pins; the harness appends a `role: "system"` message with one `tool_removal`
  block per tool other than `submit_verdict`, leaving the definitions in `tools`, and still
  answers a stray call with an error result.

The harness marks the final turn by appending the instruction as a `System` message; the adapter
renders it by the mechanism in force. The mechanism is the first the catalog lists for the model
unless `[provider] controls = { final_turn = "…" }` names another supported one; the thinking
prefix check is `controls.prefix_mismatch_behavior = "error" | "drop_block"` on models with the
thinking-binding beta (owner decision 2 sets `error` while recording). Each mechanism has adapter
fixtures, including the transition into the final turn and a stray call during it. A
configuration whose model supports none is rejected at resolve time.

## Providers

### Adapter trait

```rust
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;                                              // "anthropic-messages"
    fn send<'a>(&'a self, req: TurnRequest<'a>) -> SendFuture<'a>;     // one round trip, no retries
    fn pricing(&self) -> Option<&dyn Pricing>;                         // the catalog entry
    fn prep_version(&self) -> &str;                                    // part of the cache key
    fn prepare_image(&self, id: &str, pixels: &DynamicImage) -> Result<Prepared>;
    fn recipient(&self) -> Option<Recipient>;                          // profile, host, upstream, model
}
```

`Provider` is dyn-compatible and used as `Box<dyn Provider>`; `send` returns a boxed `Send`
future. The adapter is built from the catalog entry (capabilities, limits, prices, betas), the
profile, the credential and an injected transport, so the capability, wire-schema and preflight
steps of the request are the adapter's own: it transforms the tool schemas once (`wire`), runs
preflight over every outgoing request (`prepare::preflight`) and maps controls as the catalog
allows. `TurnRequest` carries the system prompt, the history (the harness's typed view, in which
adapter-owned opaque blocks appear as `Opaque { adapter, block }` and the final-turn instruction as
a `System` message), the frozen tool definitions, the output token limit and the prepared bytes of
every image the history names. `TurnResponse` carries text blocks, tool calls (adapter call id,
name, parsed arguments), the assistant content in the provider's order including the opaque blocks
to echo, the stop reason, usage (uncached, cache-read and cache-write tokens), and the provider
request id. The harness appends that content to the history unchanged.

The transport is injected (`http::Client`, reqwest with rustls in the binary; `replay::Player` for
recorded conversations, `replay::Recorder` around the client when recording). `squint check
--replay <dir>` runs a decision against a recording without a credential; `--record <dir>` writes
one, secrets stripped.

### Wire schemas

Each adapter derives the wire schema for `submit_verdict` and for every tool from the canonical
schema by a versioned transformation committed with tests. Constraints removed from a wire schema
stay in canonical validation.

- **Anthropic Messages.** Where the catalog says `strict_tools = true`: remove constraints the
  provider's strict mode does not accept (string lengths, numeric bounds, `pattern`, item counts,
  `const`), keep closed objects and required properties, and validate the combined tool set
  against the provider's schema complexity limits; otherwise send the canonical schema. The exact
  unsupported list is read from the provider's documentation at M2 and pinned in the catalog.
- **Gemini generateContent.** Declare functions with `parametersJsonSchema` carrying the subset
  the catalog pins as supported, with the same removals as above where needed.
- **OpenAI Responses, strict.** Every object `additionalProperties: false` and every property
  required, optional values nullable; unions only in the documented `anyOf` form; remove
  unsupported keywords per the catalog.

Common to every adapter (`squint-providers::wire`, `WIRE_VERSION`): `oneOf` becomes `anyOf`; a
union of plain enums becomes one enum; metadata (`$schema`, `title`) and `format` values the
provider does not list are dropped; the constraint keywords the catalog names for the model are
removed; `allOf`, `not`, conditionals, `patternProperties`, external references and any keyword
outside the wire subset are refused. Gemini receives references inlined (its documentation does not
state that `$defs` is accepted), with `const` as a one-value enum. Every removal and rewrite is
recorded with the schema as a JSON pointer, and the transformed `submit_verdict` schema per adapter
is committed under `fixtures/schemas/`.

The harness normalises a wire payload back to canonical form before validation: `null` is dropped
from properties the canonical schema leaves optional (OpenAI's nullable emulation) and enum strings
are restored to the canonical spelling (Anthropic does not guarantee their capitalisation).
`submit_verdict` and every enabled tool schema must pass the wire transformation for the selected
model at resolve time, and the set must fit the provider's limits (Anthropic strict mode: 20 strict
tools, 24 optional and 16 union-typed parameters per request; OpenAI: 5,000 properties, 10 levels,
1,000 enum values, 120,000 characters per schema); a tool whose schema cannot be represented is
refused with the reason, before any billable request.

### Protocol requirements

Verified at M2 and encoded in fixtures:

- **Anthropic Messages.** `tool_use` blocks paired to `tool_result` blocks by id, all results for
  one batch in a single user message in the order issued, `is_error` for failed calls. Thinking
  blocks echoed unchanged. Controls (effort, thinking, sampling) and forced tool choice are
  per-model capabilities read from the catalog: at the time of writing Opus 5.5 and Sonnet 5.5
  reject forced `tool_choice` and sampling parameters while Haiku 5.5 accepts forced tool choice.
  A response with an incomplete tool block is non-replayable and handled by reissue, as the
  provider documents. Refusal stop reasons are non-replayable. No server-side fallback
  parameters are sent.
- **Gemini generateContent.** `functionCall` parts answered by `functionResponse` parts matched by
  the call's `id`. A response without ids is accepted only for models the catalog flags
  `function_call_ids = false`, and then only when it contains exactly one call; otherwise
  `error: provider`. Thought signatures preserved by echoing the original parts unchanged.
  Function-calling mode from the catalog (`ANY` restricted to `submit_verdict` on the final turn
  where supported).
- **OpenAI Responses.** Function tools with the strict wire schema; `function_call` items answered
  by `function_call_output` items matched by `call_id`; reasoning items echoed; `store = false`;
  `incomplete` terminal status is non-replayable.

### Image preparation

The catalog records per model: accepted formats; maximum accepted dimensions and byte size per
image; the native size at which the provider stops resizing; the per-request image count limit
and any stricter per-image limit above a count; the per-request byte limit; the image token
estimate; provider-specific detail controls. Preparation is deterministic and versioned
(`prep_version`, currently `1`): fit the canonical image's long edge to the native size (or the
config's lower `images.max_long_edge`) with Lanczos3, never upscaling; encode JPEG at quality 90,
or PNG when the image carries an alpha channel; write no metadata; record the source and prepared
dimensions and the prepared SHA-256. Each provider reports its `prep_version` (the scripted
provider sends canonical pixels and reports `canonical`), and the cache key carries it. Preflight runs against the whole outgoing provider history on every turn,
counting historical and tool-result images; a request that would exceed any limit is
`error: image_limit`, never a silent downscale. Tool images are prepared the same way before they
reach the model; tool manifests bound their count and size so the bound is computable.
Provider-side processing the documentation does not describe is recorded as unverified in
`docs/provider-data.md`.

### Endpoint profiles

```toml
# providers/profiles/vercel-ai-gateway-anthropic.toml   (candidate; unshipped until fixtures pass)
[profile]
id = "vercel-ai-gateway-anthropic"
adapter = "anthropic-messages"
base_url = "https://ai-gateway.vercel.sh"
path = "/v1/messages"
auth = { scheme = "bearer", env = "AI_GATEWAY_API_KEY" }
model_id = { prefix = "anthropic/", virtual_models = false }
headers = {}
body = { providerOptions = { gateway = { only = ["anthropic"] } } }   # documented; no model list is ever sent
upstream = { allow = ["anthropic"], provenance = "unresolved" }       # mechanism verified when fixtures are recorded
streaming = false
error_mapping = "anthropic"
fixtures = "fixtures/profiles/vercel-ai-gateway-anthropic/"
```

A profile is the full transport description: adapter, base URL, path template (which may place
the model in the path), auth scheme and credential source, header and body transformations, model
id mapping, region or project requirements, streaming framing, error normalisation, redirect
policy (no cross-host redirects are followed), and the **upstream contract**: the allowed upstream
providers and how the response proves which one served the request. A gateway profile is shipped
only if it can enforce its upstream allowlist in the request and the harness can read provenance
from the response; missing, malformed or disallowed provenance is `error: provider`. Cloudflare's
provider-native endpoints have a single upstream by construction and their provenance is the
endpoint itself. Every shipped profile has conformance fixtures recorded once against the live
endpoint, including the routing and provenance behaviour.

Shipped at first release (API-key schemes only): `anthropic`, `google-ai-studio`, `openai`.
Candidates, shipped only when their fixtures pass: `vercel-ai-gateway-anthropic`,
`cloudflare-ai-gateway-anthropic`, `cloudflare-ai-gateway-google`. Later milestones, each with
an ADR: `bedrock` (Messages API with SigV4), `vertex` (Google-managed Claude, model in path,
`anthropic_version` in body), `openai-chatgpt-plan`.

### Auth schemes

| Scheme | Credential source | First release |
|---|---|---|
| `api-key-header` | one environment variable | yes |
| `bearer` | one environment variable | yes |
| `gateway-header` | environment variable for the gateway token plus the provider's scheme | with the gateway profiles |
| `aws-sigv4` | the AWS default credential chain | later |
| `gcp-adc` | Application Default Credentials | later |
| `openai-chatgpt-plan` | OAuth with PKCE; refresh token in the OS keychain or a 0600 file under the config directory; registered client id, granted inference scope, `store = false` and streaming per OpenAI's contract | later |

"Secrets only from the environment" holds for the first release. Later schemes extend the sources
to the provider's standard credential chain and, for OAuth, to a protected local token file; each
is documented in `docs/provider-data.md`. No scheme is ever substituted for another silently; a
missing credential is `error: auth`.

### Model catalog

`crates/squint-providers/catalog.toml`, versioned, part of the cache key. Per model id: adapter; context and
output limits; the image fields above; prices per million input, output and cached tokens, and
any per-image price; `verified` date and source URL; capabilities: `forced_tool_choice`,
`strict_tools`, `strict_unsupported_keywords`, `function_call_ids`, `parallel_tools`,
`system_messages_in_history`, `final_turn` (the mechanisms supported, see "Final turn
mechanics"), `sampling`, `prefill`, the controls a config may set (name, allowed values,
default: the vendor's thinking control and, where one exists, the image detail control), required
headers or betas. Fields the documentation does not state are listed per entry under `unverified`
and recorded in `docs/provider-data.md` until recordings confirm them. The file also carries the
shipped endpoint profiles (adapter, endpoint, API-key scheme, static headers). Config settings not
supported by the model's capabilities are rejected at resolve time; the test-only `fake` profile
and `fake-model` pass only in binaries built with the `testkit` feature. Candidate first
entries, re-verified at M2: `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5`,
`gemini-4-argon`, `gemini-3.8-flash`, and the current OpenAI vision models. Preview models are
excluded.

### Cost ledger

Two ledgers: per job (`budget.spend_cap_usd`) and per evaluation (`matrix.spend_cap_usd`),
shared across concurrent decisions with atomic operations. The unit of reservation is the
**decision envelope**: the upper bound of every request the decision can make (per run: tool
turns, the final turn, every repair the budget allows (`repairs`) at `repair_output_tokens`;
times decision runs), each bounded by the history it could carry (prior prompts, prior model output at the
output allowance, tool outputs at their manifest byte and image bounds, request framing, image
token estimates) at catalog prices, plus any per-image price, with every request counted once,
plus a per-run **retry reserve** of `retries` times the largest request bound. A retryable failure
is charged at its bound, so the reserve pays for `retries` failures anywhere in the run; a retry
the balance cannot cover is refused and the run ends with `error: budget` rather than overrunning
the envelope. The envelope is reserved once at step 3.

Each billable attempt transfers its bound from the envelope's unallocated balance to an attempt
hold; total reserved spending never increases. When the provider reports usage, the hold is
settled to actual cost and the difference returned to the envelope; when usage is unknown
(timeout, connection loss after send) the hold is charged at its bound. At the end of the
decision the unallocated balance is released. A request whose bound cannot be computed is
refused.

### Cache key

Computed after resolve and prepare, over: the canonical resolved job (use case, obligations
including requirements and their instances, brief, image ids, roles, labels, projections,
original and canonical SHA-256, prepared SHA-256 with `prep_version`); merged obligations with
versions; the rendered system prompt; tool manifests, versions and pack lock digests as frozen at
connect; schema, report, envelope versions; adapter, endpoint profile, resolved model id, API
version, catalog version; every control and budget setting; `decision_runs`; the harness version
(crate version plus git commit). Excluded only: credentials, output directory, cache settings,
timestamps. Evaluation runs never read the cache and always write immutable records. The cache
stores decisions by reference to their run directories; a hit whose artifacts have been pruned is
a miss.

### Drift

Model ids pinned; resolved model and API version on every report; `squint eval canary` reruns
a fixed set uncached on a schedule or on a provider notice, comparing against the pinned baseline
recorded when the configuration was qualified. One deviating canary marks the configuration
`suspect`; two consecutive deviations against the same baseline recommend disabling it; a canary
in which an item ended in an operational error and nothing deviated is `inconclusive`, since an
error is never a verdict. Disabling requires the owner's explicit approval, and requalification is
a full development-set run under its own spend cap. No fallback to another model or provider.

### Data leaving the machine

The harness refuses to run unless `[privacy] cloud_calls = true` and `account` names the
provider or gateway account the operator attests to, as `<prefix>:<account id>` where the prefix
is the one the profile declares (`anthropic`, `google`, `openai`). Consent is bound to the profile
id, the account, and the profile's upstream allowlist; the report records the recipient chain actually
observed (profile, base URL host, upstream provenance, resolved model). `docs/provider-data.md`
lists, per profile, what the harness enforces at request level (for example OpenAI
`store = false`, Vercel `only`), what the operator must configure on the account (retention, no
training, gateway logging and caching) and what the provider does not expose for verification.
The harness sends prepared image bytes, the prompt, the obligations, the brief and tool results;
it strips image metadata and never sends filenames or paths. Credentials come from the configured
scheme, are redacted from headers, URLs, errors, logs, journals and reports, and are never written
to disk by the harness except by the later OAuth scheme as documented. Local artifacts are
written with mode 0600 under the run directory and kept until `squint report prune` removes
them.

## Tools

### Manifest

```toml
[tool]
name = "regions"
version = "1.0.0"
purpose = "Finds regions that changed between an aligned pair, with high recall."
when_to_use = "After align succeeds on a source and candidate, before judging structure."
when_not_to_use = "Without a source image, or on a pair align could not register."
output_guidance = "Each region is a pointer, not a verdict. Inspect regions with view before deciding."
cost = { tier = "expensive", seconds = 25 }
requires = ["align"]
pairs_with = ["view", "classes"]
image_args = ["image"]                      # arguments taking image ids
frame_args = ["against"]                    # arguments taking frame ids (here: an aligned frame)
accepts_hints = ["regions"]
evidence_kind = "change_region"             # what its regions are; obligations bind to kinds, never to tools
returns = { max_images = 1, max_long_edge = 1024, max_image_bytes = 2_000_000, max_summary_bytes = 2048, max_regions = 12, max_region_label_bytes = 120, max_measurements = 8, max_transforms = 0, max_envelope_bytes = 65_536 }
frames_out = ["crop"]
concurrent = false
preconditions = "`against` must be an aligned frame of the source relative to `image`."
model_schema = "schemas/tools/regions-model-v1.json"      # what the model sees
input_schema = "schemas/tools/regions-input-v1.json"      # model_schema + required `_squint`
output_schema = "schemas/tools/regions-output-v1.json"    # envelope + this tool's measurement names and units
```

Two argument schemas exist for every tool. The **model schema** is what the provider receives as
the tool definition. The **input schema** is the model schema plus a required `_squint` object
the harness injects after validating the model's arguments; it is what a built-in or an MCP
server validates. The harness rejects, as `invalid_args` to the model, any attempt by the model
to supply `_squint` or any field outside the model schema. Preconditions are checked by the
harness against the registry before the call; a precondition failure is `invalid_args` naming the
missing frame.

Every tool result is a **result envelope** (`schemas/envelope-v1.json`) plus the tool's output
schema for `measurements` names and units:

```json
{
  "status": "ok | measured_nothing | not_applicable | invalid_args",
  "summary": "one paragraph the model reads",
  "measurements": [ { "name": "edge_similarity", "value": 0.82, "unit": "ratio", "frame": "raw:candidate" } ],
  "regions": [ { "id": "r1", "frame": "raw:candidate", "bbox": [0.1, 0.2, 0.3, 0.4], "label": "floor -> swimming pool", "score": 0.91 } ],
  "images": [ { "frame": "crop:call:3:0", "parent": "raw:candidate", "mapping": { "bbox": [0.1, 0.2, 0.3, 0.4] }, "media_type": "image/png" } ],
  "transforms": [ { "id": "t1", "from": "raw:candidate", "to": "raw:source", "kind": "homography", "params": [], "quality": { "inlier_fraction": 0.93 } } ],
  "provenance": { "tool": "regions", "version": "1.0.0", "duration_ms": 24180, "lock_digest": "…" }
}
```

`regions` become evidence items; `images` become `crop` frames; `transforms` become `aligned`
frames; `summary` and `measurements` are context. `invalid_args` is returned to the model as a
tool error and does not count as an operational failure. The harness checks every envelope
against the manifest's `returns` bounds, which cover the serialised envelope, every string field
and every image; a breach is `error: tool`.

Built-ins implement a `Tool` trait that returns this manifest and executes synchronously. MCP
tools carry `name`, `description`, `inputSchema` (the input schema, with `_squint` required)
and `outputSchema` (the envelope) natively and the rest under a `squint` key in their
annotations, including the model schema; `description` must equal `purpose`. The harness
validates manifests at connect time and refuses a tool whose model schema cannot pass the
selected model's wire transformation.

Rendering: a Tools section in the system prompt lists each enabled tool in `[tools] order`
(default cheap first, `requires` before dependants) with purpose, when to use, when not to use,
output guidance, prerequisites, cost and pairings. Model schemas go to the provider as native
tool definitions. The rendered text is deterministic and hashed into the cache key.

### Evaluation projections

A tool may declare named **projections** of its envelope for evaluation arms (manifest
`projections`). `align` declares `visual_only`: `images` and `transforms` with `params` kept,
`quality` removed, `measurements` and `regions` empty, `summary` replaced by the fixed text
"Result computed; its measurements are withheld." or "Result not available.", `status` mapped to
`ok` or `not_applicable` (`invalid_args` stays: it answers the model's own mistake). The
projection is applied by the harness before anything reaches the model or the checklist, so arms
that use it get identical visual artifacts and no diagnostic evidence or evidence items; the
report records the projection on every call that ran under one.

### Transports

- **Built-in (Rust, in-process).** `view` (crop of a flat frame by bbox, or a square perspective
  view of an equirectangular frame by yaw, pitch and fov; one image or the same region in two),
  `resize`, `edge_similarity`, `change_map` (over an aligned pair), `align` (same scene, same
  viewpoint, format intact; a homography for flat images, a rotation on the sphere for
  equirectangular ones; integrity measurements). Pure functions over canonical images and
  frames, tested with golden images for both projections.
- **MCP over stdio, revision 2025-11-25.**
  - *Startup.* Spawn the provisioned executable with an environment allowlist (`PATH`, `HOME`,
    locale, variables the pack declares in its manifest, minus any variable the harness knows as a
    credential source for any profile, and minus any name ending in `_KEY`, `_TOKEN` or
    `_SECRET`), working directory the run directory, stderr captured to the journal; `initialize`
    with the harness's capabilities and protocol version; the server must answer with exactly
    `2025-11-25` and advertise the `tools` capability, otherwise `error: resolve`; send
    `notifications/initialized`; `tools/list` with pagination until exhausted; freeze the
    manifests.
  - *Calls.* `tools/call` with the model's validated arguments plus `_squint`:
    `{ run_id, call_id, images: { <image-id>: { path | base64, media_type, width, height, projection } }, frames: { <frame-id>: { root, projection, parent, transform: { id, kind, params }, pixels: { path | base64, media_type, width, height } } }, hints }`.
    `images` holds the canonical images named by `image_args`; `frames` holds every frame named by
    `frame_args` with its full transform chain to the root and its lossless pixels, so a pack can
    use or reproduce it without any state of its own. Paths are under the run directory for
    `accepts = ["path"]` packs, base64 for `["base64"]` packs. One JSON-RPC id per call, unique for
    the process lifetime; concurrent calls only if the manifest declares `concurrent = true`,
    serial otherwise; per-call deadline `tool_timeout_s`, after which the harness sends
    `notifications/cancelled`, tombstones the id, ignores any late response for it, and treats the
    call as `error: tool`.
  - *Results.* The envelope as `structuredContent`, validated against `outputSchema`; images as
    MCP image content blocks in exactly the count, order and media types of `images`, and an
    empty `content` array when there are none. A JSON-RPC error or `isError = true` together with
    a non-error envelope status, or any mismatch between blocks and `images`, is `error: tool`;
    `isError = true` with an envelope whose status is `invalid_args` is the one recoverable form
    and is returned to the model as a tool error.
  - *Failure.* Process death, a malformed frame, or a response for an unknown id is `error: tool`.
  - *Shutdown.* Close stdin, wait 5 s, SIGTERM, wait 5 s, SIGKILL.
  - *Lifetime and leases.* One process per run by default. With `shared = true` one process serves
    a whole evaluation under an exclusive **lease**: a run acquires the lease before its first
    call, and the two runs of a decision lease separately. Lease acquisition has one bounded
    deadline, `startup_timeout_s`, covering everything it may need: waiting for the previous
    lease's calls to complete, sending `squint/reset` and receiving its acknowledgement (which
    guarantees prior-run tasks and mutable state are gone), or, when any of that fails, bounded
    shutdown and a restart with full initialisation and manifest verification. A process in which
    any call timed out or was cancelled is never reused: cancellation is a notification the
    server may not honour, so quiescence cannot be proven; the process is shut down before the
    next lease. Packs that do not implement reset are restarted per run.

### Python tool pack

`tools/python/`, a uv project with a committed `uv.lock`. First tools: `regions`
(segmentation-based change-region finder over an aligned pair; accepts `hints.regions`),
`classes` (what the segmenter sees in a region before and after). Later: `ocr`, `subject_match`,
`hands`. Each tool has its own tests and golden fixtures.

**Provisioning bundle.** Each release of the pack publishes a bundle containing a dedicated
**provisioning project**: a `pyproject.toml` with `tool.uv.package = false`, an exact dependency
`squint-tools==<version>`, and a `tool.uv.sources` entry pointing at the built wheel included in
the bundle by relative path; the `uv.lock` generated for that provisioning project; the wheel;
`MODEL.md`; and `bundle.toml` with the SHA-256 of each file and the declared Python version.
`squint tools setup --config squint.toml` downloads the bundle named by
`[tools.packs.<name>] package` and `version` (`--bundle <dir>` installs a local one), verifies
every digest, unpacks it into `$XDG_DATA_HOME/squint/tools/<package>-<version>/`, runs `uv sync --frozen --no-dev` there with
the declared Python (Python downloads allowed during setup only), then runs the pack's `fetch`
entry point to download and verify model artifacts into `$XDG_CACHE_HOME/squint/models`. The
harness launches `<dir>/.venv/bin/squint-tools serve` directly with `UV_OFFLINE=1` and
`UV_PYTHON_DOWNLOADS=never` in its environment; a missing environment, mismatched digest or
missing artifact is `error: resolve`. The lock digest is recorded in tool provenance and the
cache key. The pack's CI builds the bundle, installs it into a clean directory, and verifies the
executable runs and the installed dependency versions match the lock.

The segmenter is an ONNX export made by this project from publicly licensed upstream weights
(candidate: EoMT-L ADE20K, MIT code and weights on a DINOv2 backbone under Apache-2.0); nothing
from the private prototype is reused (owner decision, 2026-10-26). Before M5 begins,
`tools/python/MODEL.md` records the upstream source and licences, the export procedure, the
artifact URL and SHA-256, preprocessing version, onnxruntime version and execution provider, and
the parity tolerance against stored outputs. No test or run downloads anything.

## Config

```toml
config_version = 1                      # optional; the current schema version when absent

[provider]
profile = "anthropic"
model = "claude-opus-5-5"
controls = { effort = "high" }          # validated against the catalog's capabilities
max_output_tokens = 16000

[images]
max_decode_pixels = 50_000_000
max_decode_bytes = 67_108_864
max_long_edge = 1568                    # may only lower the catalog's native size

[rules]
base = "source-fidelity"
profile = "architecture"                # optional shipped profile
extra = ["./rules/mine.toml"]

[tools]
enabled = ["view", "align", "regions", "classes"]
order = ["align", "regions", "view", "classes"]

[tools.packs.python]
package = "squint-tools"              # a provisioned bundle, by package and version, or instead:
version = "0.1.0"                       # command = "./server", args = []  (an executable to run, for tests)
env = ["SQUINT_MODEL_DIR"]            # extra variables passed through, credential names refused
startup_timeout_s = 120
shared = false

[budget]
tool_turns = 4
tool_calls = 8
evidence_items = 24
repairs = 1
repair_output_tokens = 32000
retries = 3
decision_runs = 1
spend_cap_usd = 2.00
request_timeout_s = 300
tool_timeout_s = 120

[policy]
on_unsure = "pass"                      # fail | pass | review ; exit code only
on_error = "review"

[cache]
dir = "~/.cache/squint"              # caching is off until a dir is set
read = true
write = true

[privacy]
cloud_calls = true
account = "anthropic:org-xxxx"

[output]
dir = "./squint-out"

[serve]                       # squint serve only (table-manners 4.5, 10.1)
concurrency = 1               # checks at once; serial by default
session_budget_usd = 20.0     # across every call one server process serves; default ten checks at the default cap
roots = ["/srv/images"]       # image paths must sit inside these as well as the client's MCP roots; empty: the client's alone, and with neither, inline images only
```

Loading: parse, validate against the config schema, resolve the profile, check the model in the
catalog, check controls and image settings against capabilities, refuse credential names in pack
`env`, expand `${VAR}` from the environment only. The resolved config, redacted, goes into the
report.

## Evaluation

### Set manifest

An evaluation item is a complete job plus labels. The job part is canonical and runnable by
`squint check --job`; the label part is stored separately so blind sets can ship without it.

```toml
# evalsets/reference-consistency/pets-v1/set.toml
[set]
id = "reference-consistency/pets-v1"
use_case = "reference-consistency"
split = "dev"                       # dev | heldout | blind
licence = "CC-BY-4.0"
images_repo = "github.com/markhayden/squint-evalsets"
labels = "labels.toml"              # absent for blind until unsealed; labels_sha256 present instead
labels_sha256 = "…"
families = "families.toml"          # subject or scene family per item, for split separation
rules = { base = "reference-consistency", version = 1 }

[[item]]
id = "pip-bridge-02"
family = "pip"
job = "items/pip-bridge-02/job.toml"   # a full job: images by file + sha256, requirements, brief
```

```toml
# evalsets/reference-consistency/pets-v1/labels.toml
[[label]]
item = "pip-bridge-02"
expected = "fail"                   # pass | fail
failure_types = ["subject-marking-lost"]
words = "the white chest patch is gone"
obligation_ids = ["RC2"]
adjudicated_by = "owner"
adjudicated_on = "2026-11-02"

  [[label.regions]]
  frame = "raw:candidate"
  kind = "polygon"
  polygon = [[0.41, 0.52], [0.49, 0.52], [0.49, 0.63], [0.41, 0.63]]
  defect = "subject-marking-lost"
  obligation_ids = ["RC2"]
```

Rules: a family never appears in more than one split; `dev` tunes prompts and tools; `heldout`
scores confirmation gates; `blind` is labelled by the owner before any model or tool output exists
on its items, with labels held in owner storage outside both repositories from creation and the
manifest carrying only `labels_sha256` until the gate is scored, after which the labels are
published and the set becomes `heldout`. `squint sets check` verifies hashes, roles, families,
label presence per split, that public manifests and jobs carry no answer labels, and that every
item's job resolves against the pinned rules.

**Seal.** `squint sets seal` writes `seal.toml`: a SHA-256 over the sorted item ids, family
assignments, every canonical `job.toml`, every image hash, the rules content and version, and the
external label file's hash, plus a timestamp. Any change to any component invalidates the seal.
Prior exposure of the items to a judge or tool cannot be proven by code; the seal records the
owner's attestation that none occurred, and gate scoring treats journal searches as supporting
evidence only.

### Metrics

Per matrix cell, over labelled items (errors are their own column, never dropped). Where a cell
has `repetitions > 1`, repetition 1 is the **decision repetition** used for every gate and every
headline number; other repetitions are reported descriptively as per-item agreement.

- **Confusion** of expected `pass | fail` against computed `pass | fail | unsure | error`.
- **Missed failures**: expected fail with computed pass, plus computed unsure or error where the
  gate's frozen policy lets it through; count over expected fails.
- **Detected failures**: expected fail with computed fail; count over expected fails.
- **False rejects**: expected pass with computed fail; count over expected passes.
- **Unsure rate** and **error rate**: counts over all items.
- **Localisation**: for detected failures with labelled regions, a prediction is a checklist item
  dispositioned `violation` with a location (its obligations are its instances') or a violation
  finding, in root coordinates; a prediction and a label are eligible only when they share the
  root image and their `obligation_ids` intersect; a hit is an
  eligible pair with IoU above 0.3 in root coordinates, or `whole_image` against a whole-image
  label, or `absent` against an `absent` label on the same image; eligible pairs are matched
  one-to-one greedily by descending IoU; unmatched predictions are spurious; reported as hits over
  labelled regions and spurious over predictions.
- **Cost**: total charged cost over all evaluated items including errors, and the median per
  item. **Wall time**: median and total.

Binomial proportions carry a 95% Wilson interval. Cost and time carry no interval. Paired arm
comparisons report the items that flipped each way and the bootstrap interval defined under
"Gates".

### Runner and arms

`squint eval run --matrix matrix.toml --out <dir>` runs every item of the named sets through
every cell of model × arm × decision runs, with `repetitions` per cell, never reading the cache,
writing an immutable journal and report per item and a summary. Before the first decision it
writes an **execution descriptor**: every answer-affecting input resolved (matrix, set manifests,
jobs, image hashes, rules content, prompt bytes, tool manifests and lock digests, profiles,
catalog version, harness version); a resume refuses any mismatch, and one evaluation directory
accepts one writer at a time. Budget admission is per decision against the durable remaining cap;
`squint eval estimate` reports the worst-case envelope sum and warns when completion is not
guaranteed, but does not refuse. Each attempt's hold is persisted before its request is sent; after
a restart, attempts that may have been sent without a persisted settlement are charged at their
bound, settlement happens at most once, and completed errors are kept.

Arms are tool sets with identical prompts apart from the Tools section:

- **A**: no tools.
- **C**: the visual-access control: `view`, and `align` under its `visual_only` projection.
- **D_X**: C plus exactly one tool X under test; `align` stays under `visual_only` unless X is
  the alignment measurement itself.
- **D**: C plus every admitted tool, reported as a bundle; bundle results support only
  bundle-level claims.

### Gates

Admission of tool X for a use case is scored once on a sealed set with a frozen configuration
(model, prompt version, obligations version, budgets, decision runs, `on_unsure`, `on_error`,
`max_error_rate`, the tested hypothesis, and the minimum family count), recorded in the matrix
before unsealing. Defaults:

| Gate | Default |
|---|---|
| missed failures, D_X vs C | not higher |
| error rate, D_X vs C | not higher, and at most 5% |
| detected failures, D_X vs C | at least 3 more items, and the paired 95% lower bound above 0 |
| false rejects, D_X vs C | not more than 1 more item |
| unsure rate, D_X | at most 25% |
| cost, D_X vs C | total charged cost over all evaluated items at most 2× |
| independent families | at least 5 families containing an expected-fail item whose detection indicator (`computed_verdict == fail`) differs between arms |

**Paired bootstrap.** The estimand is the item-weighted paired difference in the proportion of
expected-fail items detected, D_X minus C. Resample families containing at least one
expected-fail item with replacement, 10,000 times, keeping every paired item of a selected
family; compute the estimand per resample; the interval is formed from percentiles by the
inclusive linear-interpolation quantile (Hyndman and Fan type 7) with a recorded seed. If fewer
families than the minimum have a differing detection indicator, the outcome is `inconclusive`
regardless of the interval.

**Multiplicity.** One hypothesis (one tool, one model, one set) per gate. Where a matrix declares
`m` hypotheses on one set, the count is frozen before unsealing and each interval uses the
Bonferroni-adjusted percentiles `0.025 / m` and `1 − 0.025 / m`; admission requires the adjusted
lower endpoint above zero. If the resample count cannot resolve the adjusted tail (fewer than 20
resamples below the lower percentile), the outcome is `inconclusive`.

Outcomes: `admitted` when every gate holds; `rejected` when detected failures are not higher or
missed failures or errors are higher; `inconclusive` otherwise.

**Stages.** A gate has two stages. The **initial** stage scores a sealed `blind` set of at least
20 items and yields `admitted (provisional)`, `rejected` or `inconclusive`. The **confirmation**
stage scores a distinct sealed set of at least 50 items from families disjoint with every earlier
set, under the identical frozen judging configuration, referencing the initial receipt; it yields
`admitted (confirmed)` only when both stages are admitted. `squint eval gate` verifies the seal
against the supplied labels and the results against the frozen matrix identity, refuses to score a
set twice without a new ADR reference, writes a durable **receipt** (seal hash, matrix identity,
results hash, stage, outcome, bootstrap seed and interval), and writes the label publication and
split change as files for the owner to review and commit; it never commits. An interrupted
scoring resumes from the receipt without rescoring. Admission state lives in
`evalsets/admissions.toml` (tool, use case, stage, receipt id), and arm D includes only tools whose
state is `confirmed`. A rejected tool may be scored again for the same use case only on a new sealed
blind set and only under an ADR that counts every earlier initial attempt in the matrix's
`gate.hypotheses` (ADR 0017).

## External interface

From M7 (ADR 0018). squint is one tool in a chain an agent drives: generate an image, check it
with squint, mask what squint found with rascal, paint inside the mask, check again. Every tool in
the chain presents the same external interface, defined by **table-manners**, the shared contract
of the waffle tools: a neutral repository (`table-manners`) holding the specification, the JSON
Schemas (the source of truth), test vectors, a fixture per tool and a Rust crate of serde types.
The contract has a core every tool follows and profiles a kind of tool adds; squint implements the
core and the images profile, and lists `images` in its description's `profiles`. squint pins a
tagged release, keeps a hash-checked copy of its schemas under `schemas/contract/`, and warns in CI
when a newer tag exists. squint pins v0.1.0. Field names and schemas are the contract's; this
section says what squint does with them.

- **Image reference**: `file_sha256`, width and height of the caller's file (enforced), projection
  and topology, an optional `pixel_sha256` in the contract's canonical form. squint computes the
  reference for every image it is given and verifies one the caller supplies; a mismatch is
  `invalid` with code `input`.
- **Location out**: every finding's location is mapped to the raw frame of its root image through
  the registry (as for the viewer), then back through the inverse of the image's EXIF orientation
  to the file as stored, and written in the contract's form: fractions from the top-left
  measuring edges, quarter-pixel precision with boxes snapped outward, a list of parts with
  `straddles_wrap` (a view's footprint across the seam becomes its two sides; the registry's own
  root location keeps the one side it cuts to, so scoring is unchanged), kinds `bbox`, `polygon`,
  `whole_image`, `absent` (with the finding's reason, never an invented box), with the finding's
  words as the label, its disposition and confidence, and its source (`squint`, version, run id,
  item id). Crop, view and aligned frames never leave squint.
- **`squint serve`**: an MCP server over stdio at revision `2025-11-25`, under the contract's
  server conventions. Tools: one per use case, as `describe` lists them (`source_fidelity`, …: one
  decision each; images by path or inline bytes in any slot; paths only inside the client's MCP
  roots narrowed by config; inline bytes bounded), `estimate` (worst-case cost of a check and the
  effective per-call cap; the session's budget is the `squint://session` resource; on the command
  line `squint estimate`, priced from the catalog with no credential), and `describe`. squint's own self-description (`rules`, `tools list`, `catalog`)
  stays on the command line and is referenced from the description's `x-squint`. An `invalid` or
  `error` outcome is a result with `isError` and the header; JSON-RPC errors are only for failures
  before a run exists. Each image out carries its reference and the locator the call asked for
  (path, inline, resource URI); image blocks only when asked. Checks are serial by default
  (`[serve] concurrency`); `describe`, `estimate` and resource reads never wait behind one. A
  check waiting for its turn beats like a running one, and cancelling it there leaves no run. Spend: the per-call cap from config, which a call (`max_spend_usd`; on
  the command line `--max-spend-usd`) may lower and never raise, and a per-session budget
  (`[serve] session_budget_usd`) across the server's lifetime, a ledger every decision reserves
  against before its first request. Paths: when the client offers roots, squint asks for them on
  every check and estimate and refuses (`invalid`, `input`) a path outside them, or outside
  `[serve] roots` when the config names any; a client that offers no roots, under a config that
  names none, can send images only inline (closed by default). A refusal says nothing about the
  file, not even whether it exists. A check over `serve` is an ordinary decision: its directory, journal,
  report and `run.json`, with the action's name.
- **Progress and cancellation**: `check` is a plain `tools/call`. With a progress token, squint
  sends a notification on every journal line of every run (through an observer the journal calls
  after each write) and a heartbeat after five seconds of silence, well inside the ten the
  description states; `progress` is elapsed seconds with no total, nudged so it never repeats,
  and the message carries the words (`run-1: turn_sent`, `waiting, 12 s`). From the moment the
  run is allocated every notification, heartbeats included, carries its reference in
  `_meta["table-manners/run"]` (id, resource, session id, client run id). A request may carry
  `_meta["table-manners/client_run_id"]` (`^[A-Za-z0-9._:-]{1,128}$`): claimed for the session
  when the request is accepted, written into the run's directory when it is made, and a reuse is
  `-32602` with the earlier request's run (null while it has none). `notifications/cancelled` stops the
  provider call; `run.json` and the run resource then hold status `cancelled` with spend to date,
  and no result is returned. An identical retry within the cache policy is answered from the
  decision cache and says so.
- **The session**: `squint://session` (table-manners 4.6), one atomic observation of the serving
  process's ledger: a random UUID (v4) made at start, when it started, the budget
  (`[serve] session_budget_usd`), spent with its unresolved part, reserved and remaining; every
  amount in a result or the session is written to at most six decimals.
- **Runs as resources**: `squint://runs` (the newest page of runs, newest first by start time and
  id, `running` included; `squint://runs?cursor=<next_cursor>` the next, fifty to a page, the
  cursor opaque base64url; a run started during a walk is seen by reading page one again),
  `squint://run/<id>/run.json`, `…/report.json` and `…/journal` for every decision the server ran,
  readable after a timeout or a dropped connection; every run has a journal, an empty one when it
  made no decision (an `estimate`, a call refused before its first run).
- **`run.json`**: written last and atomically (a temporary name, then a rename) into every
  decision directory by `check` on every surface, cache hits and cancelled decisions included: the
  contract's header (status, error code with `retryable` and a reason, run id, tool and version,
  timestamps and duration, input identity, spend (with `unresolved_usd`: a provider call that
  never left costs nothing, an authentication rejection costs nothing, and one that failed with no
  usage reported, or was cancelled in flight, counts at its bound, unresolved; a successful answer
  that reports no usage counts its bound too, settled, and makes the basis `estimated` rather
  than `measured`), cache, the images taken in with their
  slots and roles, every finding's location, notes for image warnings and repairs) and squint's
  body (verdict, exit code, whether two runs agreed, the report's path). The result's `action` is
  the use case's as `describe` names it (`source_fidelity`), from the command line as from MCP. A
  failed image is `completed` with verdict `fail`. squint's error classes map to the contract in one table
  (`contract::status_of`): `resolve` and `auth` are `invalid` with code `usage`; `input` and
  `image_limit` are `invalid` with code `input`; a provider or model-output failure is a
  retryable `error` with code `provider`; `tool` is `tool_process`; `budget`, `io` and anything
  else (`internal`) are not retryable; `cancelled` is its own status.
- **`squint describe --json --config <file>`**: the contract's description document with
  `profiles: ["images"]`: one action per use case the config can check (`source_fidelity`, …:
  the one `[rules]` names, its slots from the merged rules with relative files resolved against
  the working directory as `check` resolves them; else every shipped use case), named for
  its rule set's `use_case` with a one-sentence purpose (kept beside the description, since sealed
  sets bind the rule files byte for byte), taking `candidate`, the `source` when the
  rule set requires one, and `references` (a list; contract v0.1 has no list slot, so squint says
  so under `x-squint`), with the brief, requirements and a lower spend cap as parameters; an
  `estimate` action; every disposition and confidence word a finding can carry (`vocabulary`),
  the renders (`report view`, `report export`), the cost notice and the credential variable the
  configured profile needs (its name, never its value; a check on the scripted fake, under the
  `fake` profile or `--provider-script`, declares `may_spend: false` and no credential), the
  expected duration, the inline limit (the decode bound) and the resources. Every
  `params_schema` and MCP `inputSchema` is a schema of its own: the definitions it refers to sit
  at its root. Argument errors on every command exit 64.
- **Exit codes** across commands: 0 success, 3 error, 64 usage or input, 130 on SIGINT and 143
  on SIGTERM;
  1 fail and 2 unsure remain `check`'s; 69, 70 and 74 are `playground`'s (below).
- **`squint playground`** hands off to Wafflehouse, the shared local program, by the launcher
  agreement (wafflehouse H1): it runs `wafflehouse --tool squint` and nothing else, waits, and
  exits with the launcher's code (128 + the signal when the launcher is killed by one). It takes
  no arguments. The program is found at `WAFFLEHOUSE_BIN` (an absolute path; anything else is an
  error, never a fall-through), else `wafflehouse` on `PATH`, else `~/.local/bin/wafflehouse`;
  each must be an executable regular file once symlinks are followed. When none is found:
  on a terminal (stdin and stderr both) squint explains what Wafflehouse is and offers to install
  it into `~/.local/bin`; no (or end of input) changes nothing and exits 0. No installer is
  published yet, so release builds say so and exit 69 instead of offering; an install failure
  exits 74 and starts nothing. Off a terminal it prints the instructions and exits 69. squint
  never reads the host's state, owns its locks or opens a browser: the launcher does. Which runs
  the host shows comes from tool registration, a separate agreement; squint carries no UI.
- **Later** (its own milestone): a `mask` location in, from rascal, as the region where change was
  intended, which needs `allowed` obligations that take a region; the contract's cross-tool
  acceptance test is its definition of done.

## Tech stack

- Rust stable (edition 2024), `tokio`, `reqwest` with `rustls`, `serde`, `serde_json`, `toml`,
  `schemars`, `jsonschema`, `image`, `clap`, `sha2`, `thiserror`, `tracing`. Dependencies beyond
  this list need an ADR.
- Rust MCP: the pack client is in-house JSON-RPC (ADR 0015); `squint serve` uses `rmcp`
  pinned `=3.8.0` as `rascal serve` does (ADR 0018), and M7 decides whether the client follows.
- Python 3.12, `uv`, `pytest`, `ruff`, the MCP Python SDK, `onnxruntime`, `numpy`, `pillow`.
- Static HTML and vanilla JavaScript for the viewer.
- GitHub Actions for CI; `cargo-dist` or a hand-written release workflow for binaries.

## Commands

```
# Rust
cargo build --workspace
cargo nextest run --workspace --features squint-cli/testkit   # unit, fixture replay, golden images; no network
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
SQUINT_LIVE=1 cargo test -p squint-providers --test record -- --ignored   # records fixtures, spends money

# Python tool pack (development)
cd tools/python && uv sync --frozen            # the one step that may download
uv run --frozen --offline --no-sync squint-tools fetch   # downloads model artifacts only, explicit
uv run --frozen --offline --no-sync pytest     # offline; skips segmenter tests if not fetched
uv run --frozen --offline --no-sync pytest -m segmenter
uv run --frozen --offline --no-sync ruff check . && uv run --frozen --offline --no-sync ruff format --check .

# The binary
squint tools setup --config squint.toml     # provisions bundles and artifacts (network allowed here)
squint check --config squint.toml --candidate out.png --ref "pip=refs/pip.jpg" --brief "…"
squint check --config squint.toml --job job.toml
squint report view ./squint-out/<run-id>
squint report rebuild ./squint-out/<run-id>
squint report prune --older-than 30d
squint tools list --config squint.toml      # manifests as the model will see them
squint describe --json --config squint.toml # the contract's description document (ADR 0018)
squint estimate --config squint.toml --job job.toml [--max-spend-usd 0.10]   # worst case of one check; nothing sent
squint check --config squint.toml --job job.toml --max-spend-usd 0.10        # lowers the cap for this check, never raises it
squint serve --config squint.toml [--out runs/]   # MCP over stdio, protocol 2025-11-25 (table-manners 5)
squint sets check evalsets/
squint sets fetch --set reference-consistency/pets-v1
squint sets seal --set <set> --attest "…"    # writes seal.toml; a blind set's labels move out (--labels-out)
squint sets verify-seal --set <set>
squint eval estimate --matrix matrix.toml      # every envelope, the sum, a cost range; warns over the cap
squint eval run --matrix matrix.toml --out eval-out/<name>
squint eval report --out eval-out/<name>       # regenerate metrics.json and report.md from saved results
squint eval report eval-out/<name>
squint eval gate --matrix matrix.toml --out eval-out/<name> --tool regions --set <sealed set> --stage initial --labels <owner labels>
squint eval gate-table --fixture fixtures/stats/case-1.toml   # the gate table on a synthetic result table
squint eval canary --config squint.toml --set <pinned set> --state canary.toml      # clear | suspect | recommend_disable; never disables
squint eval requalify --config squint.toml --set <dev set> --state canary.toml --approve "…"   # records the new baseline
squint check --config squint.toml --job job.toml --replay fixtures/adapters/anthropic-messages/first-look
squint check --config squint.toml --job job.toml --record recordings/first-look
squint catalog show claude-opus-5-5 [--json]
squint catalog list [--json]
```

Exit codes for `check`: 0 pass, 1 fail, 2 unsure, 3 error, after `[policy]` is applied; 64 for a
usage error or an `invalid` request (a job that does not resolve, an image that is not the one
named) whatever the policy; 130 on SIGINT and 143 on SIGTERM.

## Project structure

```
Cargo.toml                      workspace
dist-workspace.toml             release archives: targets, the archive smoke as the publish gate (`dist` regenerates the release workflow from it)
.github/workflows/              ci (just check), package (publish dry run, install from the packaged crates, release archive dry run), release (generated), archive-smoke (called by release)
examples/                       one ready-to-run config per use case
crates/
  squint-core/                config, obligations, instances, checklist, prompt, state machine, repair, schema, aggregation, decision, journal, report, cache, ledger, frames
    rules/                      base rule sets and profiles/, embedded at build time
    prompts/                    versioned system prompt templates, embedded
  squint-providers/           Provider trait, adapters, wire schemas, endpoint profiles, catalog, auth schemes, fixtures player
    catalog.toml                model catalog and the shipped endpoint profiles, embedded
    profiles/*.toml             gateway profiles with upstream contracts, embedded (2.8)
  squint-tools/               Tool trait, built-ins, MCP client, manifest registry, envelope, projections, provisioning
  squint-eval/                set manifests, hashing, sealing, matrix runner, metrics, bootstrap, gates
  squint-cli/                 the binary
tools/python/                   squint-tools uv project (MCP server, segmenter, MODEL.md, uv.lock, bundle build, tests)
scripts/                        repository checks, the credential scanner, publish, install-from-registry, archive and live smoke scripts
scripts/sets/                   the set-building toolkit (uv project): plans, generators, construction ops, draft labels
schemas/                        generated JSON Schemas: submission, envelope, manifest, config, report, tools/;
                                contract/: the pinned table-manners schemas, hash-checked (SOURCES.toml)
fixtures/
  matrix/                       matrices the evaluation tests use
  adapters/<adapter>/           recorded conversations per capability combination
  profiles/<profile>/           recorded conversations per shipped profile, including routing
  images/                       golden images for built-in tools, flat and equirectangular
evalsets/<use-case>/<set>/      set.toml, labels.toml or labels_sha256, families.toml, seal.toml, items/*/job.toml
crates/squint-cli/viewer/     static report viewer, embedded in the binary (packaging keeps embedded assets inside their crate)
docs/
  brief.md                      untracked, the starting brief
  adr/                          one file per decision, dated
  reviews/                      verbatim external reviews
  provider-data.md              enforced, attested and unverifiable per profile
  architecture.md               the diagram above, kept current
  tools.md  rules.md  evalsets.md  evaluation.md  viewer.md
SPEC.md  PLAN.md  README.md  LICENSE
```

## Code style

Plain structs and enums, boxed trait objects for adapters and tools (traits kept dyn-compatible,
futures boxed), no clever generics, errors as one `thiserror` enum per crate with context, no
`unwrap` outside tests. Every type that crosses a boundary (config, obligations, manifest,
envelope, submission, report) derives `Serialize`, `Deserialize` and `JsonSchema`, uses
`deny_unknown_fields`, and carries a version field.

```rust
/// One aggregation row for one mandatory instance. The caller evaluates every instance an item
/// or finding depends on and keeps the strictest row; the model's text never reaches this.
pub fn row(inst: &Instance, d: &Disposition, c: Confidence) -> Row {
    use {Confidence::*, Disposition::*};
    match (d, c) {
        (Violation, Confident) => Row::Fail,
        (Violation, Tentative) => Row::Unsure,
        (CantTell, _) if inst.matters => Row::Unsure,
        (Hidden, _) if inst.matters && !inst.hidden_ok => Row::Unsure,
        (Held, Tentative) if inst.matters => Row::Unsure,
        (AllowedChange { .. }, Tentative) if inst.matters => Row::Unsure,
        _ => Row::Recorded,
    }
}

pub fn strictest(rows: impl IntoIterator<Item = Row>) -> Row {
    rows.into_iter().max().unwrap_or(Row::Recorded)   // Recorded < Unsure < Fail
}

pub fn verdict(rows: &[Row]) -> Verdict {
    match rows.iter().max() {
        Some(Row::Fail) => Verdict::Fail,
        Some(Row::Unsure) => Verdict::Unsure,
        _ => Verdict::Pass,
    }
}
```

Python: `ruff` defaults at line length 100, type hints everywhere, one module per tool, pure
functions over arrays with the MCP wrapper kept thin.

Naming: crates `squint-*`, modules by noun, tool names lower snake case and stable once
published, obligation ids as above, instance ids `<obligation>@<target>`, checklist ids
`chk:<kind>:<ref>`, call ids `call:<n>`, frame ids as above.

## Testing strategy

- **Unit** (`cargo test`, every commit): config loading and capability validation; obligation
  merge, `extends`, `waives`, `condition`, scope and evidence resolution to instances including
  the empty-expansion errors, evidence-kind binding, and rendering; checklist construction, the
  non-empty-dependency invariant, growth, the bound, drops, ordering and overlap cross-references;
  prompt assembly byte-stable for a fixed input; canonical schema, wire transformations
  round-tripping for every adapter, semantic validation with every repair path; the aggregation
  table exhaustively per instance and the strictest-row combination; the decision table and
  evidence matching exhaustively; cache key changes for each input in the key list and for
  nothing outside it; ledger arithmetic including transfers, settlement, unknown usage, reissue
  allowance and release; frame mapping round trips for flat and equirectangular including
  clipping and seam rules; redaction in every output path; environment allowlist filtering.
- **Adapter conformance** (`cargo test`): fixtures are recorded conversations, one per
  capability combination the catalog declares for each adapter, covering first look, a parallel
  batch with two same-name calls, a tool error, a multimodal tool result, each repair case in
  both forms (invalid submission, mixed batch, truncated arguments, refusal, call-less text), the
  transition into the final turn under each supported mechanism with a stray call during it, and
  the largest permitted image conversation. Replay compares canonical request JSON after removing
  only transport authentication fields declared volatile per adapter (auth headers, request ids,
  timestamps, request signatures); thought signatures, signed parts and opaque reasoning payloads
  are byte-exact comparison targets and a replay fails when any is dropped, moved or altered.
  Parsing must yield identical `TurnResponse`s and `ResponseClass`es. Recording is an ignored
  test behind `SQUINT_LIVE=1` that strips secrets.
- **Profile conformance** (`cargo test`): one recorded conversation per shipped profile,
  including the routing body and provenance response for gateways.
- **Built-in tools** (`cargo test`): golden images; crops and perspective views match stored
  outputs within tolerance; `align` recovers known transforms; envelopes validate and respect
  bounds; the `visual_only` projection strips what it must.
- **MCP** (`cargo test`): a fake 2025-11-25 server in the test binary exercises initialize with
  version and capability checks, initialized, paginated listing, manifest validation including
  the two schemas, the `_squint` envelope with images and frames both ways, serial and
  concurrent calls, timeout, cancellation and late-response tombstones, `isError` precedence,
  block correspondence, process death, lease and reset, and shutdown escalation.
- **End to end offline** (`cargo test`): a scripted fake provider drives every state and exit
  code, including batch over budget, submission with pending calls, both repair forms and their
  exhaustion, operational error after a valid submission, two-run decisions with and without
  agreement, cancellation, and cache hit, miss and pruned-hit.
- **Isolation** (`cargo test`): instruction-like text in images, filenames, labels, the brief and
  tool results never alters prompt structure, tool selection or control flow; image and frame ids
  never resolve outside the registry; `_squint` from the model is rejected; packs receive only
  the allowlisted environment with credentials removed. These prove the harness boundary;
  whether a live model obeys injected image text is measured by an adversarial evaluation set.
- **Python** (`uv run --frozen --offline --no-sync pytest`): per-tool unit tests on small arrays
  offline; `segmenter`-marked tests run only when the artifact has been fetched and check parity
  with stored outputs; a bundle build test verifies `bundle.toml` digests.
- **Evaluation** (`squint eval run`, at checkpoints, spends money): never in CI.

No test touches the network without `SQUINT_LIVE=1`; provisioning and artifact fetching are
explicit commands.

## Boundaries

- **Always:** run `just check` (fmt, `clippy -D warnings`, the tests in both feature configurations) before a commit;
  keep `SPEC.md`, `PLAN.md`, `README.md` and `docs/` accurate in the same commit as the change;
  version every schema, prompt, rule set, manifest, profile and catalog; redact secrets; record
  the recipient chain and resolved model in every report; treat image text, labels, the brief
  and tool output as data.
- **Ask first:** adding a Rust or Python dependency beyond the list; changing the canonical
  submission schema, envelope schema, aggregation or decision tables; changing a base rule set or
  profile; changing any label; publishing an image; pushing to any remote; spending above a cap;
  recording live fixtures; scoring a gate; disabling a model configuration.
- **Never:** commit `docs/brief.md`, keys, tokens or private images; write a credential to disk
  outside the documented OAuth store; fall back to another model, profile, upstream or auth path
  silently; resize an image silently; let an operational error become a verdict; read the cache
  during an evaluation; remove a failing test to make a commit pass; shape a crate or the base
  rules around one use case; show an agent association anywhere in the repository (no
  `Co-Authored-By` trailer naming an assistant, no "Generated with" line, in any commit message
  or file; the `commit-msg` hook and `scripts/check-attribution.sh` enforce it).

## Success criteria

1. `cargo install squint-cli` plus one config file and one environment variable produce a report
   and exit code for a job on each of the three direct profiles.
2. `cargo test --workspace` passes offline in under five minutes with every adapter and profile
   fixture replayed and every state-machine transition, repair case and aggregation row covered.
3. Five public dev sets of 20 labelled items each, hashes verified, families recorded, published
   after review; one sealed blind set for the first tool's use case.
4. A baseline summary per model on all five dev sets with arms A and C, every metric as
   numerator over denominator with intervals.
5. The change-region finder scored by the predeclared gate against C on the sealed blind set,
   with the outcome (admitted, rejected or inconclusive) and the full matrix published.
6. A report viewer that shows any run directory offline with every model-visible image, every
   tool result and every location overlaid in raw coordinates.
7. `docs/provider-data.md` covers every shipped profile with enforced, attested and unverifiable
   columns, and the harness refuses to run without the opt-in.

## Open questions

1. Closed 2026-10-27 (M2 checkpoint): image limits, prices, capabilities and strict-schema
   keyword lists per model were read from the providers' documentation on the catalog's
   `verified` dates and re-verified by the recordings; what the documentation did not state is
   listed in docs/provider-data.md with the recording that confirmed or corrected it.
2. Closed 2026-10-28 (ADR 0015): the pack client stays in-house; the server uses `rmcp` (ADR 0018).
3. Which image models generate the public sets and under which licence; decided with the first
   set in M3 and recorded per item.
4. Whether Vercel's response carries upstream provenance the harness can read, and how; verified
   when the profile's fixtures are recorded, and the profile stays unshipped until then.
