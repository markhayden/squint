# ADR 0019: The pack client stays in-house beside an rmcp server

Date: 2026-10-31. Status: accepted (PLAN task 7.11).

## Context

ADR 0015 kept squint's MCP client for its tool packs in-house, weighing `rmcp` against stdio
alone. Milestone M7 put `rmcp` `=3.8.0` into the binary for `squint serve` (ADR 0018), so one of
ADR 0015's reasons, the SDK's dependency tree, no longer counts against it. Task 7.11 is the
bounded look ADR 0018 promised: would the pack client be better on `rmcp`'s client, judged by the
failure modes task 4.4b tests (pagination, concurrency, a dying pack, cancellation)?

What the look found in `rmcp` 3.8.0 (source read, 2026-10-31):

- `list_all_tools` follows `nextCursor` until the server gives none, with no guard: a server that
  repeats a cursor is listed forever. squint's client refuses a repeated cursor and more than 64
  pages (4.4b, the fake server's `--loop-cursor`).
- A request with a timeout sends `notifications/cancelled` when it runs out, and a closed transport
  fails every pending request (`ServiceError::TransportClosed`). squint's client does both, and
  also tombstones the id and taints the process so it is never reused.
- The client surface is async and offers no typed way to send squint's custom `squint/reset`
  request beside the protocol's own.
- What ADR 0015 kept squint's regardless stands: a synchronous `Tool` surface the run drives on
  threads, leases across runs with one acquisition deadline, the exact-image-blocks rule, the
  `outputSchema` check and the `isError` table.

## Decision

Keep the in-house pack client. Moving it would wrap the SDK in the same squint-specific rules it
has now, add a guard the SDK lacks, and run a tokio runtime inside every synchronous tool call, for
no capability squint's packs use. The server and the client stay on different stacks: `serve` on
`rmcp` (the contract's convention for servers), the pack client squint's own (ADR 0015).

## Consequences

- The reasons that would reopen this are concrete: a pack protocol revision past `2025-11-25`
  (`2026-07-28` drops `initialize`), a network transport for packs, or authentication. Each is a
  feature the SDK tracks and squint's client would have to build.
- If the client moves later, the repeated-cursor guard and the taint-on-timeout rule move with it
  as tests first; the fake server's flags already cover them.
