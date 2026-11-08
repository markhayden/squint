"""`regions`: planted changes on synthetic pairs are found (needs the fetched segmenter), and the
map machinery behind it works without the model."""

import base64
import io
import json
import os
import time
from pathlib import Path

import numpy as np
import pytest
from PIL import Image

from squint_tools import models, regions, server
from squint_tools.context import Context

MODELS = Path(os.environ["SQUINT_MODELS"]) if os.environ.get("SQUINT_MODELS") else None
RECORD = models.Record.load()
PRESENT = models.artifact_path(RECORD, MODELS).is_file()
needs_model = pytest.mark.skipif(
    not PRESENT,
    reason=f"the segmenter is not under {models.artifact_path(RECORD, MODELS).parent}: run `squint-tools fetch`",
)


def test_components_label_each_connected_blob_once():
    mask = np.zeros((8, 10), bool)
    mask[1:3, 1:4] = True
    mask[5:7, 6:9] = True
    mask[0, 9] = True
    labels, n = regions.components(mask)
    assert n == 3
    assert len(np.unique(labels[mask])) == 3 and (labels[~mask] == 0).all()
    assert len(np.unique(labels[1:3, 1:4])) == 1


def test_box_blur_is_the_window_mean_and_normalised_luminance_removes_the_mean():
    a = np.arange(25, dtype=np.float32).reshape(5, 5)
    assert regions.box_blur(a, 1)[2, 2] == pytest.approx(12.0)
    flat = np.full((32, 32, 3), 90, np.uint8)
    assert np.abs(regions.normalised_luminance(flat)).max() < 1e-3


