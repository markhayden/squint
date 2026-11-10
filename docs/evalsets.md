# Evaluation sets

An evaluation item is a complete job plus labels. The job is canonical and runnable by
`squint check --job`; the labels live in a separate file so blind sets ship without them. The
contract is `SPEC.md` under "Evaluation" ("Set manifest", "Seal"); this page is how the sets are
laid out and handled. How public sets are generated, labelled from construction records and
reviewed image by image arrives with task 3.2a.

## Layout

```
evalsets/<use-case>/<set>/        the set's id is <use-case>/<set>
  set.toml                        [set] metadata and [[item]] entries (id, family, job)
  families.toml                   [[family]] id and description
  labels.toml                     [[label]] per item; absent on a blind set, whose manifest
                                  carries labels_sha256 instead
  seal.toml                       written by `sets seal`
  corrections.toml                optional: [[correction]] per label the owner corrected after
                                  scoring; outside the seal (see below)
  items/<item>/job.toml           a full job; image paths relative to the set directory, each
                                  pinned by sha256
  items/<item>/*.png              the images, fetched by hash (see below)
```

Every file is parsed strictly: an unknown key anywhere (an `expected` on an item, a label in a
job) is a load error, which is how a public manifest or job is kept free of answer labels.

`[set]`: `id`, `use_case`, `split` (`dev`, `heldout`, `blind`), `licence`, `images_repo`,
optional `images_rev`, `labels` or `labels_sha256`, `families`, and `rules = { base, version }`
pinning the shipped rules the items are judged under. The fixture set
`fixtures/evalsets/source-fidelity/pairs-v1` is the smallest complete example and the one the
tests mutate.

## Commands

- `squint sets check <root or set directory>` loads every set under `<root>` (or the one set)
  and fails on the first problem: a set not at its id's path; an unknown or wrongly versioned
  rules base, or one for another use case; an item whose job has another use case or does not
  resolve against the pinned rules (a missing required role, for instance); an undeclared family;
  an image without a pin, missing, or with another hash; labels wrong for the split (a dev or
  heldout set names a label file that labels every item exactly once and nothing else; a blind
  set has no label file and carries `labels_sha256`); a region with the wrong geometry; and,
  across sets of one use case, a family in more than one split. Exit 0, or 1 with the reason on
  stderr; 64 when the root holds no set.
- `squint sets fetch --set <id or path> [--root evalsets]` places every pinned image beside the
  manifest from `images_repo`: `github.com/<owner>/<repo>` serves the file raw at `images_rev`,
  an `https://` base serves it at `<base>/<set id>/<path>`, and anything else is a directory
  relative to the set with the same `<set id>/<path>` layout. A download whose hash differs from
  the pin is refused and leaves nothing behind; an image already in place with the right hash is
  left alone.
- `squint sets seal --set <id or path> --attest "<text>" [--labels-out <dir>]` checks the set,
  then writes `seal.toml`: one SHA-256 per component (sorted item ids, family assignments, every
  canonical job, every image file, the pinned rules' name, version and content, the label file or
  its external hash) and one over all of them, with the time and the owner's attestation that no
  judge or tool has seen the items. For a blind set whose labels still sit beside the manifest,
  `--labels-out` names the owner-storage directory the label file is moved to (refused when it
  lies inside any git repository); the manifest's
  `labels = …` line becomes `labels_sha256 = …`. The timestamp and attestation are outside the
  hash, so re-sealing unchanged content reproduces it.
- `squint sets verify-seal --set <id or path>` recomputes every component and names the ones
  that changed. Exit 0, or 1 with the components on stderr.

A label found wrong after a set was scored is not edited in place: `labels.toml` stays as sealed,
because the seal and any gate receipt bind its hash. The owner records the fix in
`corrections.toml` beside it, one `[[correction]]` per item with the label fields, the label it
replaces (`was`) and how it was found (`found_by`); the results that scored the set note the
corrected reading. Scoring does not read corrections yet.

The seal records the owner's attestation; prior exposure of the items to a judge or tool cannot
be proven by code, and gate scoring treats journal searches as supporting evidence only.

## Building a set (task 3.2a, ADR 0013)

`scripts/sets/` is a small Python toolkit (`uv run squint-sets …` from that directory). A
**plan** names the set and its families; each family is one scene or subject with one generator
(`fal` for FLUX.2 klein, `openai` for gpt-image-2, `gemini` for Gemini 3.1 Flash Image, or
`fake`), a source prompt and seed, the protected subjects, a brief, and its items, each made from
the source by one construction op. `fixtures/sets/houses-v1.plan.toml` is the dry-run plan.

Three family kinds cover the five use cases.

