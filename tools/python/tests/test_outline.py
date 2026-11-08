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


def test_slivers_along_one_moved_edge_are_one_piece():
    # The near edge moved 8 px, but the segmenter kept the old line in two places: three
    # slivers in a row, each too small to score, that are one change.
    before = rect(768, 1024, 100, 300, 900, 500)
    after = rect(768, 1024, 100, 300, 900, 508)
    after[500:508, 330:360] = False
    after[500:508, 600:640] = False
    found = outline.compare(before, after, scale=1.0)
    added = [p for p in found["pieces"] if p["kind"] == "added"]
    assert len(added) == 1 and added[0]["parts"] == 3, added
    x0, _, x1, _ = added[0]["bbox"]
    assert x0 < 0.11 and x1 > 0.87


def test_pieces_apart_or_out_of_line_are_not_chained():
    before = rect(768, 1024, 100, 300, 900, 500)
    after = before.copy()
    after[500:508, 100:300] = True  # one sliver under the left of the edge
    after[300:500, 900:908] = True  # one up the right side, across the other's axis
    found = outline.compare(before, after, scale=1.0)
    assert len([p for p in found["pieces"] if p["kind"] == "added"]) == 2, found["pieces"]


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


def _scores(layout: dict[str, tuple[int, int, int, int]], base: str = "sky") -> np.ndarray:
    """Segmenter-shaped scores over a 64x64 square: `base` everywhere, each named class winning
    inside its box (x0, y0, x1, y1), later boxes on top."""
    from squint_tools import segmenter

    names = segmenter.class_names()
    s = np.zeros((len(names), 64, 64), np.float32)
    s[names.index(base)] = 1.0
    for n, (name, (x0, y0, x1, y1)) in enumerate(layout.items(), 2):
        s[names.index(name), y0:y1, x0:x1] = float(n)
    return s


def _scan(before, after):
    from squint_tools import segmenter

    whole = segmenter.Placement(0, 0, segmenter.SIDE, segmenter.SIDE)
    names = segmenter.class_names()
    return outline.scan((before, whole), (after, whole), (256, 256), 1.0, names)


def test_the_scan_reports_a_removed_tower_once_on_the_tower():
    before = _scores(
        {"grass": (0, 48, 64, 64), "building": (8, 16, 56, 48), "tower": (24, 0, 32, 16)}
    )
    after = _scores({"grass": (0, 48, 64, 64), "building": (8, 16, 56, 48)})
    _, pieces = _scan(before, after)
    strong = [p for p in pieces if p["score"] >= 0.5]
    assert [(p["class"], p["kind"]) for p in strong] == [("tower", "removed")]
    assert strong[0]["other"] == "sky" and not strong[0]["relabel"]


def test_a_relabelling_scores_low():
    # The same shape read as house instead of building: the light changed, not the shape.
    before = _scores({"grass": (0, 48, 64, 64), "building": (8, 16, 56, 48)})
    after = _scores({"grass": (0, 48, 64, 64), "house": (8, 16, 56, 48)})
    _, pieces = _scan(before, after)
    assert pieces and all(p["relabel"] for p in pieces)
    assert all(p["score"] < 0.5 for p in pieces)


def test_a_new_sign_over_a_shed_is_one_piece_on_the_sign():
    before = _scores({"grass": (0, 48, 64, 64), "hovel": (8, 16, 56, 48)})
    after = _scores(
        {"grass": (0, 48, 64, 64), "hovel": (8, 16, 56, 48), "signboard": (20, 24, 36, 36)}
    )
    _, pieces = _scan(before, after)
    strong = [(p["class"], p["kind"]) for p in pieces if p["score"] >= 0.5]
    assert strong == [("signboard", "added")]


def test_an_unchanged_scene_has_no_pieces():
    s = _scores({"grass": (0, 48, 64, 64), "building": (8, 16, 56, 48), "tree": (56, 20, 64, 48)})
    compared, pieces = _scan(s, s.copy())
    assert {c for c, _ in compared} == {"grass", "building", "tree", "sky"} and pieces == []


def test_related_classes():
    assert outline.related("building", "house") and outline.related("road", "sidewalk")
    assert not outline.related("building", "sky") and not outline.related("boat", "car")
    names = set(__import__("squint_tools.segmenter", fromlist=["x"]).class_names())
    assert all(c in names for g in outline.GROUPS for c in g) and outline.STUFF <= names
