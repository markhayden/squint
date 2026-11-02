# ADR 0018: squint conforms to table-manners, the shared tool contract

Date: 2026-10-30. Status: accepted (owner decisions of 2026-10-30; the contract text is pending
its v0.1 tag).

## Context

squint is one tool in a chain an agent drives: an agent generates an image, squint checks it,
rascal (`../rascal`) masks what squint found, a filler paints inside the mask, and squint checks
the result again with the mask as the region where change was intended. Later tools (pixel)
join the same chain. Today an agent can drive squint only as a shell command (`squint check`,
the verdict in the exit code, `report.json` on disk); squint speaks MCP only as a client to its
Python pack. rascal already serves MCP (`rascal serve`, its ADR 0015). The two tools grew from
one stem, but their external surfaces differ in every place an agent touches them:

- rascal names images by workspace id (`raw:img-3`), squint by job role (`raw:candidate`), so a
  squint finding cannot be passed to rascal without rewriting its frame;
- rascal's copies of squint's envelope and manifest schemas (pinned by hash to an old commit)
  went stale when squint added the aligned frame and renamed `_dejaview`, and no test noticed;
- self-description is `ops list --json` in rascal and three commands in squint (`tools list`,
  `rules show`, `catalog show`);
- neither writes a summary of a run that another program can read without knowing the tool.

The owner's requirement: the same external interface in every tool, different intent and
internals, so that tools in one pipeline need no custom glue. The decisions below were settled
with the owner on 2026-10-30, refined by review from rascal's side.

## Decision

**A neutral contract.** The external interface lives in its own repository,
`table-manners`: a written specification, JSON Schemas (the source of truth, because the
Python packs read them), test vectors, a fixture per tool, and a Rust leaf crate of serde types
with no logic, published to crates.io (squint-cli is, so its dependencies must be). The contract
tags releases. squint pins a release, keeps a hash-checked copy of the schemas, and warns in CI
when a newer tag exists; an update is a deliberate commit. No tool takes a code dependency on
another tool. The contract has a core every tool follows (results, self-description, long runs,
the MCP server, exit codes) and profiles a kind of tool adds; v0.1 has one, images, and squint
implements the core and the images profile. v0.1 is the minimum the chain needs; squint's conformance is expected to find what
it gets wrong about long paid runs, and v0.2 follows before rascal's conformance merges.

**What crosses between tools.**

- *Image reference*: `file_sha256`, width and height of the file the caller holds (the check a
  receiver enforces), projection (flat or equirectangular) and topology (bounded or wrap), and
  an optional `pixel_sha256` with a canonical form and test vectors pinned in the contract (a
  mismatch with a matching file digest means a different decoder and is not an error). The
  first tool in a chain computes the reference; later calls carry it as the expected
  fingerprint and are refused on mismatch.
- *Location*: always on the original image, never on a crop, view or aligned frame. Fractions of
  width and height from the top-left, measuring edges not pixel centres, written to a quarter
  pixel at the longest side, snapped outward when rasterised. A list of parts, with
  `straddles_wrap` when the parts are halves of one shape across the wrap edge. Kinds `bbox`,
  `polygon`, `whole_image`, `absent` and `mask` (a feathered 8-bit greyscale rendering at the
  image's size, by reference, with a threshold); `view` is accepted as input only. A location
  carries a label, the tool's disposition and confidence, the evidence kind where there is one,
  and its source (tool, version, run id, item id). squint's roles (candidate, source) are not
  part of the key.

**Surfaces.**

- `squint serve`: an MCP server over stdio at protocol revision `2025-11-25`, following the
  contract's server conventions, which are lifted from `rascal serve` (the result as
  `structuredContent`, image blocks only when asked, validation failures in the result and
  operational failures as JSON-RPC errors carrying the error code, serial by default with a
  concurrency setting, and an independently written client in the test suite). Its tools are
  `check` (one decision: images in, the result out), `estimate` (the worst-case cost of a check
  and the session budget left) and the read-only self-description (`describe`, rules, tools,
  catalog). A check is not split into internal steps; the decision is the unit an agent wants.