- A **scene** family has one generated source and items made from it by an op. Under source
  fidelity the job carries the source and one requirement per protected subject: `grade` (a mild
  colour grade: a pass), `restyle` (the generator makes a permitted change such as light, season
  or dressing: a pass the owner confirms kept the structure), `mirror`, `rotate`, `crop`, and the
  small camera changes `tilt` (a roll of 0.5 to 10 degrees), `zoom` (1.03 to 1.5 times) and
  `keystone` (a horizontal keystone of 1 to 15%), each refilled to the frame at the source's size
  (the structural rule, a fail over the whole image), and `edit` (the generator removes, adds or
  changes a protected subject, named by `removes`, `adds` or `alters`: the identity rule, a fail
  whose region is the densest patch of pixel
  difference; any other dense patch the generator changed unasked is reported as a secondary
  change for the reviewer). Any op may also carry delivery damage, `jpeg` (a re-save at that
  quality) and `resample` (scaled by that factor and back), applied after it without changing
  its label. Under reality checks the job is the candidate alone: `as_is` (the
  clean image, a pass), `restyle` (a clean variant, a pass), `edit` with a `defect` of
  `anatomy`, `shadow`, `text` or `merge`, which plants that defect by editing the clean image and
  labels RE1 to RE4, and `generate` with a `defect`, which draws the candidate from its own
  prompt with the defect asked for. The generators follow a text prompt asking for six fingers
  or a shadow the wrong way but quietly ignore an edit instruction to add one, so anatomy,
  merge and shadow defects are generated; garbled text works either way.
- A **subject** family has one to four generated references of one subject and three item
  forms: a `scene` composed from the references (a pass whose identity the owner confirms), an
  item built `from` an earlier candidate by an `edit` that changes a marking, and a `scene` with
  a `lookalike` description generated without the references. Under reference consistency the
  edit labels RC2 and the lookalike RC3 and RC1; under product fidelity the edit's `aspect`
  (`label`, `colour`, `shape`) labels PF2, PF3 or PF1 and the lookalike PF1 and PF2.
- A **brief** family (brief compliance) is a brief with what it `asks` for, what it `forbids`
  and the exact `text` it specifies, each ask and forbid becoming a job requirement. Every item
  is `generate` with a seed: from the brief as written (a pass), or from an altered prompt that
  `violates` BC1 (something asked for missing), BC2 (something forbidden present) or BC3 (the
  text misspelt), naming what it is `about`.

The draft label comes from the construction alone; nobody looks at the picture to write it, and
no judge or tool runs. The plans of the dev sets are under `fixtures/sets/` (and double as the
offline self-test with the fake generator); the sets themselves are under `evalsets/` and
`evalsets/README.md` tabulates every published set.

```
cd scripts/sets
uv run squint-sets build --plan <plan.toml> --out <owner storage>/build/evalsets/<set id> --owner <owner storage>
# review <owner storage>/<set id>/contact-<family>.png and labels-draft.toml:
# set each label's status to "confirmed" or "struck"
cd ../.. && scripts/sets-publish.sh <set id> "<attestation>"
```

`rebuild --item <id> [--seed-offset N]` regenerates one item from the family's images already on
disk, with the plan as it now reads (a reworded edit, a new seed offset), and puts only that
item's label back to draft; this is the review loop for an item the owner struck.
`scripts/sets-publish.sh <id> "<attestation>"` runs the publishing steps for a reviewed set:
confirms the drafts with the date, promotes, commits the images into the evalsets clone beside
the harness, pins that commit in the manifest, places the set under `evalsets/`, seals it with the
owner's attestation and checks it; pushing the clone and committing here follow.
`build` writes the public set directory (manifest, families, one job per item with pinned
images, the family's source stored once under `families/`) and, to owner storage, the
construction record (generator, model, prompt, seed, every edit instruction, hashes, dates), the
draft labels and a contact sheet per family. `promote` publishes the confirmed labels as
`labels.toml` and removes struck items from the set; a draft still unreviewed is refused. The
owner then pushes the images to the `wafflery-evalsets` repository and commits the manifest,
jobs and labels here. `just sets-check` lints and tests the toolkit and runs the offline dry run
through `squint sets check`; the live dry run of 2026-10-27 is recorded in ADR 0013.

**Blind sets** (task 5.5) differ in two ways. The plan lives in owner storage, never in a
repository, because an item's construction gives away its label: `source-fidelity/blind-v1` was
drawn by a seeded script that fixed five new scene families (disjoint from every dev and
adversarial family) and chose each item's construction, structural op and generator from the
seed, writing the plan without showing it to the agent that built the set (the script and the
plan sit beside the worksheet in owner storage, `<owner>/source-fidelity/blind-v1/`); the owner
reviews the contact sheets and confirms the labels as for a dev set. And the publish step seals with
`--labels-out`, which moves `labels.toml` to `<owner>/<set id>/` and leaves `labels_sha256` in the
manifest; the labels are given to `eval gate` from there and published by the gate's publication
step. No judge or tool runs on a blind item before its gate.

Credentials for the generators come from the environment only (`FAL_KEY`, `OPENAI_API_KEY`,
`GEMINI_API_KEY`); the toolkit never writes them anywhere. Owner storage is a directory outside
both repositories; nothing in it is ever committed.
