"""`outline`: the shape comparison on planted masks (no segmenter), the manifest, and the
argument checks."""

import numpy as np

from squint_tools import outline, server
from squint_tools.context import Context


def rect(h, w, x0, y0, x1, y1):
    m = np.zeros((h, w), bool)
    m[y0:y1, x0:x1] = True
    return m


def test_identical_shapes_have_no_pieces_and_no_shift():
    m = rect(300, 400, 50, 100, 350, 250)
    found = outline.compare(m, m.copy(), scale=1.0)
    assert found["iou"] == 1.0
    assert found["shift_p95_px"] == 0.0 and found["shift_max_px"] == 0.0
    assert found["pieces"] == []


def test_a_widened_edge_is_one_added_piece_with_its_shift():
    before = rect(300, 400, 50, 100, 350, 200)
    after = rect(300, 400, 50, 100, 350, 230)  # the near edge moved 30 px down
    found = outline.compare(before, after, scale=2.0)
    added = [p for p in found["pieces"] if p["kind"] == "added"]
    assert len(added) == 1 and not [p for p in found["pieces"] if p["kind"] == "removed"]
    _, y0, _, y1 = added[0]["bbox"]
    assert abs(y0 - 200 / 300) < 0.02 and abs(y1 - 230 / 300) < 0.02
    # 30 grid px at scale 2 is 60 frame px.
    assert 55 <= added[0]["shift_px"] <= 62
    assert found["area_change"] > 0.25


def test_a_cut_corner_is_a_removed_piece():
    before = rect(300, 400, 50, 100, 350, 250)
    after = before.copy()
    after[100:160, 300:350] = False
    found = outline.compare(before, after, scale=1.0)
    assert [p["kind"] for p in found["pieces"]] == ["removed"]


def test_the_frame_edge_is_not_boundary():
    # A shape that runs off the right edge in both: the edge contributes no shift.
    before = rect(300, 400, 100, 100, 400, 200)
    after = rect(300, 400, 100, 100, 400, 200)
    edge = outline.boundary(before)
    assert not edge[:, -2:].any()
    assert outline.compare(before, after, 1.0)["shift_max_px"] == 0.0


def test_thin_jitter_along_a_boundary_is_not_a_piece():
    before = rect(300, 400, 50, 100, 350, 200)
    after = rect(300, 400, 50, 100, 350, 202)  # 2 px: under the jitter bar
    assert outline.compare(before, after, 1.0)["pieces"] == []


def test_specks_are_removed_and_the_main_shape_kept():
    m = rect(300, 400, 50, 100, 350, 200)
    m[10:14, 10:14] = True  # 16 px of 120 000: a speck
    kept = outline.despeckled(m)
    assert not kept[10:14, 10:14].any() and kept[150, 200]


def test_the_manifest_is_served_and_needs_the_segmenter():
    names = [m["name"] for m in server.select(None)]
    assert "outline" in names
    m = outline.MANIFEST
    assert m["requires"] == ["align"] and m["accepts_hints"] == ["outline"]
    assert m["returns"]["max_regions"] == outline.MAX_REGIONS
    from squint_tools import tools

    assert "outline" in tools.NEEDS_SEGMENTER and tools.HANDLERS["outline"] is outline.outline


def test_bad_arguments_are_the_models_mistake():
    ctx = Context.parse({"run_id": "r", "call_id": "call:1", "images": {}, "frames": {}})
    same = outline.outline({"frame": "raw:source", "against": "raw:source"}, ctx)
    assert same["status"] == "invalid_args"
    unknown = outline.outline({"frame": "aligned:candidate:t1", "against": "raw:source"}, ctx)
    assert unknown["status"] == "invalid_args" and "unknown frame" in unknown["summary"]


def test_aliases_name_real_segmenter_classes():
    from squint_tools import segmenter

    names = set(segmenter.class_names())
    assert all(target in names for target in outline.ALIASES.values())
    assert not (set(outline.ALIASES) & names), "an alias must not shadow a real class"


def test_the_speck_bar_is_relative_so_a_small_sole_object_survives():
    clock = rect(300, 400, 200, 50, 212, 62)  # 144 px of 120 000: 0.12% of the frame, all of it
    assert outline.despeckled(clock)[55, 205]
    pool = rect(300, 400, 50, 100, 350, 250)
    pool[10:14, 10:14] = True  # a cushion speck beside a large pool
    kept = outline.despeckled(pool)
    assert not kept[10:14, 10:14].any() and kept[150, 200]


def test_one_bar_for_both_images():
    # A fragment just over the bar of a small mask, but under the bar set by the other image's
    # large shape, goes on both sides.
    before = rect(300, 400, 50, 100, 350, 250)
    before[10:20, 10:20] = True
    after = rect(300, 400, 10, 10, 20, 20)
    b, a = outline.despeckled_pair(before, after)
    assert not b[10:20, 10:20].any() and not a.any()


def test_a_complete_removal_scores_one_and_has_no_shift():
    before = rect(300, 400, 100, 100, 140, 200)  # a bench: 1.3% of the frame
    after = np.zeros_like(before)
    found = outline.compare(before, after, scale=1.0)
    (piece,) = found["pieces"]
    assert piece["kind"] == "removed" and piece["score"] == 1.0 and piece["shift_px"] is None


def test_a_small_piece_on_the_frame_edge_scores_half():
    # The class loses a 10 px strip where it meets the frame's left edge: 0.42% of the frame.
    before = rect(300, 400, 0, 100, 30, 150)
    after = rect(300, 400, 10, 100, 30, 150)
    big = rect(300, 400, 100, 50, 380, 280)
    found = outline.compare(before | big, after | big, scale=1.0)
    (piece,) = [p for p in found["pieces"] if p["kind"] == "removed"]
    assert piece["edge"] and piece["score"] <= 0.5


def test_a_large_objects_change_at_the_frame_edge_keeps_its_score():
    # A pool running off the right edge, its end redrawn: part of a large object, not halved.
    before = rect(300, 400, 50, 100, 400, 250)
    after = before.copy()
    after[100:140, 385:400] = False  # 600 px: 0.5% of the frame, on the edge
    (piece,) = [p for p in outline.compare(before, after, 1.0)["pieces"] if p["kind"] == "removed"]
    assert piece["edge"] and piece["object"] > outline.EDGE_OBJECT
    plain = 0.5 * min(1.0, piece["area"] / 0.005) + 0.5 * min(1.0, (piece["shift_px"] or 0) / 30)
    assert piece["score"] >= round(min(1.0, plain), 3) - 0.001


def test_a_separate_structure_a_tenth_the_size_of_the_largest_is_kept():
    # A church tower beside a long terrace: a real structure, not a speck.
    m = rect(300, 400, 0, 100, 300, 250) | rect(300, 400, 330, 120, 345, 220)
    kept = outline.despeckled(m)
    assert kept[150, 335]
