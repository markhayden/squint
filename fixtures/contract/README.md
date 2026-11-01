# Contract fixtures

`pixels/`: the pixel-digest vectors of table-manners v0.1.0 (`vectors/pixels/` in
github.com/markhayden/table-manners), copied unchanged. `digests.json` records each file's
`file_sha256`, so the copy checks itself; `crates/squint-core/tests/it/image_reference.rs`
reproduces every `pixel_sha256` through squint's own decode path (task 7.2).

`squint/`: squint's conformance fixtures (tasks 7.10, 8.6), at contract v0.2: the description
and one `run.json` per status from the command line (`crates/squint-core/tests/it/contract_fixtures.rs`),
and what only a running server produces (`crates/squint-cli/tests/it/contract_fixtures_served.rs`):
a served run with its session and correlation ids and unresolved spend, an estimate, the runs
page, the session, and both refusals of a reused `client_run_id`. The volatile fields are fixed:
times, run ids, `/runs` paths, the session id `00000000-0000-4000-8000-000000000001`, versions
`0.0.0` and zeroed input identities on served documents. The test fails when squint emits something else; after a deliberate change, regenerate
with `UPDATE_CONTRACT_FIXTURES=1` and copy the files to table-manners `fixtures/squint/`, whose
tests hold them to the schemas.
