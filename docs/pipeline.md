# squint and rascal in a production pipeline

Status: draft for discussion, 2026-11-05. Builds on ADR 0018 (table-manners) and does not
change it. Where this page and `SPEC.md` disagree, `SPEC.md` wins until an ADR records the change.

## Why this page

squint and rascal are designed as one chain (ADR 0018): an image is generated, squint checks
it, rascal masks what squint found, a filler paints inside the mask, and squint checks the
result again. The contract fixes what crosses between the tools. It says nothing about where
the images live, who runs the tools, how a failed step is retried or who pays. This page sketches
that layer as it would look in a real deployment, names what is missing today, and draws the
consequences for squint's own roadmap, `squint tools call` first.

## Shape

```
          ┌────────────── orchestrator (Step Functions, Temporal or a queue) ───────────────┐
          │  job manifest: one document per job; every step's inputs, outputs, spend, verdict │
          └─────────────────────────────────────────────────────────────────────────────────┘
               │                 │                  │                  │
         ┌─────▼─────┐     ┌─────▼─────┐      ┌─────▼─────┐      ┌─────▼─────┐
generate │  squint   │fail │  rascal   │      │  filler   │      │  squint   │ pass   → publish
───────▶ │  check    ├────▶│  mask     ├─────▶│  paint    ├─────▶│  check    │ unsure → review queue
         └───────────┘     └───────────┘      └───────────┘      └───────────┘ fail   → retry ≤ N
               ▲ workers: fetch inputs to local scratch · run the CLI · upload outputs │
               └───────────────────── object store (S3), content-addressed ──────────┘
                         blobs/sha256/<hash>          jobs/<job-id>/<step>/…
```

## Decisions

### The tools never talk to the store

A worker wraps each tool. It fetches the step's inputs by digest into a local scratch
directory, runs the tool's CLI (`squint check`, `squint call`, `rascal heal`, `rascal call`),
uploads what the tool wrote, and records the step in the job manifest. The tools see local
files only.

- The contract leaves images by URL out on purpose, and `squint serve` refuses any path outside
  `--allow-dir`. A tool that fetched from S3 would need credentials, retry policy and a client
  for each store; a worker does that once for every tool.
- Store credentials stay in the worker. squint reads provider keys from the environment only,
  and the Python pack's environment already drops anything named like a credential.
- The container image carries the binaries, the Python pack and the segmenter artifact, with
  digests verified at build time (`squint tools setup`). Nothing downloads at run time.

### The store is content-addressed

Every blob lives at `blobs/sha256/<file_sha256>` and is written once. The contract's image
reference already names a file by `file_sha256` and a receiver refuses a mismatch, so the key the
store uses is the key every tool checks. A job's own outputs (`report.json`, the decision
directory, mask bundles, the exported report page) live under `jobs/<job-id>/<step>/`, and the
manifest points at them.

### State lives in the job, not in the tools

- **squint** keeps nothing between checks; each check owns its run directory and its frames
  ("squint stays stateless per check", ADR 0018). Any worker can take any check.
- **rascal**'s workspace lives for one step, in scratch. A mask leaves it as a bundle
  (`rascal export`, the contract's `export`), which goes to the store; the next step imports it.
- **Between tools** only the contract's types cross: an image reference and locations, always
  on the original image, never on a crop, view or aligned frame. That is what lets a squint
  finding drive a rascal step without glue.

### Retries and caching

The deterministic tools (`align`, `regions`, the rascal ops) are safe to cache on (input
digests, tool version, arguments, config digest). A verdict is not: the model may answer
differently, and the pipeline has to choose between a stable answer and a fresh look. squint's
cache policy covers its own side; the orchestrator states which it wants per step and records
the cache hit in the manifest.

### Where an unsure verdict goes

To a review queue with the exported report attached (`squint report export`: one
self-contained page with every image, tool call and finding). A reviewer's decision goes back as
a labelled item for the evaluation sets, so production traffic feeds the gates.

## What is missing today

| Gap | Effect | Where it is decided |
|---|---|---|
| The reverse handoff: rascal's mask into squint as the region where change was intended | The second check reports the repair itself as a violation, so the loop cannot close | ADR 0018, its own milestone |
| A spend limit across processes | Each worker keeps its own per-call cap and session budget; nothing bounds a day's spend across fifty workers | table-manners: the per-period budget is later work; until then the orchestrator keeps the ledger |
| Single tool calls from the CLI | A tool cannot be tested, debugged or used as a cheap screen without a model run | This page, below |
| Tools that see outlines and bands | `regions` finds a reshaped corner but not a moved coping band or a widened outline as one finding | This page, below |
| Live recordings with the non-strict tools (branch `fix/strict-submit-only`) | The Anthropic replay test fails until re-recorded | That branch |

## Consequences for squint

### `squint tools call` is stateless

The pipeline view settles the design question. Production code never passes an aligned frame
between processes; the contract keeps frames inside a tool and puts locations on the original
image. So:

- `squint tools call <tool> --source a.png --candidate b.png [--args '{…}'] --out <dir>` runs
  one tool and the prerequisites it declares (`requires = ["align"]` runs `align` first, in the
  same call), and writes the envelope, its images and its regions mapped onto the original
  image into `<dir>`. Exit codes are the contract's.
- No workspace. A debugging option (`--frame <file>`) may take a frame file an earlier call
  wrote, to chain tools by hand; production callers never need it.
- A content-addressed cache for the expensive prerequisites (`--cache <dir>`) is later work and
  only ever an optimisation.

This differs from `rascal tools call`, which needs a workspace. That suits rascal, whose masks
are artifacts worth keeping; squint's outputs are evidence about one comparison.

### The same call is a pipeline step

A stateless `tools call` is also a cheap screen: run `regions` on every generated image and
only pay for a model check when it reports change in a protected class.

### New tools come from the failures

The pool pair of 2026-11-05 (a rectangular pool whose right end was redrawn rounded, the pool
widened and the tile band moved) shows the gaps, tool by tool, when every tool is called on it
with no model:

| Change | Found? | By |
|---|---|---|
| Rounded right end | yes | `regions` r1 "swimming pool → floor", score 1.00 |
| Pool wider | partly | `regions` r2 and r3 "floor → swimming pool", small and scattered |
| Tile band moved and narrower | no | nothing; `change_map` is dominated by ripples at 768 px |

Two tools follow: an **outline** comparison (segment the protected class in both images,
report the overlap, the largest boundary shift and where it is, and a difference image) and a
**band** comparison (strong linear features such as coping and tile lines, matched across the
pair, with the shift and the change in width). Both report on the original image.

## Order

1. Stateless `squint tools call`; use it to prototype the outline and band tools on the pool pair.
2. The pool pair, its resort sibling and a few staging-only passes as a source-fidelity
   set under the architecture profile; the arms (no tools, view and align, everything) on Haiku
   and Sonnet.
3. The reverse handoff, which closes the loop.
4. A reference worker and manifest: one container that fetches, runs and uploads, and the
   cross-tool acceptance test (check, mask, check) on the pool pair.
5. A durable spend ledger across workers and scheduled canaries (`squint eval canary`).
