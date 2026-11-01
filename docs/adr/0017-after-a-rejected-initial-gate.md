# ADR 0017: After a rejected initial gate

Date: 2026-10-29. Status: accepted 2026-10-30 (option 1; owner decision).

## Context

The initial gate of `regions` for source fidelity (task 5.6, receipt `8e0c8307a72475ac`) was
**rejected**: under gemini-4-argon on the sealed blind set `source-fidelity/blind-v1` both arm C
and arm D_regions detected all ten planted failures, so the frozen estimand, the paired
difference in detected failures, was zero with an interval of zero to zero and no differing
family. The tool took back the one false reject of arm C and lifted localisation from 9 to 10 of
10, but neither is scored by the gate.

M6 as planned (PLAN tasks 6.1 and 6.2) cannot proceed: the confirmation stage scores a tool only
on top of an admitted initial receipt (SPEC "Stages"). Every way forward either tests the same
tool again, changes what a gate scores, or moves to another tool, and each touches the rule that a
gate's question is fixed before the labels are seen. That rule exists so that a tool cannot be
tested until it passes; this ADR records how the project goes on without weakening it.

What the rejection says is narrower than "the tool does not help". The blind set saturated the
control: whole named subjects removed from a scene are easy for this model to see without tools.
A detection gate can only separate arms on items the control sometimes misses.

## Options

1. **A harder initial set, same estimand.** Build a new blind set designed to be hard for the
   control (small removals, one subject among several alike, partial changes such as a window
   bricked up, removals under a strong restyle, changes near the edges of a panorama), score a
   new initial gate of `regions` on it with the same frozen configuration, and count the earlier
   rejection in the multiplicity: two initial attempts for the same tool and use case means each
   is scored at Bonferroni-adjusted percentiles for two hypotheses. The rejected receipt stays
   published. Cost: a set of at least 20 items (owner review time, a few dollars of generation),
   a gate run (about $1.50 under Argon).
2. **A different estimand.** Add a second scored question to the gate, for example "false
   rejects lower than C with detection not lower", or a localisation gate, so a tool that helps
   the model avoid errors on clean images or point at the right place can be admitted. Frozen for
   the next gate only, never applied to the scored blind set. Changes SPEC "Gates" and the
   statistics contract (ADR 0016's estimand); needs its own power check, since false rejects on
   ten clean items move by one or two.
3. **Record and move on.** Keep `regions` out of arm D for source fidelity, publish the result as
   it stands (done), and spend the next milestone on another tool or use case where the control is
   weaker, such as reality checks (anatomy, text, shadows), where the baseline showed the most
   variation between models.

## Decision

The owner chose option 1 on 2026-10-30, as recommended below.

## Recommendation

Option 1 first, with option 2 recorded as a question for the next gate rather than adopted now.

The arm C run and this gate both point at the same cause, sets the control saturates, and a harder
set is needed by every path: option 2 needs it to have any power, and option 3's next tool will hit
the same ceiling on the current sets. The adjusted multiplicity keeps the retest honest. The
harder set is built blind as blind-v1 was, from a seeded plan in owner storage, with opaque item
ids, and serves as the new initial set for `regions` (25 items in at least 5 new families); the
confirmation set of PLAN 6.1 follows only if that gate admits.

## Consequences

- PLAN M6 is re-scoped: task 6.0 builds the harder blind set and scores the second initial gate;
  6.1 and 6.2 run only after an admission.
- The gate matrix for the second attempt declares `hypotheses = 2` for `regions` on source
  fidelity, and the receipt names this ADR in its `adr` field.
- SPEC "Gates" gains one sentence: a rejected tool may be scored again on a new blind set only
  under an ADR that counts every earlier attempt in the hypothesis count.

## Amendment 2026-11-01

The second initial gate (blind-v2) was rejected the same way: both arms caught 15 of 15. The owner
took M6 out of the plan. `regions` stays an optional tool, off unless a config enables it, and the
question left open, whether it helps a cheaper model that misses failures on its own, is GitHub
issue #3. A later gate still counts both earlier attempts.
