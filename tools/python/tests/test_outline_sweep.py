"""`outline` against planted edits of known size (scripts/outline_sweep.py), at points the sweep
showed it holds: a regression here means the tool lost ground it had. Needs the fetched
segmenter and the eval set's hotel-pool source (`squint sets fetch`)."""

import importlib.util
import os
from pathlib import Path

import numpy as np
import pytest
from PIL import Image

from squint_tools import models

MODELS = Path(os.environ["SQUINT_MODELS"]) if os.environ.get("SQUINT_MODELS") else None
RECORD = models.Record.load()
HERE = Path(__file__).resolve().parent
SOURCE = HERE.parents[2] / "evalsets/source-fidelity/scenes-v1/families/hotel-pool/source.png"
needs_inputs = pytest.mark.skipif(
    not models.artifact_path(RECORD, MODELS).is_file() or not SOURCE.is_file(),
    reason="needs the segmenter (`squint-tools fetch`) and the hotel-pool source (`squint sets fetch`)",
)


def sweep():
    spec = importlib.util.spec_from_file_location(
        "outline_sweep", HERE.parent / "scripts/outline_sweep.py"
    )
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


@pytest.fixture(scope="module")
def setup():
    s = sweep()
    raw = np.asarray(Image.open(SOURCE).convert("RGB"))
    h = raw.shape[0]
    # The hotel pool runs off the bottom of the frame: its back edge is the one that moves.
    flipped = raw[::-1].copy()
    m = s.pool_mask(raw)[::-1]
    ys, xs = np.nonzero(m)
    bottom = int(np.median([ys[xs == x].max() for x in np.unique(xs)[::5]]))
    span = int(0.06 * h)

    def moved(n):
        img = s.resample_rows(flipped, bottom - span, bottom, min(h, bottom + 2 * span), n)
        return np.ascontiguousarray(img[::-1]), [0.0, (h - bottom - n) / h, 1.0, (h - bottom) / h]

    return s, raw, moved


@needs_inputs
@pytest.mark.segmenter
@pytest.mark.parametrize("mode", ["named", "all"])
def test_an_edge_moved_20_px_is_caught(setup, mode):
    s, raw, moved = setup
    cand, truth = moved(20)
    caught, _, best = s.judge(s.pieces(raw, cand)[mode], truth)
    assert caught, f"{mode}: best score on the moved edge {best}"


@needs_inputs
@pytest.mark.segmenter
def test_an_edge_moved_4_px_is_boundary_noise(setup):
    s, raw, moved = setup
    cand, _ = moved(4)
    strong = [p for p in s.pieces(raw, cand)["named"] if p["score"] >= s.STRONG]
    assert strong == [], strong


@needs_inputs
@pytest.mark.segmenter
@pytest.mark.parametrize("kind", ["golden", "fog", "dusk"])
def test_a_restyle_alone_reports_nothing_on_the_named_class(setup, kind):
    s, raw, _ = setup
    _, false, _ = s.judge(s.pieces(raw, s.grade(raw, kind, 1.0))["named"], None)
    assert false == 0, f"{kind}: {false} strong pieces on an unchanged pool"


@needs_inputs
@pytest.mark.segmenter
@pytest.mark.parametrize("kind", ["golden", "fog", "dusk"])
def test_a_restyle_does_not_hide_a_30_px_move(setup, kind):
    s, raw, moved = setup
    cand, truth = moved(30)
    caught, _, best = s.judge(s.pieces(raw, s.grade(cand, kind, 1.0))["named"], truth)
    assert caught, f"{kind}: best score on the moved edge {best}"