- Images in any slot by path or inline bytes. Paths are allowed over stdio only inside the
  client's MCP roots, narrowed by config, and refused outright over any later HTTP transport;
  inline bytes are bounded, with the limit stated in `describe`. Each image out carries its
  reference and one locator the caller chose (path, inline, or an MCP resource URI).
- `squint describe --json`: the contract's description document (actions, inputs and parameters
  as JSON Schemas, renders, cost, expected duration, inline limits).
- Every decision directory gains `run.json`, and every run is an MCP resource by id
  (`squint://run/<id>/…`) announced in the first progress message, so an agent whose call timed
  out or whose connection dropped can still read it.
- `squint playground`: finds the shared playground program, offers to install it, starts it
  (`../playground-recommendation.md`). It carries no UI. *Amended 2026-10-31:* the program is
  `wafflehouse`, run as `wafflehouse --tool squint`, by the launcher agreement wafflehouse H1;
  SPEC "External interface" has the lookup and exit codes.

**Results.** The MCP result, the run resource and `run.json` are one shape: a shared header and
a body squint owns. The header: `status` (`completed`, `cancelled`, `error`, `invalid`); on error
or invalid a code from `usage`, `input`, `io`, `provider`, `budget`, `timeout`, `tool_process`,
`internal`, with `retryable` and a one-line reason; run id, tool name and version; started,
finished and duration; the input identity (a digest of the resolved inputs and parameters);
spend, marked measured; whether the cache was hit; every image reference in and out and every
location used or found, each with its role, kept on error and cancel so an agent can salvage
them; and a list of notes (repairs, tool refusals). A check whose image fails is `completed`
with the verdict `fail` in the body: an agent branches on `status` first.

**Long, paid calls.** A check is a plain `tools/call` with progress, not an MCP task (tasks are
experimental in `2025-11-25` and are moving out of the core protocol); the contract adds them
later as an option. Progress is a heartbeat: a notification on every journal step and every ten
seconds while waiting, with elapsed time and spend so far, a plain message with progress and
total where known; `describe` states the expected duration so a client can set its timeout.
Cancellation stops the provider call, leaves the run resource holding the partial journal with
status `cancelled` and spend to date, and returns no result. A retry of identical inputs within
the cache policy does not pay twice and reports the cache hit. Spend has two limits in config: a
per-call cap that a call may lower but never raise, and a per-session budget across all calls;
a per-period budget is later work, since it needs a durable ledger across server processes.

**Exit codes.** Shared across tools: 0 success, 3 error, 64 usage, and on cancellation 130 for
SIGINT and 143 for SIGTERM (squint exits 3 on cancellation today and changes). 1 (fail) and 2
(unsure) stay squint's.

