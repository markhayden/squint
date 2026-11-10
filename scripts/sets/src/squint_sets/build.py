"""Build a set from a plan and write two things: the public set directory (manifest, families,
one pinned job per item, the images) and the owner worksheet (construction record, draft labels,
a contact sheet per family) in owner storage. `promote` then publishes the labels the owner
confirmed and drops the items the owner struck.

Scene families (source fidelity) generate one source and derive every item from it by an op.
Subject families (reference consistency) generate the references of one subject, then each item
is a new scene composed from those references (a pass, to be confirmed), an edit of an earlier
candidate that changes a marking (RC2), or a lookalike generated without the references (RC3).
"""

from __future__ import annotations

import datetime as dt
import hashlib
import io
import shutil
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from PIL import Image, ImageDraw

from . import ops, regions, toml_out
from .ops import DraftLabel
from .plan import Family, Item, Plan

SOURCE_FILE = "source.png"
CANDIDATE_FILE = "candidate.png"


@dataclass
class BuildReport:
    items: int = 0
    images: int = 0


@dataclass
class PromoteReport:
    confirmed: int = 0
    struck: int = 0


@dataclass
class _Built:
    """One item as built: its candidate, its job's images beyond the candidate, its draft."""

    id: str
    candidate: bytes
    label: DraftLabel
    brief: str
    record: dict[str, Any]
    #: (image id, role, label, path relative to the set, sha256)
    others: list[tuple[str, str, str | None, str, str]] = field(default_factory=list)
    requirements: list[dict[str, Any]] = field(default_factory=list)


