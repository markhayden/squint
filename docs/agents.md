# squint for agents and pipelines

squint speaks the waffle tools' shared contract, [table-manners](https://github.com/markhayden/table-manners)
v0.4.0, core and images profile (ADR 0018), pinned `=0.4.0` from crates.io. squint lists no operations profile: it has no artifact types, ports or
artifacts, and the reserved `export` and `import` are unknown tools. An agent or a script that knows the contract can
describe squint, estimate a check, run one, follow it, stop it and read it back, the same way it
does rascal. This page is squint's side; the contract's `CONTRACT.md` is the reference.

## Describe

```
squint describe --json [--config squint.toml]
```

Without `--config`, squint looks for `./squint.toml`, then `$XDG_CONFIG_HOME/squint/squint.toml`
(`~/.config/squint/squint.toml`). None found, or one that does not load, is not an error:
`describe` exits 0 and every action is `needs_setup` with one `config` requirement, `missing` at
the path to create or `invalid` with what to fix (an unset `${VAR}` by name).

The description document: one action per shipped use case (`source_fidelity`,
`reference_consistency`, `reality_checks`, `brief_compliance`, `product_fidelity`); `estimate` is
a reserved tool, not an action. Each action is of kind `task` (a whole check) with no `group`, and
`vocabulary` lists the disposition and confidence words a finding can carry. Each use case takes `candidate`, a `source` where its rule set
requires one, and `references` (a `many` slot: a non-empty list, each taken in once, in order), with `brief`, `requirements` (typed by squint's
obligation schema) and `max_spend_usd`. Each action's `readiness` comes from a local look,
never a network call: the configuration (`present`, `missing` or `invalid`), then the credential
variable of the configured profile (`present` or `missing` by name; its value is never kept or
printed), the configured model, and the segmenter file when `regions` or `classes` is enabled
(`present` once fetched, at its recorded size). Over MCP, `describe` also reflects this session's
provider answers: after an accepted request the credential and model are `valid`, after a 401 or
403 the credential is `invalid` and every later check is refused `not_ready` (restart squint
with a working key). A check on the scripted fake
needs only the configuration. The renders are `report`
(`squint report view {run.dir} --port 0 --frame-ancestor {frame_ancestor}`, kind `server`, ready
within ten seconds: its only stdout the URL, exit 0 when its stdin closes or on SIGTERM) and
`export` (`squint report export {run.dir}`, kind `document`: one self-contained page outside the
run directory, its absolute path the only stdout); neither modifies the run directory
(`docs/viewer.md`). The same document
is the `describe` tool over MCP.

## Estimate

```
squint estimate --json [--config squint.toml] --job job.toml [--max-spend-usd 0.10]
```

An estimate is a control, never a run: nothing is written, nothing is sent, no credential is
read. Over MCP the reserved `estimate` tool takes the action and exactly the arguments the call
would send, images included:

```jsonc
{ "action": "source_fidelity", "arguments": { "candidate": { "path": "/abs/edit.png" }, "source": { "path": "/abs/orig.png" } } }
```

and returns the contract's estimate (table-manners 4.5) as `structuredContent`:
`worst_case_usd` (what the limits check would reserve; every recorded provider replay spends
within it), `cap_usd` (the configured per-call cap, lowered by `max_spend_usd`), `allowed`, and
when not, `refused`: `readiness` (the action is not ready: see its `readiness`), `cap` or
`budget` (the session cannot hold it; `session` is the same observation as `squint://session`);
`effective.model` and `effective.config_identity`, with the rule sets' versions, the catalog
version and the decision runs, requests and attempts under `x-squint` (the same `effective` the
run of that call records in its invocation); `expires`, an hour on; and `binding`. The binding covers the action, the
arguments with each image as its file digest (a path and inline bytes of one file bind equal)
and its `expect`, squint's version, the configuration and the session; `return_images` and
`image_blocks` do not change it. A call that cannot be priced (no configuration, or a model the
catalog lacks) is refused for readiness with `worst_case_usd` and `effective.model` null (and
`effective.config_identity` null with no configuration), never zero. A wrong request is a
JSON-RPC error: arguments the input schema rejects are plain `-32602`; `max_spend_usd` above the
configured cap, an `expect` that does not match, an unknown action or a path outside the allowed
directories is
`-32602` with `{"reason": "invalid_arguments", "run": null, "error": {"code": "usage" | "input",
…}}`. An estimate takes no permit and claims no `client_run_id`. On the command line the same
document has `session_id` and `session` null, so no server accepts its binding; with no
configuration it answers like `describe`, refused for readiness; a wrong request exits 64.

## Serve

```
squint serve [--config squint.toml] [--out runs/] [--allow-dir /abs/images]… [--set serve.session_budget_usd=5]
```

