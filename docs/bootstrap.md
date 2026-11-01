# Bootstrap

One manual step, then everything else is a `just` recipe.

```
cargo install just --version 1.59.0 --locked    # or: brew install just
just bootstrap                                  # pinned cargo-deny and cargo-dist
just check                                      # what every commit must pass
```

`just check` runs `cargo fmt --all -- --check`, Clippy with `-D warnings` and the tests through
`cargo nextest run` (installed by `just bootstrap`; it runs test binaries in parallel), each over the
workspace with `--features squint-cli/testkit` and then over `squint-cli` alone without it (the only
crate with a feature), each crate's integration tests being one binary (`tests/it/main.rs`, a module per
file: a new test file needs its `mod` line there), `cargo deny check licenses bans sources` (advisories need the network and run in
CI as `just deny-advisories`), `scripts/check-crate-graph.sh`, `scripts/check-packaging.sh`
(every asset a crate embeds sits inside that crate and in the file list `cargo package` ships),
`scripts/check-attribution.sh` (no agent attribution in any commit message or tracked file),
`scripts/test-hooks.sh` (the credential scanner and both hooks, end to end in a scratch
repository) and `scripts/test-checks.sh` (the checks fail when they should).

`just hooks` installs the hooks from `scripts/hooks/`. The pre-commit hook scans every staged blob
for credential-shaped values first, then runs fmt and Clippy; only the exact synthetic values in
`scripts/synthetic-tokens.txt` are allowed, and each must contain `SYNTHETIC`. The commit-msg hook
refuses a message that carries agent attribution (a `Co-Authored-By` trailer naming an assistant,
a "Generated with" line or an assistant's no-reply address): the repository shows no agent
association, in its history or its files.

The `testkit` feature of the `squint` package enables the scripted fake provider and the
`--provider-script` flag for tests and debugging. Release builds never enable it.

Toolchain: `rust-toolchain.toml` pins an exact stable version (bumped deliberately, since `just
check` runs Clippy with `-D warnings`) with `clippy` and `rustfmt`; rustup installs them on first use. `uv` is needed from M1 task 1.14 onward for the Python tool pack.

## Package names

The six package names (`squint`, `squint-core`, `squint-providers`, `squint-tools`,
`squint-eval`, `squint-testkit`) were all unregistered on crates.io on 2026-10-26, so no
fallback name is needed.

## Publishing

The shipped rule sets, prompt template and model catalog live inside the crates that embed them
(`crates/squint-core/rules/`, `crates/squint-core/prompts/`,
`crates/squint-providers/catalog.toml`), so a published crate builds from its own files. The core
crate's dev-dependency on the testkit is path-only (no version): `cargo publish` drops it, which
breaks the cycle between the two crates.

- `just publish-dry-run` (`scripts/publish.sh --dry-run`) packages and verifies every crate as
  `cargo publish --workspace` would, uploading nothing. The real publication is the owner's:
  `scripts/publish.sh` from a clean checkout whose `HEAD` carries the tag `v<version>`, after
  `cargo login`. Cargo orders the crates itself: core, testkit, providers, tools, eval, squint.
- `just registry-install-test` (`scripts/registry-install-test.sh`) installs `squint` from the
  packaged `.crate` files in a scratch directory outside the repository, with the sibling crates
  supplied from the extracted packages the way crates.io would supply them, then runs the offline
  smoke. The stage is removed on exit; `REGISTRY_STAGE_DIR` may name it, and must then be new or
  empty.
- `just smoke <binary>` (`scripts/offline-smoke.sh`) is that smoke on any built binary:
  `--version`, `tools list`, `rules show`, and a `check` without a credential that must stop with
  `auth:` and exit 64 before anything leaves the machine.

The `package` workflow runs all three on every push. `squint --version` prints the short commit;
a packaged crate has no repository, so the build script reads the commit `cargo package` records
in `.cargo_vcs_info.json` instead.

## Releasing

`cargo-dist` installs an executable named `dist` (run `dist --version`; the version is pinned in
the justfile and in `dist-workspace.toml`). `dist-workspace.toml` is the one source for binary
distribution: three archive targets (macOS arm64, Linux x86_64 and arm64), no installers, GitHub
releases. `dist generate` rewrites `.github/workflows/release.yml` from it; that file is never
edited by hand, and `just dist-check` (`dist generate --check` plus `dist plan`) fails when it is
stale. The `package` workflow runs the same check.

- A pushed tag that parses as a version (`v0.1.0`) runs the release workflow: `dist` builds the
  three archives on their own runners, the `archive-smoke` workflow unpacks each one on its own
  architecture and runs the offline smoke on the binary inside, and only then is the GitHub
  release created (`github-release = "announce"` with the smoke as the one publish job). A
  failing smoke on any architecture leaves no release behind.
- The release dry run is the `archive` job of the `package` workflow on every push: each
  architecture builds its archive exactly as the release workflow does and smokes it, nothing is
  tagged or released. A pull request additionally runs the release workflow in upload mode, which
  builds the archives as workflow artifacts without releasing.
- Locally, `just archive-build` builds this machine's archive into `target/distrib/` (`dist build`
  for the host target; the other targets cross-compile only on their runners) and
  `just archive-smoke target/distrib/squint-cli-<target>.tar.xz` smokes it, verifying the checksum
  file when one sits beside the archive. CI sets `REQUIRE_CHECKSUM=1`, so a missing checksum file
  fails there.
- `just live-smoke <binary>` (`scripts/live-smoke.sh`) is the paid smoke behind owner decision 4:
  one `check` per direct profile whose key and account are in the environment, with each vendor's
  cheapest model on the fixture image pair, from any binary (the unpacked archive for a release
  record). It prints verdict, exit code and cost per vendor and never prints an account id or key.
  The M2 record is `docs/reviews/m2-live-smoke.md`.

`examples/` holds one ready-to-run config per use case; each shows the `--set` overrides that
point it at another direct profile, and a CLI test dry-runs every example and those overrides.

## The set-building toolkit

`scripts/sets/` is a second small uv project (`cd scripts/sets && uv sync`), used to generate
evaluation sets by construction (docs/evalsets.md, ADR 0013). `just sets-check` lints and tests
it and runs its offline dry run; the `python` CI job does the same. Live generation needs
`FAL_KEY`, `OPENAI_API_KEY` and `GEMINI_API_KEY` in the environment (see `.env.example`).

## Running CI locally

`brew install act` once, then `just ci-local` runs the GitHub Actions workflow in a Linux
container on this machine, before anything is pushed. It validates the workflow file, installs
the pinned toolchain and tools as the runner would, and runs `just check` under GNU tools rather
than the macOS ones. GitHub's cache action and runner image specifics are the only parts it does
not reproduce.

## Credentials

squint reads API keys from the environment only and never writes them to disk. Keep them in a
`.env` file (git ignores it; `.env.example` lists the names) and load it into the shell before a
live run:

```sh
set -a; source .env; set +a
```

The `SQUINT_*_ACCOUNT` entries feed the `[privacy] account` attestation through `${…}` expansion
in the config, for example `account = "anthropic:${SQUINT_ANTHROPIC_ACCOUNT}"`.

