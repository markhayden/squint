# squint task checklist

Mirror of `PLAN.md` draft 4. Tick a task only when its acceptance criteria and verification pass
and the commit is on the milestone branch. Checkpoints need the owner's review before merge and
tag.

## Pre-M0
- [ ] Initial documentation commit on `main` (SPEC, PLAN, todo, ADRs, reviews, .gitignore; brief excluded)

## M0 Scaffold
- [x] 0.1 Workspace, crate skeletons, tool bootstrap, crate-graph check
- [x] 0.2 CI, lint, pre-commit hook, ignore rules
- [x] 0.3 Docs skeleton, review checklist, README sections
- [x] 0.4 Owner readiness document
- [x] Checkpoint M0 → tag `m0` (2026-10-26)

## M1 Skeleton
- [x] 1.1 Walking slice → tag `m1.1`
- [x] 1.2 Canonical images and frames registry
- [x] 1.3 Config loading, validation, shorthand job
- [x] 1.4a Obligation model, merge, validation
- [x] 1.4b Instances, rendering, shipped rule sets
- [x] 1.5a Manifest and envelope types, fake tool
- [x] 1.5b Checklist and evidence binding
- [x] 1.6a Submission schema and semantic validation
- [x] 1.6b Aggregation and decision tables
- [x] 1.7a State machine: tool turns
- [x] 1.7b State machine: final turn and terminal states → tag `m1.2`
- [x] 1.8 Repair protocol and retries
- [x] 1.9 Cost ledger
- [x] 1.10a Journal and artifact durability
- [x] 1.10b Report, schema, rebuild, prune
- [x] 1.11 Cache at the decision boundary
- [x] 1.12 Isolation, environment filter, exit codes
- [x] 1.13 Minimal MCP client against a fake server
- [x] 1.14 Minimal provisioning bundle proof
- [x] 1.15 Statistics contract frozen (ADR 0016 RNG)
- [x] Checkpoint M1 → tag `m1` (2026-10-26; `just check` under a minute, docs filled, review and coverage passes applied: docs/reviews/m1-checkpoint.md)

