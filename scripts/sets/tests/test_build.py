"""The builder turns a plan into a set directory plus an owner worksheet, with the fake generator."""

import hashlib
import tomllib
from pathlib import Path

from squint_sets import build
from squint_sets import plan as plan_mod
from squint_sets.generators import FakeGenerator

PLAN = """
[set]
id = "source-fidelity/houses-v1"
use_case = "source-fidelity"
split = "dev"
licence = "CC-BY-4.0"
images_repo = "github.com/markhayden/wafflery-evalsets"
rules = { base = "source-fidelity", version = 1 }

[[family]]
id = "brick-house"
generator = "fake"
description = "a red-brick house with a green door, a blue bicycle and a maple tree"
brief = "Restyle the scene; keep the structure."
source = { prompt = "A photograph of a red-brick house.", seed = 1234, size = 256 }

[[family.item]]
id = "brick-house-grade"
op = { kind = "grade", warmth = 0.1 }

[[family.item]]
id = "brick-house-no-bicycle"
op = { kind = "edit", prompt = "Remove the bicycle; change nothing else.", removes = "bicycle" }
"""


def test_plan_parses_and_refuses_unknown_keys(tmp_path: Path):
    p = plan_mod.parse(PLAN)
    assert p.set.id == "source-fidelity/houses-v1"
    assert [f.id for f in p.families] == ["brick-house"]
    assert [i.id for i in p.families[0].items] == ["brick-house-grade", "brick-house-no-bicycle"]
    bad = PLAN.replace('split = "dev"', 'split = "dev"\nexpected = "pass"')
    try:
        plan_mod.parse(bad)
    except ValueError as e:
        assert "expected" in str(e)
    else:
        raise AssertionError("accepted an unknown key")


def test_build_writes_the_set_and_the_worksheet(tmp_path: Path):
    out = tmp_path / "evalsets" / "source-fidelity" / "houses-v1"
    owner = tmp_path / "owner"
    report = build.build(plan_mod.parse(PLAN), out, owner, generators={"fake": FakeGenerator()})
    assert report.items == 2 and report.images == 3

    manifest = tomllib.loads((out / "set.toml").read_text())
    assert manifest["set"]["id"] == "source-fidelity/houses-v1"
    assert manifest["set"]["labels"] == "labels.toml"
    assert manifest["set"]["rules"] == {"base": "source-fidelity", "version": 1}
    assert [i["id"] for i in manifest["item"]] == ["brick-house-grade", "brick-house-no-bicycle"]
    assert all(i["family"] == "brick-house" for i in manifest["item"])
    families = tomllib.loads((out / "families.toml").read_text())
    assert families["family"][0]["id"] == "brick-house"
    # No answer label anywhere public: no labels.toml yet (the draft lives in owner storage).
    assert not (out / "labels.toml").exists()

    job = tomllib.loads((out / "items/brick-house-grade/job.toml").read_text())["job"]
    assert job["use_case"] == "source-fidelity"
    assert job["brief"] == "Restyle the scene; keep the structure."
    roles = {i["id"]: i for i in job["images"]}
    assert roles["candidate"]["role"] == "candidate" and roles["source"]["role"] == "source"
    for img in job["images"]:
        path = out / img["source"]["path"]
        assert path.exists(), path
        assert hashlib.sha256(path.read_bytes()).hexdigest() == img["source"]["sha256"]
    # The source is shared by the family, stored once.
    assert roles["source"]["source"]["path"] == "families/brick-house/source.png"
    assert "expected" not in (out / "items/brick-house-grade/job.toml").read_text()

    draft = tomllib.loads((owner / "source-fidelity/houses-v1/labels-draft.toml").read_text())
    by_item = {l["item"]: l for l in draft["label"]}
    assert by_item["brick-house-grade"]["expected"] == "pass"
    assert by_item["brick-house-grade"]["status"] == "draft"
    assert by_item["brick-house-no-bicycle"]["expected"] == "fail"
    assert by_item["brick-house-no-bicycle"]["obligation_ids"] == ["SF1"]
    assert by_item["brick-house-no-bicycle"]["regions"][0]["kind"] == "bbox"
    record = tomllib.loads((owner / "source-fidelity/houses-v1/construction.toml").read_text())
    assert record["item"][1]["op"]["kind"] == "edit"
    assert record["family"][0]["generator"] == "fake"
    assert (owner / "source-fidelity/houses-v1/contact-brick-house.png").exists()


def test_promote_publishes_only_confirmed_labels(tmp_path: Path):
    out = tmp_path / "evalsets" / "source-fidelity" / "houses-v1"
    owner = tmp_path / "owner"
    build.build(plan_mod.parse(PLAN), out, owner, generators={"fake": FakeGenerator()})
    draft_path = owner / "source-fidelity/houses-v1/labels-draft.toml"
    text = draft_path.read_text().replace('status = "draft"', 'status = "confirmed"', 1)
    text = text.replace('status = "draft"', 'status = "struck"', 1)
    draft_path.write_text(text)
    report = build.promote(draft_path, out)
    assert (report.confirmed, report.struck) == (1, 1)
    labels = tomllib.loads((out / "labels.toml").read_text())
    assert [l["item"] for l in labels["label"]] == ["brick-house-grade"]
    assert "status" not in labels["label"][0]
    manifest = tomllib.loads((out / "set.toml").read_text())
    assert [i["id"] for i in manifest["item"]] == ["brick-house-grade"], "struck item removed"
    assert not (out / "items/brick-house-no-bicycle").exists()
    # A draft left unreviewed is refused.
    build.build(plan_mod.parse(PLAN), out, owner / "again", generators={"fake": FakeGenerator()})
    try:
        build.promote(owner / "again/source-fidelity/houses-v1/labels-draft.toml", out)
    except ValueError as e:
        assert "draft" in str(e)
    else:
        raise AssertionError("promoted an unreviewed draft")