def scene(rng, w, h):
    """A sky over a floor with two fixed blocks, lightly noisy: the same every call for one rng."""
    img = np.zeros((h, w, 3), np.float32)
    y = np.linspace(0, 1, h)[:, None]
    img[..., 0], img[..., 1], img[..., 2] = 120 + 60 * y, 140 + 40 * y, 180 - 60 * y
    img[h // 2 :] = [110, 90, 70]
    img[int(0.6 * h) : int(0.9 * h), int(0.1 * w) : int(0.35 * w)] = (200, 30, 30)
    img[int(0.5 * h) : int(0.8 * h), int(0.6 * w) : int(0.9 * w)] = (30, 160, 40)
    return img, rng.normal(0, 2.5, img.shape)


def planted_pair(seed, w=640, h=480, changes=2):
    rng = np.random.default_rng(seed)
    base, noise = scene(rng, w, h)
    candidate = base.copy()
    planted = []
    while len(planted) < changes:
        bw, bh = rng.uniform(0.12, 0.3), rng.uniform(0.12, 0.3)
        x0, y0 = rng.uniform(0.02, 0.98 - bw), rng.uniform(0.02, 0.98 - bh)
        box = [x0, y0, x0 + bw, y0 + bh]
        # Separate boxes: two that touch are one change to any honest finder.
        if any(
            box[0] < q[2] + 0.04
            and q[0] < box[2] + 0.04
            and box[1] < q[3] + 0.04
            and q[1] < box[3] + 0.04
            for q in planted
        ):
            continue
        colour = rng.uniform(0, 255, 3)
        candidate[int(y0 * h) : int((y0 + bh) * h), int(x0 * w) : int((x0 + bw) * w)] = colour
        planted.append(box)
    to_u8 = lambda a: np.clip(a + noise, 0, 255).astype(np.uint8)
    return to_u8(candidate), to_u8(base), planted


def iou(a, b):
    ix = max(0.0, min(a[2], b[2]) - max(a[0], b[0]))
    iy = max(0.0, min(a[3], b[3]) - max(a[1], b[1]))
    inter = ix * iy
    return inter / ((a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter)


def handle(rgb):
    buf = io.BytesIO()
    Image.fromarray(rgb).save(buf, format="PNG")
    return {
        "media_type": "image/png",
        "width": rgb.shape[1],
        "height": rgb.shape[0],
        "projection": "flat",
        "base64": base64.b64encode(buf.getvalue()).decode("ascii"),
    }


def context(candidate, source, hints=None):
    return Context.parse(
        {
            "run_id": "run-1",
            "call_id": "call:3",
            "images": {"source": handle(source)},
            "frames": {
                "aligned:candidate:t1": {
                    "root": "candidate",
                    "projection": "flat",
                    "parent": "raw:source",
                    "transform": {
                        "id": "t1",
                        "mapping": {"homography": [1, 0, 0, 0, 1, 0, 0, 0, 1]},
                    },
                    "chain": [],
                    "pixels": handle(candidate),
                }
            },
            "hints": hints or {},
        }
    )


@needs_model
@pytest.mark.segmenter
@pytest.mark.parametrize("seed", [1, 2, 3, 4, 5])
def test_every_planted_change_is_found_within_the_cap(seed):
    candidate, source, planted = planted_pair(seed, changes=1 + seed % 3)
    env = regions.regions(
        {"frame": "aligned:candidate:t1", "against": "raw:source"}, context(candidate, source)
    )
    assert env["status"] == "ok", env["summary"]
    found = env["regions"]
    assert 1 <= len(found) <= regions.MAX_REGIONS
    for box in planted:
        best = max(iou(box, r["location"]["bbox"]) for r in found)
        assert best > 0.3, f"seed {seed}: planted {box} best IoU {best:.2f} among {found}"
    assert all(r["location"]["frame"] == "aligned:candidate:t1" for r in found)
    assert all(" -> " in r["label"] or r["label"].endswith("changed") for r in found)
    [image] = env["images"]
    assert image["frame"] == "crop:call:3:0" and image["mapping"] == {"bbox": [0.0, 0.0, 1.0, 1.0]}
    assert set(env["provenance"]) == {"tool", "version", "duration_ms", "lock_digest"}
    names = {m["name"] for m in env["measurements"]}
    assert names == {"changed_fraction", "photometric_fraction", "class_disagreement", "candidates"}


@needs_model
@pytest.mark.segmenter
def test_an_unchanged_pair_measures_nothing_and_hints_nudge_scores():
    candidate, source, _ = planted_pair(7, changes=0)
    env = regions.regions(
        {"frame": "aligned:candidate:t1", "against": "raw:source"}, context(candidate, source)
    )
    assert env["status"] == "measured_nothing", env["summary"]
    assert env["regions"] == [] and len(env["images"]) == 1
    candidate, source, _ = planted_pair(8, changes=1)
    plain = regions.regions(
        {"frame": "aligned:candidate:t1", "against": "raw:source"}, context(candidate, source)
    )
    before, after = (
        plain["regions"][0]["label"].split(" -> ")
        if " -> " in plain["regions"][0]["label"]
        else (plain["regions"][0]["label"].removesuffix(" changed"),) * 2
    )
    nudged = regions.regions(
        {"frame": "aligned:candidate:t1", "against": "raw:source"},
        context(candidate, source, {"regions": {"ignore": [before, after]}}),
    )
    assert nudged["regions"][0]["score"] <= plain["regions"][0]["score"]


def test_bad_frames_are_the_models_mistake():
    candidate, source, _ = planted_pair(9, w=64, h=48, changes=0)
    ctx = context(candidate, source)
    same = regions.regions({"frame": "raw:source", "against": "raw:source"}, ctx)
    assert same["status"] == "invalid_args" and "two different" in same["summary"]
    unknown = regions.regions({"frame": "crop:call:0:0", "against": "raw:source"}, ctx)
    assert unknown["status"] == "invalid_args" and "unknown frame id" in unknown["summary"]
    squashed = context(candidate[:, :32], source)
    grid = regions.regions({"frame": "aligned:candidate:t1", "against": "raw:source"}, squashed)
    assert grid["status"] == "invalid_args" and "one grid" in grid["summary"]


def test_the_server_lists_regions_with_its_dependencies():
    out = io.StringIO()
    server.serve(
        io.StringIO(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}) + "\n"), out
    )
    listed = {t["name"]: t for t in json.loads(out.getvalue())["result"]["tools"]}
    note = listed["regions"]["annotations"]["squint"]
    assert note["requires"] == ["align"] and note["frame_args"] == ["frame", "against"]
    assert note["accepts_hints"] == ["regions"] and note["returns"]["max_regions"] == 12


