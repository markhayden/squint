<p align="center">
  <img src=".github/assets/logo.png" alt="squint" width="320" />
</p>

<p align="center"><strong>Because "looks good to me" doesn't scale.</strong></p>

squint is an agentic harness for evaluating the integrity of images. You give it an image to
check, whatever it should be checked against (a source photo, a reference subject, a brief),
and the rules that matter. A vision model works through the question with a set of tools,
then returns a verdict you can check: `pass`, `fail` or `unsure`, what it found, where it is
in the image, and every step it took to get there.

## Why

Image models are good enough to use in real pipelines and still wrong often enough that
someone has to look at every output. That someone is usually a person squinting at two images,
or a single similarity number that cannot tell a new background from a redrawn logo. Neither scales,
and neither explains itself.

Asking a vision model "does this look right?" gets you part of the way. It is good at
deciding whether a change matters and bad at noticing small ones: a logo nudged a few pixels,
a spot that went missing, a sixth finger in a crowd. squint gives the model
tools for exactly that, and makes it show its work.

## What it checks

Each of these is the same shape: something to compare against, rules, tools, a verdict.

- **Source fidelity.** An edited, restyled or re-generated image kept what mattered in the
  original. Swap the background of a portrait and the face, pose and wardrobe stay as shot;
  regrade a landscape for a campaign and the horizon and landmarks keep their shape. Only what
  was meant to change, changed.
- **Reference consistency.** A subject from a reference image shows up faithfully in a new
  scene. Put your dog on the bridge of a starship and it should still be your dog: same breed,
  same build, same spots.
- **Reality checks.** Mistakes no real photo would make: extra or missing limbs, fused
  fingers, impossible shadows and reflections, garbled text, objects melting into each other.
- **Brief compliance.** The image contains what was asked for and nothing it should not: the
  right objects in the right numbers, the right colours, text spelled correctly, no stray
  logos, watermarks or people.
- **Product fidelity.** A product shot keeps the exact product: label text, logo, shape,
  colour and proportions, whatever the scene around it.

These are where it starts. The harness is meant for any image evaluation you can write down as
rules and back with tools.

## How it works

You describe a **job**: the candidate image, the images it should be judged against (a source
photo, one or more reference subjects), the use case, and any requirements of your own in plain
language. squint turns the rules for that use case and your requirements into a numbered list
of **obligations**, each resolved to the image it judges, and builds a **checklist** the model
must answer item by item.

A vision model then looks at the images with a fixed set of **tools**: zoom into a region, align
a candidate to its source, and, as the harness grows, find changed regions, read text, or match
a subject. Every tool is deterministic, describes itself, and returns evidence, never a verdict.
Tool calls, turns and spend are all bounded.

The model ends by submitting a structured answer: a disposition for every checklist item
(`violation`, `held`, `allowed_change`, `hidden`, `cant_tell`) with a reason and a location.
**Code computes the verdict** from those dispositions and the obligations' kinds. A confident
violation of a mandatory obligation is `fail`; uncertainty on something that matters is `unsure`;
otherwise `pass`. A provider error, a tool crash or a blown budget is an `error`, never a verdict.
The model's own opinion is recorded and ignored.

Every run writes a journal and a report with every image the model saw, every tool result and
every location, viewable offline. Details: [`SPEC.md`](SPEC.md).

## How it is built

The method is the brief's: measure first, then earn each tool.

1. **Baseline.** Each candidate model alone, no tools, on public labelled sets for all five use
   cases.
2. **Equal visual access.** The same sets with zoom and alignment available but no measurements,
   so a later tool is compared against a model that could already look closely.
3. **One tool at a time.** A tool is admitted for a use case only if it beats that control on a
   sealed set by predeclared gates, confirmed on a second set. `inconclusive` is an allowed result.

Rust workspace, one binary, an optional Python tool pack over MCP, provider adapters written
against recorded fixtures. The harness core (milestones M0 and M1) runs a job end to end against a
scripted provider: rules, checklist, tool turns, repairs, ledger, journal, report, cache, and the
MCP transport with a provisioned pack; the three direct provider adapters (Anthropic, Google AI
Studio, OpenAI) are written against recorded conversations and smoked live from the release
archives (milestone M2). Install with `cargo install squint-cli` or from a release archive, copy a
config from [`examples/`](examples/) and set the one environment variable it names.

What has been measured so far, every number with its interval under
[`docs/results/`](docs/results/) and the story in [`docs/results/findings.md`](docs/results/findings.md):

- **Baseline (M3).** Six models, no tools, ten public development sets. Every model caught nearly
  every planted defect (52 to 60 of 60); the differences were in false rejects, hedging and cost,
  not detection. Opus cost about five times the cheapest model per item for no better detection.
- **Equal visual access (M4).** Five models under arms A and C, paired item by item. Looking
  closer did not change verdicts: 39 of 500 paired items flipped, almost all between pass and
  unsure on clean images. The run also showed that two tool interfaces were hard for models to
  call, which were fixed before the next run.
- **First gate (M5).** The `regions` change finder against the visual-access control on a sealed
  blind set of 20 source-fidelity items under Gemini 4 Argon: **rejected**, because the control
  already detected all ten planted failures. The tool took back the one false reject and lifted
  localisation to 10 of 10, neither of which the frozen gate scores. The sets need to be harder
  before a detection gate can separate tools.
- **Second gate.** The same comparison on a harder sealed set of 25 items in five new
  scenes, counted as a second attempt: **rejected** again. Both arms caught all 15 planted
  failures (16 once the owner corrected one label), and the tool changed no verdict. Removals a
  generator can plant cleanly are at ceiling for this model. `regions` stays an optional tool;
  whether it helps a cheaper model is tracked in issue #3.

squint is one tool in a chain an agent drives (generate, check with squint, mask with
[rascal](https://github.com/markhayden/rascal), paint, check again), so it presents the same
external interface as its siblings, defined by the shared contract
[table-manners](https://github.com/markhayden/table-manners): `squint serve` (MCP over stdio),
`squint describe --json`, `squint estimate`, and a `run.json` in every decision directory (ADR
0018, milestone M7). `squint playground` opens Wafflehouse, the shared local workspace, on squint. How an agent or a pipeline uses it: [`docs/agents.md`](docs/agents.md).

Plan: [`PLAN.md`](PLAN.md). Decisions: [`docs/adr/`](docs/adr/). Reviews of the spec, the plan and
each milestone: [`docs/reviews/`](docs/reviews/). Developer setup: [`docs/bootstrap.md`](docs/bootstrap.md).

## Origins

squint grew out of [Pixel](https://github.com/markhayden/bakin-bits-official/tree/main/agents/pixel),
the image-artist agent for [Bakin](https://github.com/markhayden/bakin), where generating
images turned out to be the easy half and knowing whether they were right was the hard one.

The project was called dejaview until 2026-10-29. The five library crates published to
crates.io under that name (`dejaview-core` and its siblings, 0.1.0) are superseded by the
`squint-*` crates and yanked; the results documents and receipts written before the rename keep
the old name, because their hashes cover it.

## License

[Apache-2.0](LICENSE).
