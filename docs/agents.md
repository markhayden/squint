# squint for agents and pipelines

squint speaks the waffle tools' shared contract, [table-manners](https://github.com/markhayden/table-manners)
v0.1.0, core and images profile (ADR 0018). An agent or a script that knows the contract can
describe squint, estimate a check, run one, follow it, stop it and read it back, the same way it
does rascal. This page is squint's side; the contract's `CONTRACT.md` is the reference.

## Describe

```
squint describe --json --config squint.toml
```

The description document: one action per shipped use case (`source_fidelity`,
`reference_consistency`, `reality_checks`, `brief_compliance`, `product_fidelity`) and `estimate`.
Each use case takes `candidate`, a `source` where its rule set requires one, and `references` (a
list), with `brief`, `requirements` (typed by squint's obligation schema) and `max_spend_usd`.
`cost.needs_env` names the credential variable of the configured profile; its value is never read
or printed. The same document is the `describe` tool over MCP.

## Estimate

```
squint estimate --config squint.toml --job job.toml [--max-spend-usd 0.10]
```

The worst case one check would reserve, priced from the catalog with no credential, and whether
the cap covers it (`fits`). Every recorded provider replay spends within its estimate. Over MCP,
`estimate` also reports the session budget left.

## Serve

```
squint serve --config squint.toml [--out runs/] [--set serve.session_budget_usd=5]
```

MCP over stdio at protocol revision `2025-11-25` only, on `rmcp` `=3.8.0`; nothing but the
protocol on stdout. Tools: `describe`, `estimate`, and one per use case. A use-case call is one
ordinary decision (its directory, journal, report and `run.json`).

- **Images** by absolute `path` or `inline` bytes, each with an optional `expect` reference whose
  digest and dimensions are verified before anything runs. When the client offers MCP roots, a
  path must sit inside them, and inside `[serve] roots` when the config names any; with no roots
  from either, images go inline (a path is refused).
- **Spend**: the per-call cap from config, lowered (never raised) by `max_spend_usd`; a session
  budget (`[serve] session_budget_usd`, default $20) every decision reserves against before its
  first request. Calls are serial unless `[serve] concurrency` allows more.
- **Progress**: with a progress token, a notification per journal line and a heartbeat after five
  seconds of silence; `progress` is elapsed seconds and never repeats; the first names the run.
- **Cancellation**: `notifications/cancelled` stops the provider call; the run's `run.json` says
  `cancelled` with spend to date, and the request gets no response.
- **Resources**: `squint://runs` (newest first, running ones included), and per run
  `squint://run/<id>/run.json`, `…/report.json` and `…/journal`, readable after a timeout or a
  dropped connection.
- **Results**: the `run.json` below as `structuredContent`; `isError` for `invalid` and `error`.
  Arguments the input schema rejects and unknown tools are JSON-RPC errors, since no run exists.

## run.json

Every decision directory ends with `run.json`, written atomically, from the command line and from
MCP alike, cache hits and cancellations included: the contract's header (status, error code with
`retryable`, timestamps, input identity, measured spend, cache, the images taken in with their
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
| 69, 70, 74 | `playground` only: Wafflehouse unavailable, its startup failed, its install failed |

## Wafflehouse

`squint playground` runs `wafflehouse --tool squint` and exits with its code (128 + the signal
when it is killed by one); it takes no arguments. The program is `WAFFLEHOUSE_BIN` (absolute),
else `wafflehouse` on `PATH`, else `~/.local/bin/wafflehouse`; a bad `WAFFLEHOUSE_BIN` is an
error, never skipped. When none exists it exits 69 with instructions: no installer is published
yet, so there is nothing to offer. The launcher agreement is wafflehouse H1; everything after the
hand-off (the host, its library, the browser) is Wafflehouse's.

## Not yet

What squint's conformance found the contract needs is in table-manners `PROPOSALS.md`.