MCP over stdio at protocol revision `2025-11-25` only, on `rmcp` `=3.8.0`; nothing but the
protocol on stdout. The configuration is found as for `describe` and read once, at start; to
take up a change, restart the server (a new session). A server with no configuration that loads
still starts and answers `describe`, `estimate` and its resources, writes nothing, and refuses a
check with `-32602`, `{"reason": "not_ready", "run": null, "requirements": [...]}` naming the
configuration. The same refusal stops any check that needs a requirement that is `missing` or
`invalid`, before any run (no directory, no spend, its `client_run_id` released), and names only
those: a check needs the configuration (a model the catalog lacks makes it `invalid`), the
profile's credential (unset, or rejected by the provider earlier in this session), the model,
and the segmenter file when `regions` or `classes` is enabled. An `estimate` is never refused
this way: it answers, its estimate `refused: "readiness"` when the action is not ready.
`squint check` exits 64 before any run directory with the same requirements on stderr (a
`--dry-run` needs only the configuration, a `--replay` no credential). Tools: `describe`,
`estimate`, and one per use case. A use-case call is one
ordinary decision (its directory, journal, report and `run.json`).

- **Images** by absolute `path` or `inline` bytes, each with an optional `expect` reference whose
  digest and dimensions are verified before anything runs. A path must lie inside a directory
  the server was started with by `--allow-dir` (absolute and existing, resolved at start; a bad
  one makes `serve` exit 64 before `initialize`); `[serve] allow_dirs` in the configuration can
  only narrow those, never add one. Paths are compared after resolving `..` and symlinks, so
  neither leads out. With no `--allow-dir` every path is refused and images go inline. A refused
  path is `invalid`, `input` (on `estimate`, the `invalid_arguments` refusal) with the same
  words whether or not the file exists. squint never asks the client for MCP roots;
  `squint://session` lists the allowed directories as `allowed_dirs`.
- **Spend**: the per-call cap from config, lowered by `max_spend_usd` (a value above the
  configured cap is `invalid`, `usage`, spending nothing; never silently lowered); a session
  budget (`[serve] session_budget_usd`, default $20) every decision reserves against before its
  first request. Over the cap the run ends `error`, `cap`; when the session cannot hold it,
  `error`, `budget`; neither is retryable unchanged.
- **Estimates**: send an estimate's `binding` back as `_meta["table-manners/estimate"]` on the
  paid call. If the call's inputs, the configuration, squint's version or the session differ, or
  the hour has passed, the run ends `invalid`, `stale_estimate`, with zero spend: estimate again.
  A call without one is checked against the limits alone; an estimate never reserves. Calls are serial unless `[serve] concurrency` allows more.
- **Progress**: with a progress token, a notification per journal line and a heartbeat after five
  seconds of silence; `progress` is elapsed seconds and never repeats; from the run's allocation,
  each one carries the run's reference in `_meta["table-manners/run"]`.
- **Correlation**: send `_meta["table-manners/client_run_id"]` with a call to find its run after a
  lost connection; an id is good once per server session (a reuse is `-32602`, reason
  `client_run_id_reused`, naming the earlier run). A request refused with a JSON-RPC error before
  any run (arguments the server rejects, a `not_ready` refusal) releases
  its id: fix the request and send it again under the same id.
- **Cancellation**: `notifications/cancelled` stops the provider call; the run's `run.json` says
  `cancelled` with spend to date, and the request gets no response. A call cancelled before its
  run is allocated (queued, or while its arguments are resolved) makes no run; its
  `client_run_id` stays claimed.
- **Resources**: `squint://runs` (newest first, running ones included, fifty to a page; the next
  page is `squint://runs?cursor=<next_cursor>`), `squint://session` (the session id, budget,
  spent, unresolved, reserved and remaining, and `allowed_dirs`), and per run `squint://run/<id>/run.json`,
  `…/report.json` and `…/journal`, readable after a timeout or a dropped connection.
- **Results**: the `run.json` below as `structuredContent`; `isError` for `invalid` and `error`.
  Arguments the input schema rejects and unknown tools are JSON-RPC errors, since no run exists.

## Call

```
squint call <action> --arguments args.json [--config squint.toml] [--allow-dir /abs/images]… [--out runs/] [--set PATH=VALUE]…
```

Runs any action from exactly the arguments an MCP call to it would send (table-manners 4.8): the
file is that JSON object, the same `candidate`, `source`, `references`, `brief`,
`requirements`, `max_spend_usd` a `tools/call` carries. Image paths follow the same rule as
`serve`: only inside a directory given by `--allow-dir` (narrowed by `[serve] allow_dirs`); with
none every path is refused and images go inline. A refused path is an `invalid` run, `input`, in
the same words `serve` uses, exit 64. The run is an ordinary command-line run (its directory and
`run.json`, no session); the result is printed on stdout and the exit code is `check`'s. An
unknown action, a reserved name (`describe`, `estimate`, `export`, `import`) or arguments squint
refuses exit 64 with nothing written.

