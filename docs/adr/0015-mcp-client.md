# ADR 0015: The MCP client stays in-house

Date: 2026-10-28. Status: accepted; revisited and kept by ADR 0019 (2026-10-31).

## Context

SPEC "Transports" leaves open whether the harness speaks MCP through the official Rust SDK
(`rmcp`) or its own client, to be decided at M4 after a bounded look (SPEC open question 2, PLAN
task 4.4a). Task 1.13 built the lifecycle in-house against a fake server; 4.4a completes it:
paginated listing, concurrent calls where a manifest allows them, frames in the injected context,
exact image content blocks, `outputSchema` validation and the full `isError` table.

What the contract needs of a client (SPEC "MCP over stdio, revision 2025-11-25"): a spawned
child on stdio under an environment allowlist with the run directory as working directory and
stderr captured; `initialize` requiring exactly `2025-11-25` and the `tools` capability;
paginated `tools/list` frozen into manifests; `tools/call` with one id per call for the process
lifetime, concurrent only when declared; a per-call deadline after which the client sends
`notifications/cancelled`, tombstones the id and never reuses the process; a custom
`squint/reset` request; exclusive leases across runs with one acquisition deadline; shutdown
escalation; and a synchronous surface, because the `Tool` trait the run drives is synchronous.

`rmcp` 3.8.0 (Apache-2.0, crates.io, checked 2026-10-28) is the official SDK. It knows
`ProtocolVersion::V_2025_11_25` (and `2026-07-28`, which drops `initialize`), offers a
`TokioChildProcess` stdio transport behind a feature, and is built on tokio: its client is an
async service with `futures`, `tokio-util`, `tracing` and friends as dependencies, `reqwest`,
`hyper` and `oauth2` optional.

## Decision

Keep the in-house client (`crates/squint-tools/src/mcp/`, about nine hundred lines with the
pack, and as many again of tests): a synchronous, multiplexed JSON-RPC client over the child's
stdio.

- A reader thread routes every response to the request waiting for its id; requests from several
  threads share the process, so tools whose manifests declare `concurrent = true` run together
  on one server while the others take turns under the pack's serial lock.
- Every wait has its own deadline. A timeout cancels, tombstones and taints; a stray id, a
  malformed frame or the end of the stream fails every pending request and taints.
- `tools/list` follows `nextCursor` to the end, refusing a repeated cursor or more than 64 pages.
- The result is the envelope validated against the tool's `outputSchema`; the content blocks must
  be exactly the image blocks of `images`, in order and media type, and nothing else; `isError`
  with `invalid_args` is the one recoverable form.
- Leases: a new run waits, within its one deadline, for the previous run's in-flight calls, then
  resets or restarts the server.

## Alternatives

- **`rmcp`.** Its transport, versioning and typed messages are sound, and it would track the
  protocol as it moves. Against it here: the harness would run a tokio runtime inside a
  synchronous `Tool` call on a thread of its own (the run already drives tool calls on threads,
  with cancellation listened for beside them); the contract's custom request (`squint/reset`),
  its lease and taint rules, its exact-blocks rule and its one-deadline acquisition would all
  still be written here, around the SDK; and the SDK's dependency tree is large for a binary
  whose MCP needs fit in a few hundred lines. The client is small enough to read in one sitting,
  which the review posture (docs/review-checklist.md) values over tracking an SDK.
- **Speaking `2026-07-28`.** The newer revision replaces `initialize` with per-request metadata.
  The first release pins `2025-11-25`, the newest revision with `initialize`, which the Python
  pack and the fake server speak; moving is a contract change recorded here when it happens.

## Consequences

- The client's surface is the harness's own: additions follow the spec, not an SDK's release
  notes, and the fake server (`squint-testkit-mcp`) grows a flag for every misbehaviour the
  tests need (`--page-size`, `--loop-cursor`, `--concurrent`, `--extra-measurement`,
  `--text-block`, beside the lifecycle flags of 1.13).
- A batch of calls whose tools all declare `concurrent = true` is dispatched together by the run
  (task 4.4a) and ingested in call order, so evidence items keep their order whatever the
  completion order.
- If a later release needs streamable HTTP, authentication or the `2026-07-28` revision, this
  decision is revisited with that need in hand; `rmcp` would then be weighed against those
  features rather than against stdio alone.
