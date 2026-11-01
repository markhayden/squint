# squint for agents and pipelines

squint speaks the waffle tools' shared contract, [table-manners](https://github.com/markhayden/table-manners)
v0.3.0, core and images profile (ADR 0018); until v0.3.0 is published squint builds against the
contract's `v0.3` branch. An agent or a script that knows the contract can
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
a reserved tool, not an action. Each use case takes `candidate`, a `source` where its rule set
requires one, and `references` (a `many` slot: a non-empty list, each taken in once, in order), with `brief`, `requirements` (typed by squint's
obligation schema) and `max_spend_usd`. Each action's `readiness` comes from a local look,
never a network call: the configuration (`present`, `missing` or `invalid`), then the credential
variable of the configured profile (`present` or `missing` by name; its value is never kept or
printed), the configured model, and the segmenter file when `regions` or `classes` is enabled
(`present` once fetched, at its recorded size). Over MCP, `describe` also reflects this session's
provider answers: after an accepted request the credential and model are `valid`, after a 401 or
403 the credential is `invalid` (restart squint with a working key). A check on the scripted fake
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
`effective.model` and `effective.config_identity`, with the decision runs, requests and attempts
under `x-squint`; `expires`, an hour on; and `binding`. The binding covers the action, the
arguments with each image as its file digest (a path and inline bytes of one file bind equal)
and its `expect`, squint's version, the configuration and the session; `return_images` and
`image_blocks` do not change it. A call that cannot be priced (no configuration, or a model the
catalog lacks) is refused for readiness with `worst_case_usd` and `effective.model` null (and
`effective.config_identity` null with no configuration), never zero. A wrong request is a
JSON-RPC error: arguments the input schema rejects are plain `-32602`; `max_spend_usd` above the
configured cap, an `expect` that does not match, an unknown action or a path outside the roots is
`-32602` with `{"reason": "invalid_arguments", "run": null, "error": {"code": "usage" | "input",
…}}`. An estimate takes no permit and claims no `client_run_id`. On the command line the same
document has `session_id` and `session` null, so no server accepts its binding; with no
configuration it answers like `describe`, refused for readiness; a wrong request exits 64.

## Serve

```
squint serve [--config squint.toml] [--out runs/] [--set serve.session_budget_usd=5]
```

MCP over stdio at protocol revision `2025-11-25` only, on `rmcp` `=3.8.0`; nothing but the
protocol on stdout. The configuration is found as for `describe` and read once, at start; to
take up a change, restart the server (a new session). A server with no configuration that loads
still starts and answers `describe`, `estimate` and its resources, writes nothing, and refuses a
check with `-32602`, `{"reason": "not_ready", "run": null, "requirements": [...]}` naming the
configuration. Tools: `describe`, `estimate`, and one per use case. A use-case call is one
ordinary decision (its directory, journal, report and `run.json`).

- **Images** by absolute `path` or `inline` bytes, each with an optional `expect` reference whose
  digest and dimensions are verified before anything runs. When the client offers MCP roots, a
  path must sit inside them, and inside `[serve] roots` when the config names any; with no roots
  from either, images go inline (a path is refused).
- **Spend**: the per-call cap from config, lowered (never raised) by `max_spend_usd`; a session
  budget (`[serve] session_budget_usd`, default $20) every decision reserves against before its
  first request. Calls are serial unless `[serve] concurrency` allows more.
- **Progress**: with a progress token, a notification per journal line and a heartbeat after five
  seconds of silence; `progress` is elapsed seconds and never repeats; from the run's allocation,
  each one carries the run's reference in `_meta["table-manners/run"]`.
- **Correlation**: send `_meta["table-manners/client_run_id"]` with a call to find its run after a
  lost connection; an id is good once per server session (a reuse is `-32602`, reason
  `client_run_id_reused`, naming the earlier run). A request refused with a JSON-RPC error before
  any run (arguments the server rejects, a failed `roots/list`) releases its id: fix the request
  and send it again under the same id.
- **Cancellation**: `notifications/cancelled` stops the provider call; the run's `run.json` says
  `cancelled` with spend to date, and the request gets no response. A call cancelled before its
  run is allocated (queued, or while its arguments are resolved) makes no run; its
  `client_run_id` stays claimed.
- **Resources**: `squint://runs` (newest first, running ones included, fifty to a page; the next
  page is `squint://runs?cursor=<next_cursor>`), `squint://session` (the session id, budget,
  spent, unresolved, reserved and remaining), and per run `squint://run/<id>/run.json`,
  `…/report.json` and `…/journal`, readable after a timeout or a dropped connection.
- **Results**: the `run.json` below as `structuredContent`; `isError` for `invalid` and `error`.
  Arguments the input schema rejects and unknown tools are JSON-RPC errors, since no run exists.

## run.json

Every decision directory ends with `run.json`, written atomically, from the command line and from
MCP alike, cache hits and cancellations included: the contract's header (status, error code with
`retryable`, timestamps, input identity, spend (`unresolved_usd` is the part no provider
answer settled, counted at its worst case: show it as "at most"; `basis` is `estimated` when an
answer reported no usage and counts its worst case), cache, the images taken in with their
roles, every finding as a contract location on the file as stored) and squint's body (verdict,
exit code, whether two runs agreed, the report's path). A `fail` verdict is `completed`.

| squint error class | status | code | retryable |
|---|---|---|---|
| `resolve`, `auth` | invalid | `usage` | no |
| `input`, `image_limit` | invalid | `input` | no |
| `provider`, `submission_invalid`, `no_submission`, `model_output` | error | `provider` | yes |
| `tool` | error | `tool_process` | no |
| `budget` | error | `budget` | no |
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
| 130, 143 | cancelled by SIGINT, SIGTERM (`check` and `eval run`) |
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
