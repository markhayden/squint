# Writing and extending rules

Rules and job requirements share one model, the **obligation** (`SPEC.md`, "Obligations"). This
page is the practical version. Code: `crates/squint-core/src/obligations.rs`; shipped sets:
`crates/squint-core/rules/`, embedded in the binary at build time.

## An obligation

A rules file is a `[rules]` header naming its use case and version, then its obligations. Every
field of one obligation:

```toml
[rules]
use_case = "source-fidelity"   # the base this file applies to; it must match the config's base
version = 1                    # yours to bump when the file changes (part of the cache key)

[[rule]]
id = "SF1"                 # two capital letters and a number; job requirements use RQ1, RQ2, …
kind = "must_hold"         # must_hold | must_not (mandatory) | allowed (exemption) | observe
text = "Permanent structure keeps its shape, position and extent relative to the source."
scope = ["candidate"]      # images judged: roles or image ids; omitted means the candidate
evidence = ["source"]      # images consulted: roles or ids; omitted means every source and reference
evidence_kinds = ["change_region"]   # tool evidence kinds whose regions bear on it; omitted = set default
matters = true             # uncertainty only; defaults to true for mandatory kinds
hidden_ok = false          # mandatory only: `hidden` is recorded instead of unsure
```

Exemptions (`kind = "allowed"`) add `waives = ["SF1"]` and a `condition`. An exemption with
`waives = []` only states what may change; it never waives anything and is never named in
`allowed_by`: a permitted change that leaves an obligation intact is `held`. Observations
(`kind = "observe"`) never affect the verdict; the model reports them as `observed` findings.

## Layering

A config's `[rules]` section names a shipped base and optionally a shipped profile and your own
rules files (a relative path resolves against the directory squint runs from):

```toml
[rules]
base = "source-fidelity"
profile = "architecture"
extra = ["./rules/mine.toml"]
```

Each file in `extra` is a rules file with its own `[rules]` header (`use_case` and `version`;
without it loading fails with "missing field `rules`"). This `rules/mine.toml` replaces one base
rule, removes another and adds one:

```toml
[rules]
use_case = "source-fidelity"
version = 1

# The same id replaces the base's SF2 (every field is the new one's).
[[rule]]
id = "SF2"
kind = "must_hold"
scope = ["candidate"]
evidence = ["source"]
text = "The candidate keeps the source's framing; a crop of up to a tenth on any side is fine."

# `disabled = true` removes SF4 (`id` and `kind` stay; nothing else is needed).
[[rule]]
id = "SF4"
kind = "observe"
disabled = true

# A new id adds a rule.
[[rule]]
id = "MY1"
kind = "must_not"
text = "No watermark or signature appears anywhere in the candidate."
```

Merge order: base, then the profile (it `extends` the base), then each consumer file in order,
then the job's `[[job.requirements]]` (or, on the command line, `--must-hold`, `--must-not` and
`--allow`, which become those requirements: `RQ1`, `RQ2`, … over every `--must-hold`, then every
`--must-not`, then every `--allow`; a job file that has its own requirements refuses them). The
same `id` replaces the earlier obligation and takes the new provenance; `disabled = true` removes
it; a new id appends. Without a `[rules]` section the base is the job's use case.

`squint rules list` names the shipped bases; `squint rules list <use-case>` prints one base's
rules with their kind, scope and evidence; `squint rules init <use-case> [--out <dir>]` writes a
starter `squint.toml` and a `rules.toml` with this header and every base rule commented out,
ready to edit. Every TOML block on this page is loaded by a test
(`crates/squint-core/tests/it/docs_rules.rs`), so the examples stay copy-paste correct.

## What is checked at load

Unique ids and id format; every `waives` target exists and is mandatory; an exemption that waives
has a condition; no `waives` or `condition` outside exemptions; `hidden_ok` only on mandatory
kinds; no explicit empty `scope`; every scope and evidence entry is a role or an image id in the
job; the job's use case matches the rules'; the set's required roles are present; `[hints.<tool>]`
for a tool that is not enabled is dropped with a warning, and so is a `[screen]` tool that is not
enabled (silently when a shipped profile names it).

## Screening

`[screen]` names tools the harness runs on the job's images before the model's first turn, each
after the tools it requires and with the arguments its descriptions default to ("usually the
aligned candidate"). Their results open the conversation, with their images, as if the model had
called them, and their regions are on the checklist it answers; a refusal (align cannot register
a restyle) is shown as one. The screen spends none of the model's tool budget. The architecture
profile screens with `outline` and `regions`, because models left both uncalled in every run of
the baseline. A later layer's `[screen]` replaces an earlier one's, and `tools = []` turns it off:

```toml
[rules]
use_case = "source-fidelity"
version = 1

[screen]
tools = ["regions"]
```

The report's `rules.screen` names the tools that ran.

## Resolution

Each mandatory obligation becomes one **instance per target image** (`SF1@candidate`), and each
instance is one checklist item the model must answer. A mandatory obligation that reaches no
image is an error (disable it instead). A job with no mandatory instance is an error. A reference
image that no mandatory obligation consults is an error.

## Shipped sets

| Base | Requires | Default evidence kinds |
|---|---|---|
| `source-fidelity` (+ profile `architecture`) | candidate, source | change_region, class_difference, alignment_failure |
| `reference-consistency` | candidate, reference | subject_match, class_difference |
| `reality-checks` | candidate | anatomy, text_region |
| `brief-compliance` | candidate | text_region, class_difference |
| `product-fidelity` | candidate, reference | subject_match, text_region, change_region |

Inspect the merged result for a config and job with `squint rules show --config … --job …`, and
the rendered Obligations section with `squint check … --dry-run`.