def _sha256(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def _today() -> str:
    return dt.date.today().isoformat()


def _cap(text: str) -> str:
    return text[0].upper() + text[1:] if text else text


def _job_text(
    set_id: str,
    use_case: str,
    brief: str,
    requirements: list[dict[str, Any]],
    images: list[dict[str, Any]],
) -> str:
    tables: list[tuple[str, dict[str, Any], bool]] = [
        ("job", {"use_case": use_case, "brief": brief}, False)
    ]
    tables += [("job.requirements", rq, True) for rq in requirements]
    tables += [("job.images", im, True) for im in images]
    return toml_out.document(
        f"Item of {set_id}; built by construction, see the owner's record.", tables
    )


def _contact_sheet(cells: list[tuple[str, bytes, DraftLabel | None]], path: Path) -> None:
    thumb = 256
    sheet = Image.new(
        "RGB", (thumb * len(cells) + 8 * (len(cells) + 1), thumb + 70), (245, 245, 245)
    )
    d = ImageDraw.Draw(sheet)
    for n, (name, b, lab) in enumerate(cells):
        im = Image.open(io.BytesIO(b)).convert("RGB")
        im.thumbnail((thumb, thumb))
        x = 8 + n * (thumb + 8)
        sheet.paste(im, (x, 8))
        caption = name if lab is None else f"{name}\n{lab.expected}: {lab.words[:48]}"
        d.multiline_text((x, thumb + 14), caption, fill=(20, 20, 20))
    sheet.save(path, format="PNG")


def _match_requirements(
    label: DraftLabel, target: str | None, requirements: list[dict[str, Any]]
) -> None:
    if not target:
        return
    for rq in requirements:
        if target.lower() in rq["text"].lower():
            label.obligation_ids.append(rq["id"])


def _scene_requirements(family: Family) -> list[dict[str, Any]]:
    return [
        {
            "id": f"RQ{n}",
            "kind": "must_hold",
            "scope": ["candidate"],
            "evidence": ["source"],
            "text": f"{_cap(subject)} keeps its identity, shape, position and extent relative to the source.",
        }
        for n, subject in enumerate(family.protected, start=1)
    ]


def _item_generator(family: Family, item: Item, gen: Any, generators: dict[str, Any] | None) -> Any:
    """The generator for `item`'s candidate: its own when the plan names one, else the family's."""
    if item.generator is None or item.generator == family.generator:
        return gen
    g = (generators or {}).get(item.generator)
    if g is None:
        raise ValueError(f"item {item.id}: no generator {item.generator!r} available")
    return g


def _build_scene_family(
    family: Family,
    gen: Any,
    out: Path,
    use_case: str = "source-fidelity",
    generators: dict[str, Any] | None = None,
) -> tuple[list[_Built], dict[str, Any], list[tuple[str, bytes, DraftLabel | None]]]:
    assert family.source is not None
    # Only source fidelity judges against the source; a reality-check job is the candidate alone.
    with_source = use_case == "source-fidelity"
    source = gen.generate(family.source.prompt, family.source.seed, family.source.size)
    source_rel = f"families/{family.id}/{SOURCE_FILE}"
    (out / source_rel).parent.mkdir(parents=True, exist_ok=True)
    (out / source_rel).write_bytes(source)
    source_sha = _sha256(source)
    requirements = _scene_requirements(family) if with_source else []
    record = {
        "id": family.id,
        "kind": "scene",
        "generator": family.generator,
        "model": getattr(gen, "model", family.generator),
        "prompt": family.source.prompt,
        "seed": family.source.seed,
        "size": family.source.size,
        "source_sha256": source_sha,
        "generated_on": _today(),
        "protected": list(family.protected),
    }
    built: list[_Built] = []
    cells: list[tuple[str, bytes, DraftLabel | None]] = [("source", source, None)]
    for n, item in enumerate(family.items):
        assert item.op is not None
        item_gen = _item_generator(family, item, gen, generators)
        candidate, label = ops.apply(item.op, source, item_gen, family.source.seed + n + 1)
        if label.expected == "fail" and label.obligation_ids == ["SF1"]:
            _match_requirements(
                label, item.op.get("removes") or item.op.get("alters"), requirements
            )
        rec = {"id": item.id, "family": family.id, "op": dict(item.op), "generated_on": _today()}
        if item.generator is not None:
            rec["generator"] = item.generator
        if label.notes:
            rec["edit_notes"] = label.notes
        built.append(
            _Built(
                id=item.id,
                candidate=candidate,
                label=label,
                brief=family.brief,
                record=rec,
                others=[("source", "source", None, source_rel, source_sha)] if with_source else [],
                requirements=requirements,
            )
        )
        cells.append((item.id, candidate, label))
    return built, record, cells


def _brief_requirements(family: Family) -> list[dict[str, Any]]:
    reqs = [
        {
            "id": f"RQ{n}",
            "kind": "must_hold",
            "scope": ["candidate"],
            "text": f"The candidate shows {ask}.",
        }
        for n, ask in enumerate(family.asks, start=1)
    ]
    for n, forbid in enumerate(family.forbids, start=len(reqs) + 1):
        reqs.append(
            {
                "id": f"RQ{n}",
                "kind": "must_not",
                "scope": ["candidate"],
                "text": f"The candidate shows {forbid}; none was asked for.",
            }
        )
    return reqs


def _brief_item(
    family: Family, item: Item, gen: Any, requirements: list[dict[str, Any]], seed_offset: int = 0
) -> _Built:
    assert item.op is not None
    prompt = item.op.get("prompt") or family.brief
    seed = int(item.op["seed"]) + seed_offset
    candidate = gen.generate(prompt, seed, int(item.op.get("size", 1024)))
    violates = item.op.get("violates")
    about = item.op.get("about")
    if violates is None:
        label = DraftLabel(
            expected="pass",
            failure_type=None,
            obligation_ids=[],
            words="generated from the brief as written; confirm everything asked for is present, as asked, and nothing forbidden",
            notes={"prompt": prompt},
        )
    else:
        label = DraftLabel(
            expected="fail",
            failure_type={
                "BC1": "asked-missing",
                "BC2": "forbidden-present",
                "BC3": "text-misspelt",
            }[violates],
            obligation_ids=[violates],
            words={
                "BC1": f"the brief asked for {about} and the candidate lacks it",
                "BC2": f"the brief forbade {about} and the candidate shows it",
                "BC3": f"the text {about!r} is not spelled as the brief gives it",
            }[violates],
            region_kind="whole_image",
            notes={"prompt": prompt},
        )
        if violates in ("BC1", "BC2") and about:
            for rq in requirements:
                if about.lower() in rq["text"].lower():
                    label.obligation_ids.append(rq["id"])
    rec = {
        "id": item.id,
        "family": family.id,
        "op": dict(item.op),
        "prompt": prompt,
        "generated_on": _today(),
    }
    if seed_offset:
        rec["seed_offset"] = seed_offset
    return _Built(
        id=item.id,
        candidate=candidate,
        label=label,
        brief=family.brief,
        record=rec,
        others=[],
        requirements=requirements,
    )


def _build_brief_family(
    family: Family, gen: Any
) -> tuple[list[_Built], dict[str, Any], list[tuple[str, bytes, DraftLabel | None]]]:
    requirements = _brief_requirements(family)
    record = {
        "id": family.id,
        "kind": "brief",
        "generator": family.generator,
        "model": getattr(gen, "model", family.generator),
        "brief": family.brief,
        "asks": list(family.asks),
        "forbids": list(family.forbids),
        "text": list(family.text),
        "generated_on": _today(),
    }
    built = [_brief_item(family, item, gen, requirements) for item in family.items]
    cells: list[tuple[str, bytes, DraftLabel | None]] = [
        (b.id, b.candidate, b.label) for b in built
    ]
    return built, record, cells


def _subject_rules(use_case: str, aspect: str | None) -> tuple[list[str], list[str]]:
    """(rules a marking edit breaks, rules a lookalike breaks) for the use case."""
    if use_case == "product-fidelity":
        edit = {"label": "PF2", "colour": "PF3", "shape": "PF1"}.get(aspect or "label", "PF2")
        return [edit], ["PF1", "PF2"]
    return ["RC2"], ["RC3", "RC1"]


def _build_subject_family(
    family: Family, gen: Any, out: Path, use_case: str = "reference-consistency"
) -> tuple[list[_Built], dict[str, Any], list[tuple[str, bytes, DraftLabel | None]]]:
    refs: list[tuple[str, bytes, str, str]] = []  # (image id, bytes, rel path, sha)
    ref_records = []
    for n, ref in enumerate(family.references, start=1):
        image = gen.generate(ref.prompt, ref.seed, ref.size)
        rel = f"families/{family.id}/ref-{n}.png"
        (out / rel).parent.mkdir(parents=True, exist_ok=True)
        (out / rel).write_bytes(image)
        sha = _sha256(image)
        refs.append((f"ref:{family.id}-{n}", image, rel, sha))
        ref_records.append(
            {"prompt": ref.prompt, "seed": ref.seed, "size": ref.size, "sha256": sha}
        )
    ref_ids = [r[0] for r in refs]
    requirements = _subject_requirements(family, ref_ids, use_case)
    record = {
        "id": family.id,
        "kind": "subject",
        "generator": family.generator,
        "model": getattr(gen, "model", family.generator),
        "subject": family.subject,
        "references": ref_records,
        "generated_on": _today(),
        "protected": list(family.protected),
    }
    others = [(rid, "reference", family.description, rel, sha) for rid, _, rel, sha in refs]
    ref_bytes = [r[1] for r in refs]
    built: list[_Built] = []
    by_id: dict[str, bytes] = {}
    cells: list[tuple[str, bytes, DraftLabel | None]] = [(rid, b, None) for rid, b, _, _ in refs]
    for item in family.items:
        b = _subject_item(
            family, item, gen, ref_bytes, by_id, others, requirements, use_case=use_case
        )
        built.append(b)
        by_id[item.id] = b.candidate
        cells.append((item.id, b.candidate, b.label))
    return built, record, cells


def _subject_requirements(
    family: Family, ref_ids: list[str], use_case: str = "reference-consistency"
) -> list[dict[str, Any]]:
    text = (
        "{s} matches the reference exactly."
        if use_case == "product-fidelity"
        else "{s} is present and unchanged in the candidate, as in the references."
    )
    return [
        {
            "id": f"RQ{n}",
            "kind": "must_hold",
            "scope": ["candidate"],
            "evidence": list(ref_ids),
            "text": text.format(s=_cap(subject)),
        }
        for n, subject in enumerate(family.protected, start=1)
    ]


def _subject_item(
    family: Family,
    item: Item,
    gen: Any,
    ref_bytes: list[bytes],
    by_id: dict[str, bytes],
    others: list[tuple[str, str, str | None, str, str]],
    requirements: list[dict[str, Any]],
    seed_offset: int = 0,
    use_case: str = "reference-consistency",
) -> _Built:
    """One subject-family item from the family's references and, for a from item, the base
    candidate; `seed_offset` varies the seed on a rebuild."""
    briefs = {i.id: i.scene.prompt for i in family.items if i.scene is not None}
    if item.from_item is not None:
        assert item.op is not None
        base = by_id[item.from_item]
        base_seed = next(
            (i.scene.seed for i in family.items if i.id == item.from_item and i.scene), 0
        )
        candidate = gen.edit(base, item.op["prompt"], base_seed + 1 + seed_offset)
        alters = item.op.get("alters") or item.op.get("removes")
        changes = regions.difference_regions(base, candidate)
        label = DraftLabel(
            expected="fail",
            failure_type=f"{alters}-changed" if alters else "edited",
            obligation_ids=list(_subject_rules(use_case, item.op.get("aspect"))[0]),
            words=f"the {alters} is changed in the candidate, unlike the references"
            if alters
            else f"edited: {item.op['prompt']}",
            notes={"prompt": item.op["prompt"], "from": item.from_item},
        )
        _match_requirements(label, alters, requirements)
        if changes.primary is None:
            label.region_kind = "whole_image"
        else:
            label.region_kind = "bbox"
            label.bbox = list(changes.primary)
        if changes.secondary:
            label.notes["secondary_changes"] = changes.secondary
            label.words += (
                f"; the generator also changed {len(changes.secondary)} other area(s), check them"
            )
        brief = briefs.get(item.from_item, "")
        rec = {
            "id": item.id,
            "family": family.id,
            "from": item.from_item,
            "op": dict(item.op),
            "generated_on": _today(),
            "edit_notes": label.notes,
        }
    elif item.lookalike is not None:
        assert item.scene is not None
        prompt = f"{item.scene.prompt} The subject is {item.lookalike}."
        candidate = gen.generate(prompt, item.scene.seed + seed_offset, item.scene.size)
        label = DraftLabel(
            expected="fail",
            failure_type="lookalike",
            obligation_ids=list(_subject_rules(use_case, None)[1]),
            words=f"a lookalike, not the reference subject: {item.lookalike}",
            region_kind="whole_image",
            notes={"prompt": prompt},
        )
        brief = item.scene.prompt
        rec = {
            "id": item.id,
            "family": family.id,
            "scene": {"prompt": item.scene.prompt, "seed": item.scene.seed},
            "lookalike": item.lookalike,
            "generated_on": _today(),
        }
    else:
        assert item.scene is not None
        prompt = f"{item.scene.prompt} The subject is exactly the one in the reference image(s): {family.subject}; keep every marking, colour and feature."
        candidate = gen.compose(ref_bytes, prompt, item.scene.seed + seed_offset)
        label = DraftLabel(
            expected="pass",
            failure_type=None,
            obligation_ids=[],
            words="the subject placed in a new scene from the references; confirm identity, markings and features against the references before accepting",
            notes={"prompt": prompt},
        )
        brief = item.scene.prompt
        rec = {
            "id": item.id,
            "family": family.id,
            "scene": {"prompt": item.scene.prompt, "seed": item.scene.seed},
            "generated_on": _today(),
        }
    if seed_offset:
        rec["seed_offset"] = seed_offset
    return _Built(
        id=item.id,
        candidate=candidate,
        label=label,
        brief=brief,
        record=rec,
        others=list(others),
        requirements=requirements,
    )


def _write_item(
    set_id: str, use_case: str, out: Path, b: _Built
) -> tuple[str, dict[str, Any], list[tuple[str, dict[str, Any], bool]]]:
    """Writes one item's candidate and job; returns its sha, its record entry and its draft tables."""
    item_dir = out / "items" / b.id
    item_dir.mkdir(parents=True, exist_ok=True)
    candidate_rel = f"items/{b.id}/{CANDIDATE_FILE}"
    (out / candidate_rel).write_bytes(b.candidate)
    candidate_sha = _sha256(b.candidate)
    images = [
        {
            "id": "candidate",
            "role": "candidate",
            "source": {"path": candidate_rel, "sha256": candidate_sha},
        }
    ]
    for image_id, role, label, rel, sha in b.others:
        entry: dict[str, Any] = {"id": image_id, "role": role}
        if label:
            entry["label"] = label
        entry["source"] = {"path": rel, "sha256": sha}
        images.append(entry)
    (item_dir / "job.toml").write_text(_job_text(set_id, use_case, b.brief, b.requirements, images))
    fields: dict[str, Any] = {
        "item": b.id,
        "expected": b.label.expected,
        "failure_types": [b.label.failure_type] if b.label.failure_type else [],
        "words": b.label.words,
        "obligation_ids": list(b.label.obligation_ids),
        "adjudicated_by": "owner",
        "adjudicated_on": "",
        "status": "draft",
    }
    drafts: list[tuple[str, dict[str, Any], bool]] = [("label", fields, True)]
    if b.label.region_kind:
        region: dict[str, Any] = {
            "frame": "raw:candidate",
            "kind": b.label.region_kind,
            "defect": b.label.failure_type or "changed",
            "obligation_ids": list(b.label.obligation_ids),
        }
        if b.label.bbox:
            region["bbox"] = b.label.bbox
        drafts.append(("label.regions", region, True))
    return candidate_sha, {**b.record, "candidate_sha256": candidate_sha}, drafts


def _label_tables(labels: list[dict[str, Any]]) -> list[tuple[str, dict[str, Any], bool]]:
    tables: list[tuple[str, dict[str, Any], bool]] = []
    for label in labels:
        label = dict(label)
        regs = label.pop("regions", [])
        tables.append(("label", label, True))
        for r in regs:
            tables.append(("label.regions", r, True))
    return tables


DRAFT_COMMENT = (
    'Draft labels from the construction record. Set each label\'s status to "confirmed" or "struck"\n'
    "and adjudicated_on to the date, then run `squint-sets promote`."
)


def rebuild(
    plan: Plan,
    out: Path,
    owner: Path,
    generators: dict[str, Any],
    item_id: str,
    seed_offset: int = 0,
) -> BuildReport:
    """Regenerates one item from the family's images already on disk (references or source, and
    the base candidate of a from item), with the plan as it now reads and an optional seed
    offset; rewrites that item's job, record entry, draft label (back to draft) and the family's
    contact sheet, and nothing else."""
    out, owner_dir = Path(out), Path(owner) / plan.set.id
    family = next((f for f in plan.families for i in f.items if i.id == item_id), None)
    if family is None:
        raise ValueError(f"no item {item_id!r} in the plan")
    item = next(i for i in family.items if i.id == item_id)
    gen = generators.get(family.generator)
    if gen is None:
        raise ValueError(f"family {family.id}: no generator {family.generator!r} available")
    if family.kind == "subject":
        refs = []
        for n in range(1, len(family.references) + 1):
            rel = f"families/{family.id}/ref-{n}.png"
            data = (out / rel).read_bytes()
            refs.append((f"ref:{family.id}-{n}", data, rel, _sha256(data)))
        others = [(rid, "reference", family.description, rel, sha) for rid, _, rel, sha in refs]
        requirements = _subject_requirements(family, [r[0] for r in refs], plan.set.use_case)
        by_id = {}
        if item.from_item is not None:
            by_id[item.from_item] = (out / "items" / item.from_item / CANDIDATE_FILE).read_bytes()
        b = _subject_item(
            family,
            item,
            gen,
            [r[1] for r in refs],
            by_id,
            others,
            requirements,
            seed_offset,
            plan.set.use_case,
        )
        cells: list[tuple[str, bytes, DraftLabel | None]] = [
            (rid, data, None) for rid, data, _, _ in refs
        ]
    elif family.kind == "brief":
        b = _brief_item(family, item, gen, _brief_requirements(family), seed_offset)
        cells = []
    else:
        assert family.source is not None and item.op is not None
        with_source = plan.set.use_case == "source-fidelity"
        source_rel = f"families/{family.id}/{SOURCE_FILE}"
        source = (out / source_rel).read_bytes()
        requirements = _scene_requirements(family) if with_source else []
        n = family.items.index(item)
        item_gen = _item_generator(family, item, gen, generators)
        candidate, label = ops.apply(
            item.op, source, item_gen, family.source.seed + n + 1 + seed_offset
        )
        if label.expected == "fail" and label.obligation_ids == ["SF1"]:
            _match_requirements(
                label, item.op.get("removes") or item.op.get("alters"), requirements
            )
        rec = {"id": item.id, "family": family.id, "op": dict(item.op), "generated_on": _today()}
        if item.generator is not None:
            rec["generator"] = item.generator
        if label.notes:
            rec["edit_notes"] = label.notes
        if seed_offset:
            rec["seed_offset"] = seed_offset
        b = _Built(
            id=item.id,
            candidate=candidate,
            label=label,
            brief=family.brief,
            record=rec,
            others=[("source", "source", None, source_rel, _sha256(source))] if with_source else [],
            requirements=requirements,
        )
        cells = [("source", source, None)]
    _, entry, drafts = _write_item(plan.set.id, plan.set.use_case, out, b)
    # The family's entries in the record, the draft and the manifest follow the plan's item list:
    # the rebuilt item replaced, a new id inserted at its place, an id the plan lost dropped;
    # other families untouched.
    record = tomllib.loads((owner_dir / "construction.toml").read_text())
    plan_ids = [i.id for i in family.items]
    family_ids = {i["id"] for i in record.get("item", []) if i.get("family") == family.id} | set(
        plan_ids
    )

    def resync(
        entries: list[dict[str, Any]], key: str, fresh: dict[str, Any]
    ) -> list[dict[str, Any]]:
        own = {e[key]: e for e in entries if e[key] in family_ids}
        own[item_id] = fresh
        block = [own[i] for i in plan_ids if i in own]
        merged: list[dict[str, Any]] = []
        placed = False
        for e in entries:
            if e[key] in family_ids:
                if not placed:
                    merged.extend(block)
                    placed = True
            else:
                merged.append(e)
        if not placed:
            merged.extend(block)
        return merged

    items = resync(record.get("item", []), "id", entry)
    (owner_dir / "construction.toml").write_text(
        toml_out.document(
            f"Construction record of {plan.set.id}: owner material, never published.",
            [("set", record["set"], False)]
            + [("family", f, True) for f in record.get("family", [])]
            + [("item", i, True) for i in items],
        )
    )
    draft = tomllib.loads((owner_dir / "labels-draft.toml").read_text())
    new_label = dict(drafts[0][1])
    if len(drafts) > 1:
        new_label["regions"] = [d[1] for d in drafts[1:]]
    labels = resync(draft.get("label", []), "item", new_label)
    (owner_dir / "labels-draft.toml").write_text(
        toml_out.document(DRAFT_COMMENT, _label_tables(labels))
    )
    manifest = tomllib.loads((out / "set.toml").read_text())
    for i in manifest.get("item", []):
        if i["id"] in family_ids and i["id"] not in plan_ids:
            shutil.rmtree(out / "items" / i["id"], ignore_errors=True)
    manifest_items = resync(
        manifest.get("item", []),
        "id",
        {"id": item_id, "family": family.id, "job": f"items/{item_id}/job.toml"},
    )
    (out / "set.toml").write_text(
        toml_out.document(
            "Built by squint-sets; labels arrive with `promote` after the owner's review.",
            [("set", manifest["set"], False)] + [("item", i, True) for i in manifest_items],
        )
    )
    draft_by_item = {label["item"]: label for label in labels}
    for i in family.items:
        data = (out / "items" / i.id / CANDIDATE_FILE).read_bytes()
        lab = draft_by_item.get(i.id)
        cells.append(
            (
                i.id,
                data,
                DraftLabel(
                    expected=lab["expected"],
                    failure_type=None,
                    obligation_ids=[],
                    words=lab["words"],
                )
                if lab
                else None,
            )
        )
    _contact_sheet(cells, owner_dir / f"contact-{family.id}.png")
    return BuildReport(items=1, images=1)


def build(plan: Plan, out: Path, owner: Path, generators: dict[str, Any]) -> BuildReport:
    out = Path(out)
    owner_dir = Path(owner) / plan.set.id
    out.mkdir(parents=True, exist_ok=True)
    owner_dir.mkdir(parents=True, exist_ok=True)
    report = BuildReport()
    manifest_items: list[dict[str, Any]] = []
    families_out: list[dict[str, Any]] = []
    record_families: list[dict[str, Any]] = []
    record_items: list[dict[str, Any]] = []
    draft_labels: list[tuple[str, dict[str, Any], bool]] = []
    for family in plan.families:
        gen = generators.get(family.generator)
        if gen is None:
            raise ValueError(f"family {family.id}: no generator {family.generator!r} available")
        if family.kind == "subject":
            built, record, cells = _build_subject_family(family, gen, out, plan.set.use_case)
            report.images += len(family.references)
        elif family.kind == "brief":
            built, record, cells = _build_brief_family(family, gen)
        else:
            built, record, cells = _build_scene_family(
                family, gen, out, plan.set.use_case, generators
            )
            report.images += 1
        families_out.append({"id": family.id, "description": family.description})
        record_families.append(record)
        for b in built:
            _, entry, drafts = _write_item(plan.set.id, plan.set.use_case, out, b)
            manifest_items.append(
                {"id": b.id, "family": family.id, "job": f"items/{b.id}/job.toml"}
            )
            record_items.append(entry)
            draft_labels.extend(drafts)
            report.items += 1
            report.images += 1
        _contact_sheet(cells, owner_dir / f"contact-{family.id}.png")
    set_fields: dict[str, Any] = {
        "id": plan.set.id,
        "use_case": plan.set.use_case,
        "split": plan.set.split,
        "licence": plan.set.licence,
        "images_repo": plan.set.images_repo,
        "images_rev": plan.set.images_rev,
        "labels": "labels.toml",
        "families": "families.toml",
        "rules": dict(plan.set.rules),
    }
    manifest = [("set", set_fields, False)] + [("item", i, True) for i in manifest_items]
    (out / "set.toml").write_text(
        toml_out.document(
            "Built by squint-sets; labels arrive with `promote` after the owner's review.",
            manifest,
        )
    )
    (out / "families.toml").write_text(
        toml_out.document(
            "Subject or scene families; a family never appears in more than one split.",
            [("family", f, True) for f in families_out],
        )
    )
    (owner_dir / "construction.toml").write_text(
        toml_out.document(
            f"Construction record of {plan.set.id}: owner material, never published.",
            [("set", {"id": plan.set.id, "built_on": _today()}, False)]
            + [("family", f, True) for f in record_families]
            + [("item", i, True) for i in record_items],
        )
    )
    (owner_dir / "labels-draft.toml").write_text(toml_out.document(DRAFT_COMMENT, draft_labels))
    return report


def promote(draft_path: Path, set_dir: Path) -> PromoteReport:
    """Publishes the confirmed labels as labels.toml and removes struck items from the set."""
    draft = tomllib.loads(Path(draft_path).read_text())
    set_dir = Path(set_dir)
    report = PromoteReport()
    confirmed: list[dict[str, Any]] = []
    struck: set[str] = set()
    for label in draft.get("label", []):
        status = label.get("status", "draft")
        if status == "confirmed":
            if not label.get("adjudicated_on"):
                label["adjudicated_on"] = _today()
            label.pop("status", None)
            confirmed.append(label)
            report.confirmed += 1
        elif status == "struck":
            struck.add(label["item"])
            report.struck += 1
        else:
            raise ValueError(
                f"label for {label.get('item')} is still a draft; confirm or strike every label first"
            )
    manifest = tomllib.loads((set_dir / "set.toml").read_text())
    kept = [i for i in manifest.get("item", []) if i["id"] not in struck]
    for i in manifest.get("item", []):
        if i["id"] in struck:
            shutil.rmtree(set_dir / "items" / i["id"], ignore_errors=True)
    tables: list[tuple[str, dict[str, Any], bool]] = [("set", manifest["set"], False)] + [
        ("item", i, True) for i in kept
    ]
    (set_dir / "set.toml").write_text(
        toml_out.document("Built by squint-sets; labels confirmed by the owner.", tables)
    )
    label_tables: list[tuple[str, dict[str, Any], bool]] = []
    for label in confirmed:
        regs = label.pop("regions", [])
        label_tables.append(("label", label, True))
        for r in regs:
            label_tables.append(("label.regions", r, True))
    (set_dir / "labels.toml").write_text(
        toml_out.document(
            "Answer labels adjudicated by the owner from the construction record.", label_tables
        )
    )
    return report
