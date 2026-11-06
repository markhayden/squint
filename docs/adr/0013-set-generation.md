# 0013: Public set generation: three generators mixed by family, defects by construction, CC-BY-4.0

Date: 2026-10-27. Status: accepted.

## Context

The public evaluation sets (SPEC "Evaluation", ADR 0007) need images nobody holds rights over
and whose answers are known before any judge sees them. Owner decision 11 rules out everything
from the prototype. On 2026-10-27 one image was generated from the same prompt (a red-brick house,
green door, blue bicycle, maple tree) with each candidate generator and compared by the owner:

- FLUX.2 [klein] 4B on fal (`fal-ai/flux-2-klein/4b`): all elements, snapshot realism, 0.4 s,
  reproducible by seed. Weights are Apache 2.0 (Black Forest Labs' help centre lists klein 4B as
  fully open); fal's model page tags it "Commercial use" and fal's terms (last updated
  2026-09-08) claim no intellectual property in output content and place its use at the
  customer's risk. The edit endpoint is `fal-ai/flux-2/klein/4b/edit` with `image_urls`
  (fal's klein user guide); price is per megapixel, roughly $0.012 to $0.018 per 1024×1024 image
  according to fal's guide and aggregators. fal's pricing page did not list klein on the day; the
  dry run records the billed amount.
- OpenAI gpt-image-2 (`/v1/images/generations`, `/v1/images/edits`): all elements, documentary
  and simple, no seed. OpenAI's terms assign output rights to the user and allow commercial use;
  the agent could not fetch the terms page (HTTP 403) and the owner confirms the clause from the
  page. Usage for the sample: 1,756 output image tokens at medium quality.
- Gemini 3.1 Flash Image (`gemini-3.1-flash-image:generateContent`): all elements plus unrequested
  scenery, no seed. The Gemini API additional terms (last updated 2026-04-28) say Google claims
  no ownership of generated content, may generate similar content for others, and on paid
  services does not use prompts or responses to improve its products. Usage for the sample: 1,120
  output image tokens.

Every generator can reproduce a scene well enough; the fail items of a set are edits, so editing
fidelity (changing only what was asked) matters more and is measured in the dry run. A set made by
one generator carries that generator's fingerprint, which a judge could learn in place of the
defect.

## Decision

1. **Three generators, mixed by family.** FLUX.2 [klein] 4B via fal, OpenAI gpt-image-2 and
   Gemini 3.1 Flash Image. Each family (one scene or subject) uses one generator for its source,
   its edits and its candidates, so the items of a family differ only in the planted change.
   Families are spread across the three so no set is one vendor's style; the generator, model id,
   prompt, seed where the API has one, and every edit instruction are recorded per item in the
   construction record.
2. **Defects by construction, labels from the record.** Pass items are permitted changes (a colour
   grade, a restyle, a new scene for a reference subject). Fail items are either deterministic
   transforms of the source (rotate, mirror, crop: the structural rules) or an edit that removes
   or alters a protected subject (the identity rules). The draft label and its region come from
   the construction record only: a transform labels the whole image; an edit's region is the
   bounding box of the pixel difference between source and candidate. No judge or tool runs on an
   item before the owner adjudicates it.
3. **Owner review.** The toolkit writes `labels-draft.toml`, the construction record and a contact
   sheet per family to owner storage outside both repositories (decision 10: batches of 20 to 25,
   the owner confirms or strikes each item; struck items are logged and discarded, never
   relabelled). Confirmed labels are what ship.
4. **Licence.** Published sets, images and labels are CC-BY-4.0, with the generator named per
   item in the manifest's construction notes and in the evalsets repository README. The weights
   licence (Apache 2.0 for klein) and the vendors' terms above permit this for all three; the
   OpenAI clause is confirmed by the owner from the terms page.
5. **Where things live.** Images in the public `wafflery-evalsets` repository (named
   `squint-evalsets` until 2026-11-05, when it was renamed to hold rascal's sets too), cloned
   beside the harness at `../wafflery-evalsets`; manifests, jobs and confirmed labels in this repository under
   `evalsets/`; construction records, drafts, contact sheets and blind labels in owner storage.

## Dry run (2026-10-27)

`fixtures/sets/houses-v1.plan.toml` built live with FLUX.2 klein on fal: the house source (seed
1234, reproducing the sample above exactly), a colour-graded pass item, a mirrored fail item and
an edit "remove the blue bicycle; change nothing else". The edit removed the bicycle cleanly and
left the door, tree, windows and framing in place. Measured against the source, the drafted
region landed on the bicycle (x 0.19 to 0.45, y 0.78 to 0.97 of the frame) and the toolkit
reported one secondary change, a strip along the top edge (roofline and sky, y 0 to 0.13) the
generator altered unasked, with sparse pixel noise over the rest of the image. That is the drift
the owner's review exists to catch; the draft label names it so the reviewer looks there. The
whole run cost a few cents. `squint sets check` accepted the built set after every draft label
was confirmed. The same plan runs offline with the fake generator as the toolkit's self-test.

## Alternatives

- One generator (klein alone): cheapest and reproducible, but one fingerprint across every set.
- The owner's private generation service: excluded by decision 11 and by the public licence.
- Real images from the owner's pipeline: wanted, and added over time as labelled real failures;
  they cannot be the first sets because labelling them is slow and they are not public.

## Consequences

- Three vendor credentials in the environment for set building (fal, OpenAI, Google), each under
  the standing $25 cap (decision 5); a dev set of 20 items costs well under a dollar.
- Reproducibility is by seed on klein and by recorded prompt and date on the other two; the
  construction record is the provenance either way.
- Every results document states that planted defects are cleaner than real failures.
