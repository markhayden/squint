"""A set plan: what to generate, per family, and which construction op makes each item.

Strict: unknown keys anywhere are refused, so an answer label cannot sneak into a plan and from
there into a public file. The plan is owner material; it goes to owner storage with the record.
"""

from __future__ import annotations

import tomllib
from dataclasses import dataclass, field
from typing import Any

GENERATORS = ("fal", "openai", "gemini", "fake")
SPLITS = ("dev", "heldout", "blind")


@dataclass(frozen=True)
class SetSpec:
    id: str
    use_case: str
    split: str
    licence: str
    images_repo: str
    rules: dict[str, Any]
    images_rev: str | None = None


@dataclass(frozen=True)
class Source:
    prompt: str
    seed: int
    size: int = 1024


@dataclass(frozen=True)
class Item:
    id: str
    # Scene families: the op applied to the family source. Subject families: `scene` generates
    # the candidate from the references; `from_item` plus `op` edits an earlier candidate;
    # `lookalike` generates the scene from a changed description without the references.
    op: dict[str, Any] | None = None
    scene: Source | None = None
    from_item: str | None = None
    lookalike: str | None = None
    # Scene families only: a generator for this item's candidate instead of the family's (the
    # source is still the family's), for an edit the family's generator will not make.
    generator: str | None = None


@dataclass(frozen=True)
class Family:
    id: str
    generator: str
    description: str
    items: list[Item] = field(default_factory=list)
    # "scene" (source fidelity: one source, items by op) or "subject" (reference consistency:
    # references of one subject, items generated into new scenes from them).
    kind: str = "scene"
    brief: str = ""
    source: Source | None = None
    subject: str = ""
    references: list[Source] = field(default_factory=list)
    # Protected subjects, one job requirement each (SF1/RC rules protect what they name).
    protected: list[str] = field(default_factory=list)
    # Brief families: what the brief asks for and forbids (one requirement each) and the exact
    # text it specifies.
    asks: list[str] = field(default_factory=list)
    forbids: list[str] = field(default_factory=list)
    text: list[str] = field(default_factory=list)


@dataclass(frozen=True)
class Plan:
    set: SetSpec
    families: list[Family]


def _only(table: dict[str, Any], allowed: set[str], where: str) -> None:
    extra = sorted(set(table) - allowed)
    if extra:
        raise ValueError(f"{where}: unknown key(s) {', '.join(extra)}")


def _require(table: dict[str, Any], keys: tuple[str, ...], where: str) -> None:
    missing = [k for k in keys if k not in table]
    if missing:
        raise ValueError(f"{where}: missing {', '.join(missing)}")