**The reverse handoff** (rascal's mask to squint as the region where change was intended)
needs squint's `allowed` obligations to take a region. It is its own milestone after the
conformance work, and the contract's cross-tool acceptance test is its definition of done.

*Amended 2026-11-01 for table-manners v0.3 (PLAN.md M9; takes effect as each task lands;
squint's planning questions settled in table-manners `7f9316c`).* The contract now decides that
an estimate is a control, never a run, and that `estimate` takes the paid call's exact arguments;
squint's zero-spend estimate runs go, and `estimate` no longer reports "the session budget left"
in a body of its own (the estimate's `session` is the session resource; a command-line estimate
has none, `session_id` and `session` null, and no server accepts its binding). The error codes gain
`credential`, `cap` and `stale_estimate`; a call that needs a missing or invalid requirement is
refused `not_ready` before any run, and `credential` is a provider's 401 or 403 during a run.
Where the contract leaves the choice to the tool, squint chooses:

- **Configuration lookup.** `--config <path>`, else `./squint.toml`, else
  `$XDG_CONFIG_HOME/squint/squint.toml`; none found is not an error for `describe` or `serve`.
  An unconfigured server refuses a call that needs the config with `not_ready`, so it writes
  nothing and needs no output directory.
- **Readiness with no configuration** lists only the `config` requirement: the credential and
  the model are the config's choice, and naming one profile's key would be a guess. A config that
  loads is `present`, one that does not is `invalid`. The segmenter file is never hashed while
  describing, so it is `present` at most, never `valid`. squint emits no `env` requirement.
- **Binding and expiry.** The binding is a sha256 over the action, the arguments with each image
  replaced by its file digest (path and inline bind equal) and without `return_images` and
  `image_blocks`, the tool version, the identity of the redacted configuration and the session
  id; an estimate expires after one hour, and a binding the session never issued is stale.
- **Cap and budget.** Refusals by the per-call ledger, including a retry the decision envelope can
  no longer cover, are `cap`; refusals by the session ledger are `budget`.
- **Provider failures and `retryable`.** The contract makes `retryable` on a `provider` error the
  tool's judgement per occurrence. squint's error class `provider_rejected` (a status the retries
  never take: a 400, 404 or 422, or a 400 naming no cause after its one retry) maps to code
  `provider` with `retryable: false`, since an identical request would be refused again; a
  transport failure after the retries ran out (a 5xx, 429 or timeout) and a model-output failure
  are `provider` with `retryable: true`. A 400 whose body names no cause (each adapter reads its
  provider's error envelope: an empty body, or an envelope with no message, details or parameter)
  is retried once per turn inside the same retry budget and envelope reserve, charged like any
  other retry, because a provider has answered that way to requests an identical resend served
  (follow-up F3); the second is final. A 401 or 403 is never retried: it is `credential`.
- **Renders.** With no `--frame-ancestor`, `report view` allows no framing at all
  (`frame-ancestors 'none'`). A render never modifies the run directory, so `report export`
  writes to `$XDG_CACHE_HOME/squint/reports/<run id>.html` unless `--out` says otherwise.

## Alternatives

- **squint stays the origin of the shared schemas.** It is how rascal's copies went stale, and it
  would make squint own rules about masks.
- **Each tool writes its own copy of the interface.** Drift by construction.
- **A shared workspace or image registry across tools.** Shared ids without fingerprints, at the
  cost of coupling every tool to one store. Workspaces stay a tool's own extra (rascal keeps
  its own; squint stays stateless per check).
- **A pixel digest as the cross-tool key.** Portable for PNG, not for JPEG: decoders in Rust and
  Python may differ by a level in chroma upsampling and IDCT.
- **MCP tasks, or a custom start, status and result trio of tools.** Tasks are experimental and
  moving; a custom trio is the per-tool wiring the contract exists to remove. A run resource by
  id gives the useful half of tasks with standard MCP.
- **One verdict field shared by every tool.** It would make rascal and a future generator
  pretend to judge something.

## Consequences

- The deferrals "server mode" and `squint tools serve` in PLAN.md are replaced by milestone M7
  (conformance). M7's tasks are written after contract v0.1 is tagged, because they bind to the
  schema and field names the tag fixes. M6 stays blocked on the owner's `regions` decision
  (2026-11-01: M6 left the plan; the `regions` retest is issue #3).
- The server side uses `rmcp` pinned as `rascal serve` pins it (`=3.8.0`), with the features it
  needs for a server; progress, cancellation and resources are what the in-house client (ADR
  0015) never had to provide. Two MCP implementations in one binary is debt: M7 decides, with
  a bounded look, whether the pack client moves to `rmcp` too, and records it by amending ADR
  0015.
- The contract repository gains a cross-tool acceptance test: a squint check, its finding into a
  rascal remove, rascal's mask into a squint source-fidelity check as the allowed region, the
  header asserted at each step. squint's steps replay recorded model replies (`--replay`), so
  it runs in both tools' CI offline and without keys.
- Images by URL (SPEC decision sub-list) are not part of the contract; the HTTP transport, when
  it comes, refuses paths and adds resource locators.