## M2 Provider adapters
- [x] 2.1 Model catalog and capability validation
- [x] 2.2 Wire schema transformations
- [x] 2.3 Image preparation and preflight
- [x] 2.4 Privacy enforcement before any live call
- [x] 2.5a Anthropic adapter and replay harness
- [x] 2.5b Anthropic recordings → tag `m2.1`
- [x] 2.6a Gemini adapter
- [x] 2.6b Gemini recordings
- [x] 2.7a OpenAI adapter
- [x] 2.7b OpenAI recordings
- [x] Follow-up from the recordings: models cited the `SF3` exemption in `allowed_by`; the rule text no longer asks them to cite anything, the rendered hint and the rejection name `held` as the disposition to use, and every vendor was re-recorded (2026-10-27)
- [ ] 2.8 Gateway profile candidates (optional)
- [x] 2.9 provider-data.md
- [x] 2.10a Packaging for the registry
- [x] 2.10b Binary distribution and live smoke (record: docs/reviews/m2-live-smoke.md)
- [x] Follow-up from the live smoke: the default budget planned an envelope above the default cap on Gemini 3.8 Flash and GPT-6.1 Terra; resolved by the envelope change above
- [x] Checkpoint M2 → merged to `main` and tagged `m2` on 2026-10-27 after the owner's go-ahead (docs/reviews/m2-checkpoint.md); the push is the owner's
- [x] From the M2 review (owner 2026-10-27: "do whatever you need to do"): the envelope plans each request once plus a per-run retry reserve of `retries ×` the largest bound, and a retry the balance cannot cover is refused with `error: budget`; the defaults now fit every shipped model and the examples are back on them
- [ ] 4.1 follow-up: a run whose tool images can carry the history past `many_image_threshold` fails preflight mid-run with `error: image_limit` after money is spent; at resolve, either prepare at `many_image_max_dimension_px` when the worst case can pass the threshold (and record it in `prep_version`) or refuse before the first call
- [ ] Recorded from the M2 review, not fixed: transport-error excerpts (up to 2 KB of a provider's error body) reach the journal and stderr without the redaction pass (the credential is never in a response body, so nothing has leaked; route them through the redaction list when the journal gains a redaction step); the `dist` installer in the `package` workflow is fetched by `curl | sh` without a checksum, as the generated release workflow does

## M3 Evaluation sets and baseline
- [x] 3.1 Set manifests, check, fetch, seal
- [x] 3.2a Set-building toolkit and ADR 0013 (toolkit, dry run and ADR done 2026-10-27; the public `squint-evalsets` repository is the owner's to create, with README and licence, before 3.2b publishes images)
- [x] 3.2b First dev set adjudicated and published (reference-consistency/pets-v1, 2026-10-27; images in markhayden/squint-evalsets, pushed by the owner)
- [x] 3.3a Source fidelity dev set (source-fidelity/scenes-v1, 2026-10-27)
- [x] 3.3b Reality checks dev set (reality-checks/people-v1, 2026-10-27)
- [x] 3.3c Brief compliance dev set (brief-compliance/stills-v1, 2026-10-27)
- [x] 3.3d Product fidelity dev set (product-fidelity/packshots-v1, 2026-10-27)
- [x] 3.3e Adversarial sets (five, one per use case) → tag `m3.1` (2026-10-27)
- [x] 3.4a Execution descriptor and durable evaluation ledger (2026-10-27)
- [x] 3.4b Matrix runner, arms and resume (2026-10-27)
- [x] 3.4c Pre-run estimate (2026-10-27)
- [x] 3.5a Metrics, localisation, report regeneration
- [x] 3.5b Gate lifecycle (stages, receipts, admissions)
- [x] 3.6 Static report viewer
- [x] 3.7 Canary and requalify
- [x] 3.8 Arm A baseline and publication (docs/results/baseline-2026-10, 2026-10-28: six models, 1,320 decisions, ledger $46.54 of $75 with $18.48 of it refused Anthropic calls charged at bound; the first run found the localisation scorer ignoring checklist-item violations, fixed and re-scored)
  - follow-ups from the baseline, owner decisions on the metric's definition, not blocking: `whole_image` labels against boxed predictions never match (label the defect's region, or let a whole-image label accept any box on that image); one defect reported under several obligations gives the same region several times and one-to-one matching counts the extras as spurious (count identical root geometries once with the union of their obligations); a call refused before processing (an error body, no usage) is charged at its bound in the ledger, which overstates spend after an outage or an empty balance; the estimate's expected-cost figure should lead with per-model observed medians (Anthropic ran at about five times the others per decision)
- [x] Checkpoint M3 → merged to `main` and tagged `m3` on 2026-10-28 (docs/reviews/m3-checkpoint.md); the push is the owner's
  - follow-ups recorded at the M3 review (2026-10-28), not blocking: a real SIGINT test of `eval run` through the binary; prompt equivalence across arms checked on the journaled system prompt; an automated devtools check of the viewer; a test that an environment-expanded matrix account is `[redacted]` in the report; the seal's image and rules components isolated in the mutation tests; one fixture per `sets check` failure mode; `JsonSchema` on `DecisionResult`, `Summary`, `Estimate` and `Matrix`; a concurrency cap on the viewer server; a harness-level test that instruction-like text in a label or brief never alters the prompt

## M4 Built-in tools and MCP client
- [x] 4.0 Alignment feasibility spike and ADR 0014 (2026-10-28: direct registration on normalised pyramids, within 0.05 px and 0.01° at 2048 px, sphere within 0.03°; `squint_tools::align`, failure fixtures under fixtures/images/align)
- [x] 4.1 Registry in production (2026-10-28: manifest `projections`, `ToolRegistry::with_projections` wrapping tools so envelopes are projected before the run sees them, the projection on the report's tool calls, the Tools section snapshot; `tools list` prints projections; built-in registration lands with the first built-in in 4.2a)
- [x] 4.2a `view`, `resize`, equirectangular geometry (2026-10-28: built-ins registered and listed; perspective views written from first principles, not ported from the prototype (owner decision 11), checked against the stored view and the analytic scene; view frames map to polygon footprints on the panorama, seam cut, pole as a full-width box)
- [x] 4.2b Viewer equirectangular overlay (2026-10-28: `geometry.js` mirrors `frames::sphere`; view crops and `view` locations drawn as footprints on the panorama; held to the Rust reference on stored footprints under node rather than by a rendered-pixel compare, which needs a browser)
- [x] 4.3a `align` flat and `visual_only` (2026-10-28; still to do from this line: run `eval gate` end to end through the binary now that arms C and D can run, and look at the gate tests' forty-second fake runs)
- [x] 4.3b `align` equirectangular (2026-10-28, with 4.3a: rotation on the sphere, warped panorama, the same envelope; aligned frames carry the rotation and map back through its inverse)
- [x] 4.3c `edge_similarity` and `change_map` → tag `m4.1` (2026-10-28: edge-map correlation and the locally normalised difference map, golden under fixtures/images/change-map)
- [x] 4.4a Full MCP client, ADR 0015, pagination, concurrency, images (2026-10-28: in-house client kept; multiplexed requests, pagination guards, concurrent tools and serial turns, lease waits for in-flight calls, outputSchema validation, exact blocks, isError truth table, frame chain, concurrent batch dispatch in the run)
- [x] 4.4b MCP failure modes (2026-10-28: one fake-server test per mode; leases keyed by decision directory; `shared = true` packs connected once per evaluation)
- [x] 4.5 Arm C on the dev sets (2026-10-28: `evalsets/matrix/arm-c.toml`, arms A and C, five models (Opus left out: over the per-decision cap under arm C and over the standing $25 per vendor with it), one repetition, bootstrap seed frozen; the paired-flips section with the bootstrap interval is in the report; the planner priced each stored envelope's inline pixels as text and put arm C envelopes at $500 each, now it prices the presentation the model reads, and the estimate gives $1.47 to $5.15 per decision; the run completed 2026-10-29: 1100 decisions, $28.31, three errors; results in `docs/results/arm-c-2026-10/`, the entry in `docs/results/findings.md`)
- [x] 4.6 Segmenter artifact and MODEL.md (2026-10-28: `scripts/export_segmenter.py` exports EoMT-L ADE20K to ONNX, 1.26 GB, hash reproduced on a third run under the pinned versions, the pinned upstream revision and the verified weights hash; parity within 6.5e-5 on class logits and 1.8e-3 on mask logits, enforced by the script; the record is committed as `tools/python/segmenter-v1.record.json`; publication is pending: the artifact, goldens and record wait in tools/python/dist/segmenter for the owner to upload as the `segmenter-v1` release assets)
- [ ] Follow-ups seen in the first arm C decisions (2026-10-28), after the run, since a manifest change changes the descriptor: the model's first `align` call named frames (`raw:candidate`) where the manifest takes image ids and was answered `invalid_args`, costing a turn per decision; say so in the argument descriptions (image ids, no `raw:` prefix) or accept a raw frame id as its image. The stored envelope artifact of an `align` result is truncated at the 2 MB artifact cap because it carries the aligned frame's base64 pixels, which the frame files already hold; store envelopes without image bytes (the frames keep them) so the viewer can read every envelope in full
- [x] Tools the models could not call, from the arm C run (2026-10-29, fixed on `m5-ml-tool` before the next tools run): an image argument now takes the image's raw frame id (`raw:candidate`), which is how the prompt names the picture, and the tool receives the image id (124 of 196 `align` calls had been refused for this); `view`'s refusal of angles on a flat frame now says to send `bbox` and what it is, and the angle parameters' descriptions say panoramas only (claude-sonnet-5-5 had made 325 `view` calls with `yaw: 0` and no `fov` on flat frames, all refused, and repeated the same call every turn); a tool-side `invalid_args` is two journal lines under one call id by design (dispatch, then the refusal with `executed: true`) and the report folds them, now said in the event's documentation
- [ ] A harder source-fidelity set before the confirmation stage (from the regions gate, 2026-10-29): the control detected every planted removal, so no detection gate can separate tools on sets like scenes-v1 and blind-v1; the next set needs smaller removals, one subject among many alike, partial changes, and changes under a strong restyle, at least 50 items in new families, opaque item ids, owner review. Whether a gate may credit false rejects taken back or localisation needs an ADR (the estimand is frozen before unsealing).
- [ ] Blind-set item ids name their construction (`bakery-front-no-awning`, `canal-footbridge-crop`), as the dev sets' do: the model never sees an id, so the gate is unaffected, but anyone reading the public manifest, the agent included, can infer the label. The next blind plan should give items opaque ids (`bakery-front-1` to `-4`) and keep the construction tag in owner storage only.
- [ ] From the arm C run, recorded: claude-sonnet-5-5 once sent the submission's `items` as a JSON string; the rejection quoted the schema path (`is not of type "array" at /items`) and the repair resent the string, so this is the model's behaviour, not a gap in the message. Gemini returned `HTTP 400 INVALID_ARGUMENT` twice minutes apart with request shapes that served 108 other items; a 400 is not retried and is charged at its bound, worth one retry when the body names no cause.
- [x] A static diagnostics page per decision (owner request 2026-10-29, modelled on the rascal run page; done the same day: `squint report export` writes the self-contained page, and both it and `report view` open each run with the timeline waterfall from the journal; the evaluation-level page linking every decision remains): `squint report export <decision dir>` writing one self-contained HTML file in funnel order (resolve, prepare, reserve, each turn, each tool call, submission, verdict), a timing waterfall from the journal's timestamps, each step's envelope, resolved parameters and artifacts with images embedded, and costs; then an evaluation-level page linking every decision of a run or gate. The data is all in the journal and artifacts already; the live `report view` stays.
- [ ] Release archives at the next version (2026-10-29): the `v0.1.0` release workflow never started, because the generated workflow granted the archive-smoke publish job no `contents` permission while that job asks for read (fixed in `dist-workspace.toml` with `github-custom-job-permissions`). The owner chose to leave `v0.1.0` without archives (the crates are published) and build them at the next version tag, after more local testing.
- [ ] M5 review follow-ups (2026-10-29, from docs/reviews/m5-checkpoint.md). Done 2026-10-29: CI fetches the segmenter from the segmenter-v1 release into a cache keyed on the record and runs the segmenter-marked tests; `tools setup` passes the pack's fetch progress on as it comes; a frame argument given as a bare image id resolves to its raw frame (the mirror of the `align` fix); a test pins that with no admissions.toml arm D is arm C. Still open: `initialize` could fail when the segmenter artifact is absent, so the harness gets the spec's `error: resolve` at resolve time (today each call that needs it gets a tool error).
- [ ] M4 review follow-ups (2026-10-28, from docs/reviews/m4-checkpoint.md), recorded rather than fixed: the alignment pyramid builds LCN, gradient and summed-area planes for every level including those finer than `max_side` that are never refined (build the raw halving chain only and the levels from `finest` up); the server's stderr reaches the journal only inside a failed call's error text, never as its own event or at shutdown; `paired_metrics` pairs cells by model only and does not refuse a pair whose decision runs, policy or rules digest differ; a lease is never released, so interleaving runs would reset between every call (sequential today); a response with a string id is dropped as a notification; the reader's line length is unbounded; `isError = false` with `invalid_args` is accepted as is; an `invalid_args` summary skips `max_summary_bytes`; `sanitise` can map two frame ids to one file name; `image::open` in the built-ins uses the image crate's default limits rather than the config's; the sphere failure quality reports `overlap: 1.0`; the coarse search cost grows with the longer side (limit shifts to the shorter); `view` recomputes the basis per pixel and accepts `also == frame`; `root_of_frame` and `to_root` loop unbounded where `chain` guards with 64; a `view` location on an aligned panorama passes `validate` and fails late; the viewer geometry test skips silently when `node` is absent; the paired metrics do not report unpaired items, treat an empty family as one pseudo-family and print no "inconclusive" note; `MatrixMetrics` has no schema; `Mapping` carries `Vec<f64>` where `[f64; 9]` would let the schema say 3×3; the artifact name omits the opset. Coverage gaps left open: image blocks and frame handles over the MCP wire end to end, a JSON-RPC `error` reply to `tools/call`, the viewer's chain walker under node, stored sphere alignment fixtures, `executed` after a tool-side `invalid_args`, offline tests of the export script, `tools list` printing projections
- [x] Checkpoint M4 → tag `m4` (2026-10-29: review and coverage in `docs/reviews/m4-checkpoint.md`, arm C results in `docs/results/arm-c-2026-10/`; `m4-tools` merged into `main`)

## M5 First ML tool
- [x] 5.1 Python pack skeleton and MCP server (2026-10-29: `context.py` parses `_squint` images and frames and loads pixels by path or inline; `envelope.py` builds envelopes and PNG images and splits them into MCP image blocks; `echo_region` cuts the box from a named frame; a malformed context is a JSON-RPC error; `crates/squint-tools/tests/it/python_pack.rs` drives the real server from the Rust client with an image and an aligned frame, run by `just pack-test` and CI's python job; numpy and Pillow are the pack's dependencies, locked)
- [x] 5.3 Segmenter fetch and parity (2026-10-29: `models.py` ships the record inside the package and `fetch` downloads or copies every file of it with each byte hashed, refusing a mismatch and keeping nothing; `segmenter.py` runs the ONNX export with onnxruntime 1.31 and computes the semantic map; the `segmenter`-marked parity test passes on the four goldens within the recorded tolerance and skips with the fetch command when the artifact is absent; `tools setup` running `fetch` after the sync is part of 5.2)
- [x] 5.4a `regions` tool (2026-10-29: `regions.py` over two frames in one grid, photometric change (normalised residual and blurred colour difference) and confident class disagreement, components boxed and labelled by majority class each side, scored, nudged by `hints.regions`, capped at twelve, one overlay image; `requires = ["align"]`; the segmenter pads to the 512 square (preprocessing `1-pad`); planted colour blocks on synthetic pairs are all found with IoU above 0.3 across five seeds; every pack manifest passes the three adapters' wire transformations in `crates/squint-cli/tests/it/python_pack_manifests.rs`)
- [x] 5.4b `classes` tool (2026-10-29: `classes.py` over a box on two frames in one grid: the classes covering it on each side with shares and confidence as measurements, one `class_difference` region when the dominant class differs; three named fixtures under `tests/fixtures/classes/` return exactly their recorded class lists; the manifest passes the three adapters' wire transformations with the rest of the pack)
- [x] 5.2 Provisioning bundle for the real pack (2026-10-29: the bundle built from the pack with numpy, Pillow and onnxruntime locked; `tools setup` runs the installed pack's `fetch` after the sync, from the release or `--models-from <dir>`, `--no-fetch` to skip, a refusal is `error: resolve`; the installed server lists `echo_region`, `regions` and `classes`; the lock digest rides on the registry into the cache key; the bundle script no longer removes the export's output) → tag `m5.1`
- [x] 5.5 Sealed blind set for source fidelity (2026-10-29: `source-fidelity/blind-v1`, five new scene families, twenty items, built from a plan drawn by seed 20261029 into owner storage; the owner confirmed all twenty from the contact sheets; published and sealed with the labels moved to owner storage, `labels_sha256` in the manifest, images at 9820cd4 of the evalsets repository)
- [x] 5.6 Initial gate D_regions versus C (2026-10-29: `evalsets/matrix/gate-regions.toml` frozen, estimate approved under the standing posture, 40 decisions for $1.32 with no errors; `eval gate --stage initial` scored against the owner-held labels: rejected, both arms detected 10 of 10, the tool took back the one false reject and lifted localisation to 10 of 10; receipt `8e0c8307a72475ac`, labels published and the set now `heldout`, admissions unchanged; results in `docs/results/gate-regions-initial-2026-10/`, the entry in `docs/results/findings.md`)
- [x] Checkpoint M5 → tag `m5` (2026-10-29: review and coverage passes done, findings fixed or recorded in `docs/reviews/m5-checkpoint.md` with the spec success criteria checked one by one; the two test passes take about three minutes each; README "How it is built" carries the measured results; final artifacts rebuilt: staged-registry install smoke, this machine's release archive and its smoke, the Python bundle's clean install; published to crates.io as squint 0.1.0 on 2026-10-29 after the rename from dejaview (six crates, the `squint-cli` binary crate last under the new-crate rate limit), and `cargo install squint-cli --version 0.1.0 --locked` from outside the repository installs a binary that passes the offline smoke; `m5-ml-tool` merged into `main`)

## M6 Confirmation
- [x] Owner decision on ADR 0017 (2026-10-30): option 1, a harder blind set and a second initial gate of `regions` at `gate.hypotheses = 2`; PLAN task 6.0
- [x] Task 6.0 (2026-10-30): blind-v2 built (seed 20261030), owner-confirmed 25 of 25, sealed, and scored as the second initial gate of `regions` at two hypotheses: **rejected** (both arms 15/15, receipt `be41ae402334a394`); results in docs/results/gate-regions-initial-2-2026-10
- [x] Owner: check `boatyard-5` (2026-10-30): the door moved; the label is corrected to fail in `evalsets/source-fidelity/blind-v2/corrections.toml`
- [ ] Scoring reads a set's `corrections.toml` before blind-v2 is run again (the exploratory Flash run would otherwise score `boatyard-5` as pass)
- [ ] Owner decision after the second rejection: a gate under a cheaper model, a gate on false rejects and localisation (ADR 0017 option 2), or `regions` out of source fidelity (option 3); M6 6.1 and 6.2 stay blocked
- [ ] 6.1 Confirmation set (50+, two batches of 25)
- [ ] 6.2 Confirmation gate and admission record
- [ ] Checkpoint M6 → tag `m6`

## M7: contract conformance (ADR 0018)

- [x] ADR 0018 and SPEC "External interface" (2026-10-30): the table-manners contract decisions recorded on squint's side
- [x] `table-manners` v0.1.0 (2026-10-30): drafted from rascal's seed section, split into a core and an images profile, reviewed from rascal's side against rascal `917c5c3`, tagged
- [x] M7 task list in PLAN.md (2026-10-30), written against v0.1.0
- [x] Owner: table-manners v0.1.0 published to crates.io (2026-10-30)
- [x] Owner: the shared program is `wafflehouse`, launched as `wafflehouse --tool squint` (wafflehouse H1, accepted 2026-10-31 with squint's amendments); its release installer is a later subgate
- [x] 7.1 The contract pinned (2026-10-30): `table-manners =0.1.0`, schemas copied and hash-checked under `schemas/contract/`, CI warns on a newer tag
- [x] 7.2 Image references in and verified (2026-10-30): `expect` on job images, `error: input`, stored dimensions and the contract pixel digest; the twelve vectors reproduce
- [x] 7.3 Contract locations out (2026-10-30): `contract::location`, the inverse EXIF orientation, quarter-pixel precision, seam footprints as two parts
- [x] 7.4 `run.json`, the result header and exit codes (2026-10-30): written last and atomically on every surface; `invalid` exits 64; signals exit 130 and 143 for `check` and `eval run`
- [x] 7.5 `squint describe --json` (2026-10-30): one action per use case and `estimate`, the vocabulary from squint's own types; argument errors exit 64
- [x] 7.6 `estimate` for a check (2026-10-30): `squint estimate` from catalog prices with no credential, `--max-spend-usd` lowers the cap; every recorded replay spends within its estimate
- [x] 7.7a `squint serve`: the server, `describe` and `estimate` (2026-10-30): rmcp `=3.8.0` at 2025-11-25 only; the independent client `squint_testkit::mcp_client`; a call's arguments become a job (`contract::job_from_call`)
- [x] 7.7b `check` over `serve` (2026-10-30): one tool per use case, paths inside the client's roots and `[serve] roots`, the session budget as a shared ledger, calls serial by default
- [x] 7.8 Progress, cancellation and runs as resources (2026-10-31): a journal observer drives progress, heartbeats every 5 s, a cancel leaves a `cancelled` run and no response, `squint://runs` and per-run resources, a cache hit costs nothing
- [x] 7.9 `squint playground` (2026-10-31): `wafflehouse --tool squint` by wafflehouse H1, lookup `WAFFLEHOUSE_BIN`/`PATH`/`~/.local/bin`, the launcher's exit code kept; the installer a testkit-only seam until a release is approved
- [x] 7.10 Conformance fixtures (2026-10-31): `fixtures/contract/squint/` generated and held by `contract_fixtures.rs`; copied to table-manners `fixtures/squint/` (`6025a06`), which validates them
- [x] 7.11 The MCP client on `rmcp` (bounded) (2026-10-31): ADR 0019, the pack client stays in-house
- [ ] Contract v0.2 from what M7 finds, before rascal's conformance merges; written up in table-manners `PROPOSALS.md`. Found so far:
  - an image slot that takes a list (squint's `references`; v0.1 slots take one image)
  - how a render command reports the URL it serves (wafflehouse brief, gap 1)
  - MCP roots are deprecated by SEP-2577 after `2025-11-25`: the contract's path rule needs its replacement before any tool moves revision
  - readiness: `describe` without a config, telling installed-but-unconfigured from unavailable (wafflehouse C1/S1.2)
  - a provider's 401 or 403 is not retryable; squint maps every provider failure to `provider`, retryable (wafflehouse S1.4)
- [x] Checkpoint M7 → tag `m7` (2026-10-31): merged as `eef1f10`, released by the owner
- [x] Follow-up: the M7 checkpoint's review and test-coverage pass (agent-skills review and test), skipped before the merge; run 2026-10-31; the release stands, findings are the M7 review fixes below
- [x] M7 review fixes (branch `m7-review`, merged 2026-10-31):
  - [x] `estimate` over MCP checks paths against roots like a check (review C1, security); a missing path is refused in the words of one outside; the checked real path replaces the given one (S3)
  - [x] an image path is a regular file read only up to the decode bound; inline data over the bound refused undecoded (I1)
  - [x] every decision writes its own `run.json` on every exit; a cache-store failure is a note; serve returns the decision's run, never a second one (I2); `Services::allocated` names the directory when it exists
  - [x] a reference's `images.in` slot is `references` (I3)
  - [x] serve honours `[rules]` and resolves rule paths as the CLI does (I6); a rules file that cannot be read makes `describe` and `serve` exit 64
  - [x] client roots parsed as `file:` URLs: an empty or `localhost` host, percent-decoded (I5)
  - [x] `straddles_wrap` never on a bounded image; a running run listed under its action (coverage 3, 4)
  - [x] vacuous tests made to bite: cancelled spend, heartbeat gaps, path/inline identity; argument errors exit 64 per command; `status_of` exhaustive; `run.json` in the secret scan. Found on the way: a decode bound is `image_limit` (contract `input`, review S1); `input_identity` leaves out how an image arrived (the fixtures' identities changed: refresh table-manners `fixtures/squint/` with v0.2)
  - [x] closed by default: with no client roots and no `[serve] roots`, a path is refused and images go inline (review S6, owner 2026-10-31)
  - [x] only checks take the permit: `describe` and `estimate` answer while a check runs; a queued check beats and a cancel while it waits leaves no run (I4; C4 review 9, 10)

## M8: table-manners v0.2 (branch `m8-v0.2`)

- [x] Owner: M8 approved 2026-10-31 ("keep going")
- [x] 8.1 The contract at v0.2 (2026-10-31): path dependency on `../table-manners` until v0.2.0 is published; schemas and fixtures at 0.2.0, new fields at their v0.1 values
- [x] 8.2 The session (2026-11-01): UUID v4 at `serve` start, `squint://session` from an atomic ledger snapshot, the association written into each run's directory at allocation so `run.json` and running entries carry `session_id`; CLI runs null; `estimate` drops `session_budget_left_usd`
- [x] 8.3 Correlation and the run reference (2026-11-01): `client_run_id` validated and claimed on acceptance (before the permit wait, held when cancelled there), a reuse `-32602` with the refusal and the earlier run; the association in the run's directory at allocation; `_meta["table-manners/run"]` on every notification from allocation, the URI out of the message
- [x] 8.4 Unresolved spend (2026-11-01): `TransportError::sent`; a call that never left and an auth rejection cost nothing, one sent with no usage or cancelled in flight counts its bound as unresolved, a cancel between attempts charges nothing more (the ledger had charged it twice); `unresolved_usd` in the report, `run.json` and the session
- [x] 8.5 Runs by page (2026-11-01): `squint://runs{?cursor}`, fifty newest first by (started, id), only the page's run files opened, an opaque base64url cursor, `next_cursor` null on the last page
- [x] 8.6 Fixtures (2026-11-01): squint's v0.2 set, the served documents generated by driving `squint serve`; they pass table-manners' fixtures test at `0aa506c`; handed to rascal as a directory
- [ ] After v0.2.0 is published: `table-manners = "=0.2.0"` from crates.io instead of `../table-manners`
- [ ] Checkpoint M8 → tag `m8`