def parse(text: str) -> Plan:
    doc = tomllib.loads(text)
    _only(doc, {"set", "family"}, "plan")
    _require(doc, ("set", "family"), "plan")
    s = doc["set"]
    _only(s, {"id", "use_case", "split", "licence", "images_repo", "images_rev", "rules"}, "[set]")
    _require(s, ("id", "use_case", "split", "licence", "images_repo", "rules"), "[set]")
    if s["split"] not in SPLITS:
        raise ValueError(f"[set] split must be one of {SPLITS}")
    _only(s["rules"], {"base", "version"}, "[set] rules")
    _require(s["rules"], ("base", "version"), "[set] rules")
    spec = SetSpec(
        id=s["id"],
        use_case=s["use_case"],
        split=s["split"],
        licence=s["licence"],
        images_repo=s["images_repo"],
        images_rev=s.get("images_rev"),
        rules=dict(s["rules"]),
    )
    families: list[Family] = []
    seen_items: set[str] = set()
    for f in doc["family"]:
        where = f"family {f.get('id', '?')}"
        kind = f.get("kind", "scene")
        if kind not in ("scene", "subject", "brief"):
            raise ValueError(f"{where}: kind must be scene, subject or brief")
        if f.get("generator") not in GENERATORS:
            raise ValueError(f"{where}: generator must be one of {GENERATORS}")
        if kind == "scene":
            _only(
                f,
                {"id", "kind", "generator", "description", "brief", "source", "item", "protected"},
                where,
            )
            _require(f, ("id", "generator", "description", "brief", "source"), where)
            src = f["source"]
            _only(src, {"prompt", "seed", "size"}, f"{where} source")
            _require(src, ("prompt", "seed"), f"{where} source")
            source = Source(
                prompt=src["prompt"], seed=int(src["seed"]), size=int(src.get("size", 1024))
            )
            references: list[Source] = []
        elif kind == "brief":
            _only(
                f,
                {
                    "id",
                    "kind",
                    "generator",
                    "description",
                    "brief",
                    "asks",
                    "forbids",
                    "text",
                    "item",
                    "size",
                },
                where,
            )
            _require(f, ("id", "generator", "description", "brief", "asks"), where)
            source = None
            references = []
        else:
            _only(
                f,
                {
                    "id",
                    "kind",
                    "generator",
                    "description",
                    "subject",
                    "references",
                    "item",
                    "protected",
                    "size",
                },
                where,
            )
            _require(f, ("id", "generator", "description", "subject", "references"), where)
            source = None
            references = []
            for n, r in enumerate(f["references"], start=1):
                _only(r, {"prompt", "seed", "size"}, f"{where} reference {n}")
                _require(r, ("prompt", "seed"), f"{where} reference {n}")
                references.append(
                    Source(
                        prompt=r["prompt"],
                        seed=int(r["seed"]),
                        size=int(r.get("size", f.get("size", 1024))),
                    )
                )
            if not 1 <= len(references) <= 4:
                raise ValueError(f"{where}: one to four references")
        items: list[Item] = []
        family_item_ids: set[str] = set()
        for it in f.get("item", []):
            iw = f"{where} item {it.get('id', '?')}"
            _require(it, ("id",), iw)
            if it["id"] in seen_items:
                raise ValueError(f"{iw}: duplicate item id")
            seen_items.add(it["id"])
            if kind == "scene":
                _only(it, {"id", "op", "generator"}, iw)
                _require(it, ("id", "op"), iw)
                if "kind" not in it["op"]:
                    raise ValueError(f"{iw}: op needs a kind")
                if it.get("generator") is not None and it["generator"] not in GENERATORS:
                    raise ValueError(f"{iw}: generator must be one of {', '.join(GENERATORS)}")
                items.append(Item(id=it["id"], op=dict(it["op"]), generator=it.get("generator")))
            elif kind == "brief":
                _only(it, {"id", "op"}, iw)
                _require(it, ("id", "op"), iw)
                op = dict(it["op"])
                if op.get("kind") != "generate" or "seed" not in op:
                    raise ValueError(f"{iw}: a brief item's op is generate with a seed")
                if op.get("violates") not in (None, "BC1", "BC2", "BC3"):
                    raise ValueError(f"{iw}: violates must be BC1, BC2 or BC3")
                items.append(Item(id=it["id"], op=op))
            else:
                _only(it, {"id", "scene", "from", "op", "lookalike"}, iw)
                scene = None
                if "scene" in it:
                    _only(it["scene"], {"prompt", "seed", "size"}, f"{iw} scene")
                    _require(it["scene"], ("prompt", "seed"), f"{iw} scene")
                    scene = Source(
                        prompt=it["scene"]["prompt"],
                        seed=int(it["scene"]["seed"]),
                        size=int(it["scene"].get("size", f.get("size", 1024))),
                    )
                from_item = it.get("from")
                op = dict(it["op"]) if "op" in it else None
                lookalike = it.get("lookalike")
                if from_item is not None:
                    if from_item not in family_item_ids:
                        raise ValueError(
                            f"{iw}: from {from_item!r} is not an earlier item of this family"
                        )
                    if op is None or op.get("kind") != "edit":
                        raise ValueError(f"{iw}: an item built from another needs an edit op")
                    if scene is not None or lookalike is not None:
                        raise ValueError(f"{iw}: from takes an op only")
                elif scene is None:
                    raise ValueError(f"{iw}: a subject item needs a scene or a from")
                elif op is not None:
                    raise ValueError(f"{iw}: an op applies to a from item")
                items.append(
                    Item(id=it["id"], op=op, scene=scene, from_item=from_item, lookalike=lookalike)
                )
            family_item_ids.add(it["id"])
        if not items:
            raise ValueError(f"{where}: no items")
        families.append(
            Family(
                id=f["id"],
                kind=kind,
                generator=f["generator"],
                description=f["description"],
                brief=f.get("brief", ""),
                source=source,
                subject=f.get("subject", ""),
                references=references,
                items=items,
                protected=[str(x) for x in f.get("protected", [])],
                asks=[str(x) for x in f.get("asks", [])],
                forbids=[str(x) for x in f.get("forbids", [])],
                text=[str(x) for x in f.get("text", [])],
            )
        )
    return Plan(set=spec, families=families)


def load(path) -> Plan:
    with open(path, "rb") as f:
        return parse(f.read().decode("utf-8"))
