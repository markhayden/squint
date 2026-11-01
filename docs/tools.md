# Tools

Tools supply evidence; the model decides what it means; code computes the verdict. This page
describes the tool contract as built so far. The full design is SPEC.md "Tools".

## Manifest

Every tool describes itself once, in a manifest (`schemas/manifest-v1.json`): name, version,
purpose and guidance text, cost tier, dependencies, which arguments take image ids and frame ids,
the evidence kind its regions carry, the bounds its results stay within (`returns`), the
evaluation projections it supports (`projections`), and three schemas: the model schema (what the provider receives), the input schema (the model schema plus
the required `_squint` object the harness injects), and the output schema. `manifest_version`
defaults to the current version and is validated. `fixtures/tools/regions.toml` is the example.

## Envelope

Every result is an envelope (`schemas/envelope-v1.json`): status, a one-paragraph summary,
measurements, regions, images, transforms and provenance. The model reads a presentation of it
(summary, measurements, regions with their checklist ids, the images' frame lines) and receives
the images as images; the envelope planner prices that presentation (`Bounds::presented_bytes`,
from the manifest's text bounds) plus the images, not the stored envelope with its inline pixels. Regions become checklist evidence items
by evidence kind; images become crop frames; `invalid_args` is answered to the model as a tool
error; anything else out of bounds is `error: tool`.

## Evaluation projections

An evaluation arm may run a tool under a named projection of its envelope (SPEC "Evaluation
projections"): `ToolRegistry::with_projections` wraps the named tools so every envelope is
projected before the run, the checklist or the model sees it. The only projection so far is
`visual_only`, the control arms' `align`: images and transforms (with their parameters) kept,
`quality`, measurements and regions withheld, a fixed summary, `status` mapped to `ok` or
`not_applicable`. A tool must declare a projection in its manifest to run under it; the report's
tool call entry names the projection and its stored envelope is the projected one. The runner
takes the projections from the arm (`align:visual_only` in the descriptor's selections).

## Context injection

After validating the model's arguments against the model schema (no `_squint`, no unknown
keys, required keys present), the harness resolves image and frame arguments through its frame
registry and injects `_squint`: run and call ids, the named images with their lossless pixels by
path under the run directory, the named frames with their transform chains, and the rules' hints
for the tool. An id that is not in the registry, such as a path, is an unknown id and never
touches the filesystem.

## Transports

**Built-in (in-process Rust).** `Tool` implementations in `crates/squint-tools/src/builtin/`,
manifests embedded from `crates/squint-tools/manifests/`, listed by `squint tools list`. A pack
tool of the same name takes the built-in's place (packs are registered first). Shipped so far:

- `view` (`frame`, optional `also`; either `bbox` or `yaw`, `pitch` and `fov`): a crop of a frame
  by normalised box, snapped outward to whole pixels (the mapping records the pixels actually cut)
  and scaled to a 1024 px long edge when larger; or a square perspective view into an
  equirectangular frame, sized to the panorama's own resolution over the field of view between
  256 and 1024 px, rendered by inverse-mapping every pixel to the sphere and sampling the panorama
  across the seam (`squint_core::frames::sphere`). `also` repeats the same region in a second
  frame of the same projection. Boxes under 8 px, angles on a flat frame, both forms at once, or
  `fov` outside 10 to 120 are `invalid_args`. Returned frames are `crop:<call>:<n>` with the
  mapping the registry maps locations through: a box on a view frame becomes the polygon of its
  footprint on the panorama, cut at the seam and flagged when it straddles it, a full-width box
  when it holds a pole.
- `resize` (`frame`, `long_edge` 64 to 1024): the frame scaled down with a triangle filter,
  mapping the whole parent; it never enlarges.
- `align` (`image`, `against`): registers one image onto another of the same projection with the
  estimators of ADR 0014 (a homography for flat pairs, a rotation for panoramas). A registered
  pair returns the transform `t1` with its quality, the measurements (`ncc`, `inlier_fraction`,
  `overlap`, `texture`, and `translation_px`, `rotation_deg`, `scale` or `yaw_deg`, `pitch_deg`,
  `roll_deg`) and the aligned frame `aligned:<image>:t1`: the image warped losslessly into
  `against`'s grid, capped at 1024 px, whose coordinates are `against`'s and whose pixels are the
  image's. A location on it maps back to the image's raw frame through the inverse transform
  (`frames::aligned`): a box becomes the polygon of its corners under a homography, its footprint
  under a rotation. A pair that does not register is `not_applicable` with one
  `alignment_failure` region over the image, the evidence SF2 binds to; a textureless pair is
  `not_applicable` with no region. It declares the `visual_only` projection the control arms run
  it under.
- `edge_similarity` (`frame`, `against`, two frames in one grid such as `raw:source` and
  `aligned:candidate:t1`): the normalised cross-correlation of the two frames' edge-strength maps
  (gradient magnitude of the blurred luminance at a working size of 768 px), 1 for the same
  structure, 0 when either frame has no edges; with the weaker frame's mean edge energy. It
  survives a tone change and drops for a different scene. Requires `align`.
- `change_map` (`frame`, `against`): a grey map in `frame`'s grid of the absolute difference of
  the two frames' locally contrast-normalised luminances, saturating at three units, returned as
  `crop:<call>:0` over the whole frame with `changed_fraction`, the share of pixels differing by
  more than one unit. Bright where structure changed, dark under a uniform tone or colour change.
  Requires `align`.

The alignment estimators behind `align` are in place (`squint_tools::align`, ADR 0014): `align_flat` returns
the homography from source to candidate pixel coordinates and `align_sphere` the rotation between
two equirectangular panoramas, each with `overlap`, `ncc`, `inlier_fraction` and `texture` and
an outcome of `registered`, `low_texture` or `not_registered`. Direct registration on locally
contrast-normalised pyramids: an exhaustive coarse search, then robust Gauss-Newton per level;
measured within 0.05 px and 0.01° at 2048 px on synthetic scenes with exact ground truth, and
refusing unrelated, textureless, mirrored, quarter-turned and barely overlapping pairs
(`fixtures/images/align/`).

**MCP over stdio, revision 2025-11-25** (`crates/squint-tools/src/mcp/`). The client is
synchronous like the `Tool` trait. On connect it spawns the server with the environment allowlist
(`PATH`, `HOME`, the locale, the pack's declared variables minus every credential name), sends
`initialize` and requires exactly `2025-11-25` and the `tools` capability (otherwise
`error: resolve`), sends `notifications/initialized`, lists tools following `nextCursor`, and
freezes the manifests: each tool's `name`, `inputSchema` and `outputSchema` from the native
fields, the rest from a `squint` object in its annotations, with `description` equal to
`purpose`.

A pack serves one run at a time under a lease. A run acquires the lease before its first call,
within `startup_timeout_s`, one deadline for everything: `squint/reset` and its acknowledgement
(after which no prior-run state survives), or, when the server answers method-not-found, hangs, or
is tainted, shutdown and a restart whose tool list must equal the frozen one. A call runs under
`budget.tool_timeout_s`; past it the client sends `notifications/cancelled`, tombstones the id so
a late answer is ignored, and marks the process unreusable. Shutdown closes stdin, waits, sends
SIGTERM, waits, then SIGKILL. `structuredContent` is the envelope, validated against the tool's
`outputSchema`; the content blocks must be exactly the image blocks of `images`, in order and
media type, and nothing else; `isError` with an `invalid_args` envelope is the one recoverable
form (`the_is_error_truth_table_is_exhaustive` enumerates the rest).

The client is multiplexed (ADR 0015): a reader thread routes each response to the request waiting
for its id, so tools whose manifests declare `concurrent = true` run together on one process while
the others take turns; a stray id, a malformed frame or the end of the stream fails every pending
request and taints the process. `tools/list` follows `nextCursor` to the end and refuses a
repeated cursor or more than 64 pages. A new run's lease waits, within its one deadline, for the
previous run's in-flight calls before resetting or restarting. The context's `frames` carry each
named frame's `chain` down to its raw root, so a pack can reproduce any location with no state of
its own. In the run, a batch of calls whose tools all declare `concurrent = true` is dispatched
together and settled in call order, so evidence items keep the batch's order whatever finishes
first.

Configured per pack as `[tools.packs.<name>] command = "..."` (plus `args`, `env`,
`startup_timeout_s`) for an executable, or `package` and `version` for a provisioned bundle.
With `shared = true` one process serves a whole evaluation: the binary connects the pack once and
every decision's runs take leases on it, keyed by run directory and run number so one decision's
`run-1` is not another's; a new lease waits for the previous run's calls, then resets or restarts.
The failure modes have a test each against the fake server: a server dying mid-call, a malformed
frame, a response for an unknown id, a late answer after cancellation, reset failure, shutdown
escalation, and two runs contending for one pack.

## Writing a tool, with `view` as the example

A built-in is a Rust type implementing `squint_core::tool::Tool` in
`crates/squint-tools/src/builtin/`, registered in `builtin::all()`. `view`
(`builtin/view.rs`, manifest `manifests/view.toml`) shows every part:

1. **The manifest** is a TOML file embedded with `include_str!` and parsed once in `new()`. It
   names the tool, says in plain words what it does, when to use it and when not to, and what its
   output means; declares the cost tier; lists which arguments take image ids (`image_args`) and
   frame ids (`frame_args`), so the harness resolves them and injects the pixels (an image
   argument also takes the image's raw frame id, `raw:candidate` for `candidate`, since that is
   how the prompt names the picture, and a frame argument takes a bare image id as its raw frame;
   each such argument's description names both forms, an image id and a `raw:` frame id, which
   `squint_testkit::tool::image_argument_gaps` checks over every built-in and pack manifest);
   bounds what the
   tool may return (`returns`); names the frames it makes (`frames_out`) and the projections it
   supports (`projections`); and gives the model schema (what the provider sees, closed, with a
   description per argument), the input schema (the model schema plus `_squint`) and the output
   schema (the measurement names and units it may report). `view` takes `frame`, an optional
   `also`, and either `bbox` or `yaw`, `pitch` and `fov`.
2. **Arguments** arrive validated against the model schema's required keys and closed shape; the
   tool still checks types and ranges itself, and answers a mistake with an `invalid_args`
   envelope (`super::invalid_args`), which the harness returns to the model as a tool error and
   never counts as an operational failure. `view` refuses both forms at once, angles on a flat
   frame, a box under eight pixels and `fov` outside 10 to 120 this way.
3. **Pixels** come through the context: `ctx.frames[id]` is a `FrameHandle` with the frame's
   projection, parent, mapping, chain to its root and a lossless PNG by path under the run
   directory (`raster::load` reads it); `ctx.images[id]` likewise for image arguments. Never a
   path from the model.
4. **Results** go back as an `Envelope`: a summary the model reads, measurements (named in the
   output schema), regions (which become checklist evidence of the manifest's `evidence_kind`),
   images and transforms. A returned image is a new frame: `crop:<call id>:<n>` with its parent
   and the mapping back into it (`raster::frame` encodes the PNG and names it), or
   `aligned:<image>:<transform>` after a transform in the same envelope. The harness checks every
   bound in `returns`, stores the envelope, registers the frames, shows the model the images and
   maps any location drawn in them back to the root image.
5. **Tests** live in `crates/squint-tools/tests/`: the tool called directly with a context
   built from synthetic images (`squint_testkit::images`), its outputs checked against what the
   maths says (a crop equals the region it cuts; a perspective view matches the analytic view of
   the same scene and a stored golden), its refusals checked one by one, and one run through
   `run::check` with a scripted provider showing the frame in the report and a finding on it
   mapped back to the root.

An MCP tool is the same contract over stdio: the manifest's fields travel in the tool listing's
`squint` annotation, the context arrives as `_squint`, and the envelope comes back as
`structuredContent` with the images as content blocks.

## The Python pack and its provisioning bundle

`tools/python/` is a uv project (`squint-tools`, on numpy, Pillow and onnxruntime) whose
`squint-tools serve` runs the stdio server. `context.py` parses the `_squint` the harness
injects into every call (the run and call ids, the images named by `image_args` and the frames
named by `frame_args`, each with lossless pixels by path under the run directory or inline, and
the rules' hints) and loads pixels as RGB arrays; a malformed context is a JSON-RPC error, which
the harness records as a tool error, never an answer the model acts on. `envelope.py` builds
result envelopes and returned images (lossless PNG named `crop:<call id>:<n>` or an aligned frame,
with its mapping in the parent), and splits the images' bytes out of `structuredContent` into the
MCP image blocks the client pairs with them by position and media type. A handler that cannot
answer (the segmenter artifact missing, an image that will not decode) gets a JSON-RPC error with
its words, which the harness records as a tool error, and the server goes on serving. The pack
carries three tools: `echo_region`, the transport proof, which places the given box on the image
as a region and returns the box cut from the named frame (or the image's raw frame) as its one
image; and `regions` and `classes` below.
`just python-check` lints and tests the pack offline; `just pack-test` also drives the real
server from the Rust client (`crates/squint-tools/tests/it/python_pack.rs`: an image and an
aligned frame by path in, a valid envelope with one image block out) and puts every manifest the
pack lists through the three adapters' wire transformations beside `submit_verdict`
(`crates/squint-cli/tests/it/python_pack_manifests.rs`).

**`regions`** (`regions.py`, task 5.4a) is the first real tool: where an aligned pair differs, as
high-recall pointers. It takes two frames in one grid, `frame` (usually the aligned candidate) and
`against` (usually `raw:source`), and marks a pixel changed when either of two photometric views
says so (the locally contrast-normalised luminance residual, which sees texture change through a
lighting change, and the blurred colour difference, which sees a flat area replaced by another
flat colour) or when the segmenter gives the two sides different classes and is confident on
both. Photometric components become boxes first, then class disagreements no box already covers;
each is labelled by the majority class on each side (`floor -> swimming pool`), scored by how
strongly it changed, nudged by `hints.regions` (`preserve` classes up by a quarter, `ignore`
classes halved, never dropped), and the strongest twelve are returned on `frame` with one image,
the frame with the boxes drawn. Nothing changed is `measured_nothing`. The segmenter sees each
frame as a 512×512 square: the longer edge resized to 512, the shorter padded with the image's
mean colour, centred, and the quarter-resolution semantic map read back from the same placement
(preprocessing `1-pad`, recorded in `MODEL.md`; an envelope's provenance holds only the fixed
fields the harness knows). Its test plants separated
colour blocks on synthetic scenes and requires every one found with IoU above 0.3 within the cap;
it needs the fetched segmenter and says so when skipped.

**`classes`** (`classes.py`, task 5.4b) names what the segmenter sees in one box of the same pair,
before and after: the classes covering the box on each side with their shares (those under five
per cent folded away, four at most) and the mean confidence there, as measurements
(`before:floor 0.62`, `after:signboard 0.71`, `confidence_before`, `confidence_after`), plus one
region of kind `class_difference` on the box when the dominant class differs. It decides nothing;
the rules do. The two tools share one loaded segmenter per server process. Its test holds three
named fixtures (small synthetic pairs under `tests/fixtures/classes/`) to the exact class lists
recorded beside them.

`scripts/build-bundle.sh` (`just bundle`) builds the wheel and assembles
`tools/python/dist/bundle-<version>/`: the wheel, a provisioning project whose `pyproject.toml`
pins `squint-tools==<version>` by path to that wheel, its `uv.lock`, `MODEL.md`, and
`bundle.toml` with the SHA-256 of every file, the declared Python and the executable path.

`squint tools setup --bundle <dir>` verifies every digest (a mismatch or an unlisted file is
`error: resolve` and nothing is installed), copies the bundle to
`$XDG_DATA_HOME/squint/tools/<package>-<version>/`, verifies the copy, runs
`uv sync --frozen --no-dev --python <declared>` there, and then the installed pack's `fetch`, so
the model artifacts are in `$XDG_CACHE_HOME/squint/models/` before the first run: from the
release the pack's record names, or with `--models-from <dir>` from a directory (the export's
output before publication); `--no-fetch` skips the step, and a tool that needs the artifact then
reports it missing. A fetch the pack refuses (a hash mismatch, an unreachable source) fails the
setup as `error: resolve` with the pack's own words. A pack configured by `package` and
`version` is then launched as `<dir>/.venv/bin/squint-tools serve` with `UV_OFFLINE=1`,
`UV_PYTHON_DOWNLOADS=never` and the lock digest in `SQUINT_LOCK_DIGEST`, which the pack echoes
in every envelope's provenance and the harness folds into the cache key (the same manifest from
a pack whose lock differs is a different tool). Downloading a published bundle by name is not
planned yet.

**The segmenter artifact** (task 4.6, `tools/python/MODEL.md`). `scripts/export_segmenter.py`
exports `tue-mps/ade20k_semantic_eomt_large_512` (EoMT-L on DINOv2, MIT weights and code) to ONNX
with inline dependencies of its own, checks the export against PyTorch on synthetic scenes
generated from a seed, and writes `record.json`; `MODEL.md` records the licences, the procedure
and versions, the artifact's SHA-256 (the script reproduces it), the preprocessing version, the
runtime and execution provider, the parity measured and the tolerance the parity test holds, and
the release URL the owner publishes the artifact and goldens at. `squint-tools fetch` (task
5.3) downloads the artifact and goldens from it into `$XDG_CACHE_HOME/squint/models/`, hashing
every byte against the record shipped in the package and keeping nothing that does not match;
`segmenter.py` loads the artifact with onnxruntime on the CPU and turns its two heads into the
semantic map, and the `segmenter`-marked parity test holds it to the recorded tolerance on the
goldens, skipping with the fetch command when they are absent. `squint tools setup` runs
`fetch` after the sync in task 5.2.

`just provisioning-test` builds the bundle and runs the ignored tests in
`crates/squint-tools/tests/it/provisioning.rs`: a clean-directory install whose executable
reports the version the lock pins and lists `echo_region`, `regions` and `classes`, an
`echo_region` call through MCP with its image block, a tampered bundle refused, and a setup that
fetches the segmenter and goldens from the export's directory into a models cache and refuses a
tampered copy. CI runs the install and call inside `unshare -rn`, a namespace with no network, as
the proof that the launch needs none.

## Testing

`squint-testkit` ships `squint-testkit-mcp`, a fake server with flags for every misbehaviour
the contract guards against (`--protocol-version`, `--no-tools-capability`, `--no-reset`,
`--hang-reset`, `--slow-call-ms`, `--ignore-stdin-close`, `--envelopes <file>`), and
`mcp_server::path()` builds and locates it. `cargo test -p squint-tools -- mcp::minimal` runs
the lifecycle tests.