To run a recorded call again, take a result's `invocation`, add a locator (`"path"` or
`"inline"`) beside each image's `expect`, write its `arguments` to a file and pass it to `squint
call <invocation.action>`: the new run records the same invocation and input identity, and the
`expect` makes sure the files are the ones first used.

```
squint call --bundle /abs/bundles/check.tar --allow-dir /abs/bundles [--config squint.toml] [--out runs/]
```

Replays an invocation bundle (table-manners 4.9), the `.tar` archive or its unpacked directory,
with the original files gone. The bundle must lie inside an `--allow-dir`. squint checks it whole
before anything runs and refuses, exit 64 with the reason on stderr and no run: a bundle outside
the allowed directories, a damaged or tampered member, a step another tool made (a host replays a
mixed bundle itself), an input left out on purpose (`reference_only`: there is no way to supply
it yet), and a step whose inputs are not the images its invocation names. It then copies the
inputs into its own cache (`$XDG_CACHE_HOME/squint/bundles/<sha256 of invocation.json>/inputs/`)
and runs each step as `squint call` with its invocation's arguments pointed there. One step
prints its result and exits as a call does. Several print one result per line and stop at the
first step that does not complete, exiting with its code; when every step completes, the exit is
the most serious step's (error or invalid, then fail, then unsure, then pass), so an earlier `fail`
is never hidden by a later `pass`. The call repeats exactly; a live model's verdict may not, so
replays that must match use the scripted provider. squint has no operations, so no step takes an earlier step's output.

## run.json

Every decision directory ends with `run.json`, written atomically, from the command line and from
MCP alike, cache hits and cancellations included: the contract's header (status, error code with
`retryable`, timestamps, input identity, spend (`unresolved_usd` is the part no provider
answer settled, counted at its worst case: show it as "at most"; `basis` is `estimated` when an
answer reported no usage and counts its worst case), cache, the images taken in with their
roles, every finding as a contract location on the file as stored), the contract's `verdict`
(table-manners 2.4: `value`, and in `where` the located places tied to the call's own
`requirements` that were violated or could not be told, on the candidate; a finding about a
built-in rule is in `locations.found` but never in `where`; empty on a pass) and squint's body
(verdict, exit code, whether two runs agreed, the report's path). A `fail` verdict is
`completed`. Every check action's description says `"answers": "verdict"`, so a tool that asks
squint whether something is still visible (rascal's remove with a checker) reads `verdict`
without knowing squint's body.

Every result carries `invocation` (table-manners 4.8): what the call resolved to, so it can be run
again elsewhere. `arguments` hold each image as `{"expect": <image reference>}` with no path or
inline bytes (a check by path and by inline bytes record the same invocation), with `brief`,
`requirements` and `max_spend_usd` as given and `return_images` and `image_blocks` left out;
`effective` is the estimate's object for the same call; `requirements` names what the call needed
(the configuration and the segmenter file by file name, the credential variable and the model by
name, never a value). A job file's own image ids and reference labels are not call arguments, so
they are left out and a note says so. `invocation` is null only on an `invalid` run whose
arguments did not resolve (an image that could not be read, does not match its `expect`, or a
path refused).

| squint error class | status | code | retryable |
|---|---|---|---|
| `resolve` | invalid | `usage` | no |
| `input`, `image_limit` | invalid | `input` | no |
| `credential` (a provider's 401 or 403 during the run; never retried, costs nothing) | error | `credential` | no |
| `provider` (a 5xx, 429 or timeout after the retries ran out), `submission_invalid`, `no_submission`, `model_output` | error | `provider` | yes |
| `provider_rejected` (any other status the provider refused the request with: a 400, 404, 422; a 400 whose body names no cause is retried once first) | error | `provider` | no |
| `tool` | error | `tool_process` | no |
| `cap` | error | `cap` | no |
| `budget` | error | `budget` | no |
| `stale_estimate` | invalid | `stale_estimate` | no |
| `io` | error | `io` | no |
| `cancelled` | cancelled | | |
| anything else | error | `internal` | no |

## Exit codes

| code | meaning |
|---|---|
| 0 | pass (and success for every other command) |
| 1 | fail |
| 2 | unsure (under the default policy) |
| 3 | error |
| 64 | a usage error, or an `invalid` request (a job that does not resolve, an image that is not the one named), whatever the policy |
| 130, 143 | cancelled by SIGINT, SIGTERM (`check`, `call` and `eval run`) |
| 69, 70, 74 | `playground` only: Wafflery unavailable, its startup failed, its install failed |

## Wafflery

Wafflery was called Wafflehouse until 2026-11-01.

`squint playground` runs `wafflery --tool squint` and exits with its code (128 + the signal
when it is killed by one); it takes no arguments. The program is `WAFFLERY_BIN` (absolute),
else `wafflery` on `PATH`, else `~/.local/bin/wafflery`; a bad `WAFFLERY_BIN` is an
error, never skipped. When none exists it exits 69 with instructions: no installer is published
yet, so there is nothing to offer. The launcher agreement is wafflery H1; everything after the
hand-off (the host, its library, the browser) is Wafflery's.

## Not yet

What squint's conformance found the contract needs is in table-manners `PROPOSALS.md`.
