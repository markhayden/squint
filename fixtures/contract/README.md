# Contract fixtures

`pixels/`: the pixel-digest vectors of table-manners v0.1.0 (`vectors/pixels/` in
github.com/markhayden/table-manners), copied unchanged. `digests.json` records each file's
`file_sha256`, so the copy checks itself; `crates/squint-core/tests/it/image_reference.rs`
reproduces every `pixel_sha256` through squint's own decode path (task 7.2).

`squint/`: squint's conformance fixtures (tasks 7.10, 8.6, 9.10, 10.6) for table-manners
`fixtures/squint/`, at contract v0.4.0 (the unpublished `v0.4` branch until its release), per
CONTRACT.md 7 item 7: the v0.2 and v0.3 sets at 0.4.0 and v0.4's additions. Every action is of
kind `task`, with no `group`; every result carries `invocation`, null only on an `invalid` run
whose arguments did not resolve (`run-invalid.json`, an image that does not match its `expect`,
and the `run-path-*.json` refusals). squint lists no operations profile, so there is no
`run-op-*`, `refusal-binding-*`, `artifact-*`, `export-*` or `import-*` file, and no
description has `artifact_types` or `ports`. Two generators write them, and the same tests hold
the committed copy to what squint emits now:

- `crates/squint-core/tests/it/contract_fixtures.rs`, from squint's own code: the descriptions
  (`describe.json`; `describe-unconfigured.json`, only the `config` requirement, `missing`;
  `describe-credential-missing.json`, `-present`, `-valid` and `-invalid`, the last two from the
  session's provider answers; `describe-ready.json`, the free scripted profile with nothing to
  set up) and one command-line `run.json` per status (`run-completed`, `-fail`, `-invalid`,
  `-error`, `-cancelled`).
- `crates/squint-cli/tests/it/contract_fixtures_served.rs`, by driving the `squint` binary:
  estimates (`estimate-allowed.json`, `estimate-refused-cap.json`, `-budget`, `-readiness`,
  `estimate-refused-readiness-unconfigured.json` with the worst case, model and configuration
  identity null, and `estimate-cli.json` from the command line with `session_id` and `session`
  null); served runs (`run-served-cancelled-unresolved.json`; `run-stale-estimate-image.json`,
  `-argument`, `-session` (another server's binding, which also stands for a changed
  configuration, since the configuration is the session's) and `-expired`; `run-cap.json`,
  `run-budget.json`, `run-credential.json` from a 401 during the run;
  `run-accepted-after-refusal.json`, a request refused with no run whose id the corrected
  request then takes); the runs page; the sessions (`session.json`, `allowed_dirs` from
  `--allow-dir`; `session-no-allow-dir.json`, `[]`; `session-narrowed.json`, an `--allow-dir`
  narrowed by `[serve] allow_dirs` to its subdirectory); the refusals (`refusal-no-run.json`
  while the earlier request waits for the permit, `refusal-cancelled-before-allocation.json`
  after it was cancelled there, `refusal-earlier-run.json`, `refusal-not-ready.json`, and from an
  estimate `refusal-invalid-arguments.json`, a cap above the configured one, and
  `refusal-invalid-arguments-path-outside.json`, a path outside the allowed directory); the path
  rule's served refusals, each `invalid`, `input`: `run-path-outside.json`, `run-path-dotdot.json`
  (a `..` escape), `run-path-symlink.json` (a symlink inside that points out),
  `run-path-missing.json` (a missing file outside, in the same words as an existing one) and
  `run-path-no-allow-dir.json` (a server started with none); and from `squint call`, offline on
  the scripted provider: `run-call.json`, the call of `run-completed.json`'s invocation with a
  `path` beside each image's `expect`, and `bundle-check/`, an invocation bundle of that run
  (`invocation.json` and its two input members, `inputs/<file_sha256>.png`, as squint's test-only
  writer lays them out), with `run-bundle-replay.json`, its replay by `squint call --bundle` from
  a copy in a fresh directory, from another working directory. Both record `run-completed.json`'s
  invocation exactly. An estimate is never a run, so no run fixture is one.

The volatile fields are fixed: times and expiries, run ids, `/runs` paths, the session id
`00000000-0000-4000-8000-000000000001`, versions `0.0.0`, zeroed input identities, bindings and
(in the served fixtures, but for the invocations `run-call.json` and `run-bundle-replay.json`
keep as squint wrote them) configuration identities, allowed directories as
`/allowed` (`/allowed/sub` for the narrowed one) and the path rule's outside directory as
`/outside`, and `/home/user/.config` for the configuration lookup. The test fails when squint
emits something else; after a deliberate change, regenerate with `UPDATE_CONTRACT_FIXTURES=1`
(both generators: the CLI one needs `--features testkit`; the core one first, since the call and
the bundle are made from `run-completed.json`) and copy the directory to table-manners
`fixtures/squint/`, removing files squint no longer produces.
`every_fixture_validates_as_table_manners_reads_it` (in `contract_fixtures.rs`) checks the
directory the way table-manners' `tests/fixtures.rs` does at its `v0.4` branch, with the crate's
embedded schemas: the schema and the type by file name (`describe*.json`, `estimate-*.json`,
`run-*.json`, `runs*.json`, `session.json` and `session-*.json`, `refusal-*.json`), every
`bundle-*/` directory a manifest valid against `invocation-bundle.json` whose listed members are
beside it with the `file_sha256` it states; contract `0.4.0` and tool `squint` wherever a document
names them, a bundle's steps included; amounts to at most six decimals; in every result, a
bundled step's included, an invocation of the result's action with no locator. Beside those it
holds squint's own v0.4 promises: every action of kind `task` with no `group`, no
`artifact_types`, `ports` or `artifacts`, `invocation` on every result and null only on an
`invalid` one, requirement names that are never paths (no `/` or `\`), nothing in a bundle
directory the manifest does not list, and no operations-profile file. squint's proof that
`describe` and `estimate` make no provider call and write nothing (7 item 7) is
`describing_and_estimating_call_no_provider` in `crates/squint-cli/tests/it/serve_session_cli.rs`.
