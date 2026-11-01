# Owner decisions

Decisions only the owner can make, recorded before the task that needs them (PLAN.md task 0.4).
A blank entry blocks the task named in its row. Dates are when the owner decided.

| # | Decision | Needed before | Status |
|---|---|---|---|
| 1 | Initial judge-model subset for live fixtures and matrices | 2.1 | 2026-10-26: two per vendor: `claude-opus-5-5`, `claude-sonnet-5-5`, `gemini-4-argon`, `gemini-3.8-flash`, and two OpenAI vision models (frontier and mid-tier) named at 2.1. Confirmed 2026-10-26 from the live model list: `gpt-6.1-sol` (mid-tier, $2/$10) and `gpt-6.1-terra` (low-tier, $1/$5); the frontier `gpt-6-astra` was declined as cost prohibitive at $10/$50 |
| 2 | Anthropic account for recordings: enforced-thinking (created on or after 2026-08-31) or not | 2.5b | 2026-10-26: the existing account, with the prefix check switched on per request (`thinking-binding-controls` beta, `prefix_mismatch_behavior = "error"`) during recording |
| 3 | Spend cap: fixture recordings (all three vendors) | 2.5b | 2026-10-26: $25 for the Anthropic recordings (spent about $1.15 across Opus 5.5, Sonnet 5.5 and Haiku 5.5). 2026-10-27: $25 each for the Gemini and OpenAI recordings |
| 4 | Spend cap: release and gateway smoke tests | 2.8, 2.10b | 2026-10-27: the standing $25 per vendor applies; each vendor's account also carries its own credit limit. Estimated for the 2.10b live smoke: under $1 in total across the three direct profiles |
| 5 | Spend cap: image generation for public sets | 3.2a | 2026-10-27: the standing $25 per vendor applies (fal, OpenAI, Google); generation costs about one to five cents an image |
| 6 | Spend cap: arm A baseline | 3.8 | 2026-10-28: $75 for the whole matrix (`evalsets/matrix/baseline.toml`: all six models of decision 1, 1,320 decisions), chosen against `eval estimate` (envelope sum $4,585, floor $235, expected about $33 from recorded live usage: Anthropic ~$16, Google ~$8, OpenAI ~$8, each under the standing $25 per vendor) |
| 7 | Spend cap: arm C | 4.5 | decide before 4.5, against `eval estimate` |
| 8 | Spend cap: initial gate | 5.6 | decide before 5.6, against `eval estimate` |
| 9 | Spend cap: confirmation gate | 6.2 | decide before 6.2, against `eval estimate` |
| 10 | Labelling plan: worksheet format, batch size, adjudicator | 3.2b | 2026-10-26: draft answer key from construction plus a contact sheet per batch; batches of 20 to 25; the owner confirms or corrects |
| 11 | Reuse of prototype images, outputs and the segmenter export | 3.2a, 4.0, 4.6 | 2026-10-26: nothing from the prototype. Public upstream models (for example the EoMT-L ADE20K weights on Hugging Face under their own licences) are not prototype assets and may be exported afresh; golden panoramas are newly generated. Task 4.6 produces its own export and provenance record |
| 12 | Whether M6 (confirmation) is funded in this release | 6.1 | 2026-10-26: yes, funded; 2026-11-01: M6 out of the plan, issue #3 |
| 13 | Set generators and publication licence | 3.2a | 2026-10-27: a family-level mix of three generators so no set carries one vendor's fingerprint: FLUX.2 klein 4B (via fal, seeded), OpenAI gpt-image-2 and Gemini 3.1 Flash Image; one generator per family, recorded per item. Chosen after one sample from each on the same prompt. Publication licence and each vendor's output terms verified and recorded in ADR 0013 |

## Notes on the decisions

- **1.** Two models per vendor from the start; the rest of the catalog may join after the baseline.
  For OpenAI the pair is the mid-tier and low-tier model rather than frontier and mid-tier: the
  frontier model's output price would dominate every matrix row.
- **2.** The harness sends the thinking-binding controls header with `error` behaviour during fixture
  recording so any history edit fails loudly; the same header is available to consumers.
- **11.** "Nothing from the prototype" covers its images, outputs, stored views and ONNX export.
  Public upstream weights and code under their own licences are not prototype assets; task 4.6
  exports afresh and records provenance. Golden panoramas for the equirectangular path are
  generated or taken from CC-licensed sources.
- **12.** M6 is funded; `regions` can reach confirmed admission in this release. Superseded
  2026-11-01: both initial gates were rejected and the owner took M6 out of the plan; the retest
  of `regions` under a cheaper model is GitHub issue #3.
- **3 to 9.** Spend caps are set when each paid task is about to run, against `squint eval
  estimate` where it exists; the task refuses to start without its cap entry.

## Open questions for 3.2a

- Generators and publication licence (ADR 0013).
