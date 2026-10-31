# Contract fixtures

`pixels/`: the pixel-digest vectors of table-manners v0.1.0 (`vectors/pixels/` in
github.com/markhayden/table-manners), copied unchanged. `digests.json` records each file's
`file_sha256`, so the copy checks itself; `crates/squint-core/tests/image_reference.rs`
reproduces every `pixel_sha256` through squint's own decode path (task 7.2).