class FakeSegmenter:
    """Every pixel one class, so the photometric passes are tested on their own."""

    def segment(self, images):
        from squint_tools.segmenter import prepare

        out = []
        for rgb in images:
            _, place = prepare(rgb)
            out.append((np.zeros((128, 128), int), np.full((128, 128), 0.9), place))
        return out


def flat_pair(w=256, h=256, blocks=(), value=80):
    """A flat grey candidate with white blocks planted, and the unchanged source."""
    source = np.full((h, w, 3), value, np.uint8)
    candidate = source.copy()
    for x0, y0, x1, y1 in blocks:
        candidate[y0:y1, x0:x1] = 230
    return candidate, source


def test_components_on_a_serpentine_mask_stay_fast_and_count_one():
    mask = np.zeros((256, 256), bool)
    for row in range(0, 256, 4):
        mask[row, :] = True
        if (row // 4) % 2 == 0:
            mask[row : row + 4, 255] = True
        else:
            mask[row : row + 4, 0] = True
    started = time.monotonic()
    labels, n = regions.components(mask)
    assert time.monotonic() - started < 1.0
    assert n == 1 and (labels[mask] == 1).all() and (labels[~mask] == 0).all()


def test_the_cap_keeps_the_strongest_twelve_renumbered_and_says_how_many_were_left(monkeypatch):
    monkeypatch.setattr(regions.segmenter, "shared", lambda: FakeSegmenter())
    monkeypatch.setattr(regions.segmenter, "class_names", lambda: ["thing"] * 150)
    blocks = [(x, y, x + 28, y + 28) for x in range(8, 248, 60) for y in range(8, 248, 60)]
    assert len(blocks) == 16
    candidate, source = flat_pair(blocks=blocks)
    env = regions.regions(
        {"frame": "aligned:candidate:t1", "against": "raw:source"}, context(candidate, source)
    )
    assert env["status"] == "ok"
    assert [r["id"] for r in env["regions"]] == [f"r{n}" for n in range(1, 13)]
    candidates = next(m for m in env["measurements"] if m["name"] == "candidates")["value"]
    assert candidates == 16
    assert "4 weaker candidates were left out" in env["summary"]
    assert all(r["label"] == "thing changed" for r in env["regions"])


def test_specks_are_dropped_and_a_class_box_inside_a_photometric_box_is_not_counted_twice():
    names = ["a", "b"] + ["x"] * 148
    size = 256  # the working grid, so a planted speck keeps its size
    candidate, source = flat_pair(size, size, blocks=[(40, 40, 160, 160)])
    # A 2×2 speck is below MIN_AREA (and is opened away); the class maps disagree only inside
    # the planted block, over a box whose IoU with it is above 0.3.
    candidate[200:202, 200:202] = 230
    la = np.zeros((size, size), int)
    la[60:140, 60:140] = 1
    lb = np.zeros((size, size), int)
    conf = np.full((size, size), 0.9)
    found, measured = regions.find(candidate, source, (la, conf), (lb, conf), None, names)
    assert len(found) == 1, found
    assert found[0]["label"] in ("b -> a", "a -> b", "a changed")
    assert measured["candidates"] == 1


def test_hints_move_scores_by_exactly_their_factors():
    names = ["wall", "chair"] + ["x"] * 148
    size = 64
    candidate, source = flat_pair(size, size, blocks=[(8, 8, 40, 40)])
    la = np.full((size, size), 1)
    lb = np.full((size, size), 1)
    conf = np.full((size, size), 0.9)
    plain, _ = regions.find(candidate, source, (la, conf), (lb, conf), None, names)
    ignored, _ = regions.find(
        candidate, source, (la, conf), (lb, conf), {"ignore": ["chair"]}, names
    )
    preserved, _ = regions.find(
        candidate, source, (la, conf), (lb, conf), {"preserve": ["chair"]}, names
    )
    base = plain[0]["score"]
    assert ignored[0]["score"] == pytest.approx(round(base * 0.5, 3), abs=0.002)
    assert preserved[0]["score"] == pytest.approx(min(1.0, round(base * 1.25, 3)), abs=0.002)
    # Hints that are not an object are ignored rather than crashing.
    assert regions.find(candidate, source, (la, conf), (lb, conf), ["chair"], names)[0]


def test_the_overlay_is_capped_at_the_manifests_long_edge_and_a_projection_mismatch_is_refused(
    monkeypatch,
):
    big = np.zeros((1000, 2000, 3), np.uint8)
    out = regions.overlay(big, [{"id": "r1", "bbox": [0.1, 0.1, 0.5, 0.5]}])
    assert max(out.shape[:2]) == 1024 and out.shape[1] == 1024
    monkeypatch.setattr(regions.segmenter, "shared", lambda: FakeSegmenter())
    candidate, source = flat_pair(64, 32)
    ctx = Context.parse(
        {
            "run_id": "r",
            "call_id": "c",
            "images": {"source": {**handle(source), "projection": "equirectangular"}},
            "frames": {
                "aligned:candidate:t1": {
                    "root": "candidate",
                    "projection": "flat",
                    "parent": "raw:source",
                    "transform": None,
                    "chain": [],
                    "pixels": handle(candidate),
                }
            },
            "hints": {},
        }
    )
    env = regions.regions({"frame": "aligned:candidate:t1", "against": "raw:source"}, ctx)
    assert env["status"] == "invalid_args" and "one grid" in env["summary"]


def textured_pair(seed=3, size=256):
    rng = np.random.default_rng(seed)
    base = rng.uniform(40, 200, (size // 8, size // 8, 3))
    base = np.asarray(Image.fromarray(base.astype(np.uint8)).resize((size, size), Image.BICUBIC))
    return base.copy(), base


def graded(rgb, gain=(1.25, 1.05, 0.75), offset=12.0):
    return (rgb.astype(np.float32) * np.array(gain) + offset).clip(0, 255).astype(np.uint8)


def test_a_grade_over_the_whole_frame_is_no_region_and_does_not_hide_a_local_one():
    names = ["wall"] * 150
    candidate, source = textured_pair()
    lab, conf = np.zeros((256, 256), int), np.full((256, 256), 0.9)
    found, _ = regions.find(graded(candidate), source, (lab, conf), (lab, conf), None, names)
    assert [r for r in found if r["score"] >= 0.5] == [], found
    candidate[100:140, 60:100] = (230, 40, 200)
    found, _ = regions.find(graded(candidate), source, (lab, conf), (lab, conf), None, names)
    strong = [r for r in found if r["score"] >= 0.5]
    assert len(strong) == 1, found
    x0, y0, x1, y1 = strong[0]["bbox"]
    assert x0 < 100 / 256 < x1 and y0 < 120 / 256 < y1


def test_a_class_flip_with_nothing_changed_in_the_pixels_scores_low():
    names = ["tank", "building", "tree"] + ["x"] * 147
    candidate, source = textured_pair()
    conf = np.full((256, 256), 0.9)
    before = np.zeros((256, 256), int)
    for after_class in (1, 2):  # related (tank, building) and not (tank, tree)
        after = before.copy()
        after[60:140, 60:140] = after_class
        found, _ = regions.find(candidate, source, (after, conf), (before, conf), None, names)
        assert found and all(r["score"] < 0.3 for r in found), found
