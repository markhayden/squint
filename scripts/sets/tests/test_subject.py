"""Subject families (reference consistency): references of one subject, candidates generated
into new scenes from them (pass, to be confirmed by the owner), an edit of a candidate that
changes a marking (fail, RC2) and a lookalike generated without the references (fail, RC3)."""

import hashlib
import tomllib
from pathlib import Path

from squint_sets import build
from squint_sets import plan as plan_mod
from squint_sets.generators import FakeGenerator

PLAN = """
[set]
id = "reference-consistency/pets-v1"
use_case = "reference-consistency"
split = "dev"
licence = "CC-BY-4.0"
images_repo = "github.com/markhayden/wafflery-evalsets"
rules = { base = "reference-consistency", version = 1 }

[[family]]
id = "pip"
kind = "subject"
generator = "fake"
description = "Pip, a golden retriever with a white chest patch and a red collar"
subject = "Pip, a golden retriever with a white chest patch on the chest and a red collar"
protected = ["Pip's white chest patch", "Pip's red collar"]
references = [
  { prompt = "Studio photo of Pip sitting, front view, plain grey background.", seed = 11 },
  { prompt = "Pip standing on grass, side view, daylight.", seed = 12 },
]

[[family.item]]
id = "pip-bridge"
scene = { prompt = "Pip on the bridge of a starship, evening light.", seed = 21 }

[[family.item]]
id = "pip-bridge-no-patch"
from = "pip-bridge"
op = { kind = "edit", prompt = "Make the chest fur the same golden colour as the rest of the coat; remove the white patch. Change nothing else.", alters = "white chest patch" }

[[family.item]]
id = "pip-bridge-lookalike"
scene = { prompt = "A golden retriever on the bridge of a starship, evening light.", seed = 23 }
lookalike = "a golden retriever with no chest patch and a blue collar"
"""


def test_subject_plan_parses():
    p = plan_mod.parse(PLAN)
    f = p.families[0]
    assert f.kind == "subject" and len(f.references) == 2
    assert [i.id for i in f.items] == ["pip-bridge", "pip-bridge-no-patch", "pip-bridge-lookalike"]
    assert f.items[1].from_item == "pip-bridge"
    bad = PLAN.replace('from = "pip-bridge"', 'from = "nowhere"')
    try:
        plan_mod.parse(bad)
    except ValueError as e:
        assert "nowhere" in str(e)
    else:
        raise AssertionError("accepted an unknown from item")


def test_subject_family_builds_references_scenes_edits_and_lookalikes(tmp_path: Path):
    out = tmp_path / "evalsets/reference-consistency/pets-v1"
    owner = tmp_path / "owner"
    report = build.build(plan_mod.parse(PLAN), out, owner, generators={"fake": FakeGenerator()})
    assert report.items == 3 and report.images == 5, "two references and three candidates"

    job = tomllib.loads((out / "items/pip-bridge/job.toml").read_text())["job"]
    assert job["use_case"] == "reference-consistency"
    assert job["brief"] == "Pip on the bridge of a starship, evening light."
    ids = {i["id"]: i for i in job["images"]}
    assert ids["candidate"]["role"] == "candidate"
    assert ids["ref:pip-1"]["role"] == "reference" and ids["ref:pip-1"]["label"].startswith("Pip")
    assert ids["ref:pip-2"]["source"]["path"] == "families/pip/ref-2.png"
    assert job["requirements"][0]["evidence"] == ["ref:pip-1", "ref:pip-2"]
    for img in job["images"]:
        path = out / img["source"]["path"]
        assert hashlib.sha256(path.read_bytes()).hexdigest() == img["source"]["sha256"]

    draft = tomllib.loads((owner / "reference-consistency/pets-v1/labels-draft.toml").read_text())
    by_item = {label["item"]: label for label in draft["label"]}
    assert by_item["pip-bridge"]["expected"] == "pass"
    assert "confirm" in by_item["pip-bridge"]["words"].lower()
    assert by_item["pip-bridge-no-patch"]["expected"] == "fail"
    assert by_item["pip-bridge-no-patch"]["obligation_ids"][:1] == ["RC2"]
    assert "RQ1" in by_item["pip-bridge-no-patch"]["obligation_ids"]
    assert by_item["pip-bridge-no-patch"]["regions"][0]["frame"] == "raw:candidate"
    assert by_item["pip-bridge-lookalike"]["expected"] == "fail"
    assert by_item["pip-bridge-lookalike"]["obligation_ids"] == ["RC3", "RC1"]
    assert by_item["pip-bridge-lookalike"]["regions"][0]["kind"] == "whole_image"

    record = tomllib.loads((owner / "reference-consistency/pets-v1/construction.toml").read_text())
    assert record["family"][0]["kind"] == "subject"
    assert len(record["family"][0]["references"]) == 2
    assert record["item"][1]["from"] == "pip-bridge"
    assert record["item"][2]["lookalike"].startswith("a golden retriever")
    assert (owner / "reference-consistency/pets-v1/contact-pip.png").exists()
