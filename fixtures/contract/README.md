# Contract fixtures

`pixels/`: the pixel-digest vectors of table-manners v0.1.0 (`vectors/pixels/` in
github.com/markhayden/table-manners), copied unchanged. `digests.json` records each file's
`file_sha256`, so the copy checks itself; `crates/squint-core/tests/it/image_reference.rs`
reproduces every `pixel_sha256` through squint's own decode path (task 7.2).

`squint/`: squint's conformance fixtures (tasks 7.10, 8.6, 9.10) for table-manners
`fixtures/squint/`, at contract v0.3.0 (the unpublished `v0.3` branch until its release), per
CONTRACT.md 7 item 7. Two generators write them, and the same tests hold the committed copy to
what squint emits now:

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
  request then takes); the runs page and the session; and the refusals (`refusal-no-run.json`
  while the earlier request waits for the permit, `refusal-cancelled-before-allocation.json`
  after it was cancelled there, `refusal-earlier-run.json`, `refusal-not-ready.json`, and
  `refusal-invalid-arguments.json` from an estimate). An estimate is never a run, so no run
  fixture is one.

The volatile fields are fixed: times and expiries, run ids, `/runs` paths, the session id
`00000000-0000-4000-8000-000000000001`, versions `0.0.0`, zeroed input identities, bindings and
configuration identities, and `/home/user/.config` for the configuration lookup. The test fails
when squint emits something else; after a deliberate change, regenerate with
`UPDATE_CONTRACT_FIXTURES=1` (both generators: the CLI one needs `--features testkit`) and copy the
directory to table-manners `fixtures/squint/`, removing files squint no longer produces.
`every_fixture_validates_as_table_manners_reads_it` (in `contract_fixtures.rs`) checks the
directory the way table-manners' `tests/fixtures.rs` does, with the crate's embedded schemas: the
schema by file name (`describe*.json`, `estimate-*.json`, `run-*.json`, `runs*.json`,
`session.json`, `refusal-*.json`), contract `0.3.0`, tool `squint`, every amount to at most six
decimals. squint's proof that `describe` and `estimate` make no provider call and write nothing
(7 item 7) is `describing_and_estimating_call_no_provider` in
`crates/squint-cli/tests/it/serve_session_cli.rs`.
