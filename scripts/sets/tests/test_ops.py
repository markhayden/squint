"""Deterministic construction operations and the draft label each one implies."""

import io

from PIL import Image

from squint_sets import ops


def png(w=64, h=48, colour=(200, 40, 40)):
    im = Image.new("RGB", (w, h), colour)
    # A distinct patch so flips and rotations are observable.
    im.paste((40, 40, 200), (0, 0, 16, 12))
    buf = io.BytesIO()
    im.save(buf, format="PNG")
    return buf.getvalue()


def load(b):
    return Image.open(io.BytesIO(b)).convert("RGB")


def test_mirror_flips_horizontally_and_is_a_structural_fail():
    out, label = ops.apply({"kind": "mirror"}, png(), generator=None, seed=1)
    im = load(out)
    assert im.size == (64, 48)
    assert im.getpixel((63, 0)) == (40, 40, 200), "the patch moved to the right edge"
    assert label.expected == "fail"
    assert label.obligation_ids == ["SF2"]
    assert label.region_kind == "whole_image"
    assert label.failure_type == "mirrored"


def test_rotate_changes_dimensions_for_90_degrees():
    out, label = ops.apply({"kind": "rotate", "degrees": 90}, png(), generator=None, seed=1)
    assert load(out).size == (48, 64)
    assert label.expected == "fail" and label.failure_type == "rotated"


def test_crop_keeps_a_fraction_and_fails_structure():
    out, label = ops.apply({"kind": "crop", "keep": 0.75}, png(), generator=None, seed=1)
    assert load(out).size == (48, 36)
    assert label.expected == "fail" and label.obligation_ids == ["SF2"]


def test_grade_is_a_mild_permitted_change():
    out, label = ops.apply({"kind": "grade", "warmth": 0.1}, png(), generator=None, seed=1)
    im = load(out)
    assert im.size == (64, 48)
    r, _g, b = im.getpixel((40, 40))
    assert r >= 200 and b < 40, "warmer: red up, blue down"
    assert label.expected == "pass"
    assert label.obligation_ids == []
    assert label.region_kind is None


class FakeGen:
    name = "fake"
    model = "fake-model"

    def edit(self, image, prompt, seed):
        im = load(image)
        im.paste((0, 255, 0), (20, 20, 40, 40))
        buf = io.BytesIO()
        im.save(buf, format="PNG")
        return buf.getvalue()


def test_edit_uses_the_generator_and_draws_the_region_from_the_pixel_difference():
    out, label = ops.apply(
        {
            "kind": "edit",
            "prompt": "Remove the bicycle; change nothing else.",
            "removes": "bicycle",
        },
        png(),
        generator=FakeGen(),
        seed=7,
    )
    assert load(out).getpixel((30, 30)) == (0, 255, 0)
    assert label.expected == "fail" and label.obligation_ids == ["SF1"]
    assert label.region_kind == "bbox"
    x0, y0, x1, y1 = label.bbox
    assert abs(x0 - 20 / 64) < 0.15 and abs(y0 - 20 / 48) < 0.2, label.bbox
    assert abs(x1 - 40 / 64) < 0.15 and abs(y1 - 40 / 48) < 0.2, label.bbox
    assert "bicycle" in label.words


def test_unknown_op_is_refused():
    try:
        ops.apply({"kind": "explode"}, png(), generator=None, seed=1)
    except ValueError as e:
        assert "explode" in str(e)
    else:
        raise AssertionError("accepted an unknown op")


def test_restyle_is_a_permitted_edit_that_keeps_structure():
    out, label = ops.apply(
        {
            "kind": "restyle",
            "prompt": "Make it dusk with warm window light. Keep every structure and object exactly where it is.",
        },
        png(),
        generator=FakeGen(),
        seed=3,
    )
    assert load(out).size == (64, 48)
    assert label.expected == "pass"
    assert label.obligation_ids == []
    assert label.region_kind is None
    assert "restyle" in label.words.lower()
    assert label.notes["prompt"].startswith("Make it dusk")
    assert "changed_area" in label.notes, "the reviewer is told where the edit landed"


class FakeTextGen:
    name = "fake"
    model = "fake-model"

    def generate(self, prompt, seed, size):
        im = Image.new("RGB", (size, size), (10, 10, 10))
        buf = io.BytesIO()
        im.save(buf, format="PNG")
        return buf.getvalue()


def test_generate_makes_the_candidate_from_its_own_prompt_with_the_defect_named():
    out, label = ops.apply(
        {
            "kind": "generate",
            "prompt": "A vendor whose hand has six fingers.",
            "defect": "anatomy",
            "words": "six fingers",
            "size": 32,
        },
        png(),
        generator=FakeTextGen(),
        seed=9,
    )
    assert load(out).size == (32, 32)
    assert label.expected == "fail" and label.obligation_ids == ["RE1"]
    assert label.region_kind == "whole_image"
    assert label.words == "six fingers"
    out, label = ops.apply(
        {"kind": "generate", "prompt": "A clean scene.", "size": 32},
        png(),
        generator=FakeTextGen(),
        seed=9,
    )
    assert label.expected == "pass" and label.obligation_ids == []


def textured(w=96, h=72):
    im = Image.new("RGB", (w, h))
    im.putdata(
        [((x * 7) % 256, (y * 11) % 256, ((x + y) * 5) % 256) for y in range(h) for x in range(w)]
    )
    buf = io.BytesIO()
    im.save(buf, format="PNG")
    return buf.getvalue()


def test_camera_ops_keep_the_size_fill_the_frame_and_fail_structure():
    for op, kind in (
        ({"kind": "tilt", "degrees": 3}, "tilted"),
        ({"kind": "zoom", "factor": 1.1}, "zoomed"),
        ({"kind": "keystone", "strength": 0.06}, "keystoned"),
    ):
        out, label = ops.apply(op, textured(), generator=None, seed=1)
        im = load(out)
        assert im.size == (96, 72), kind
        assert label.failure_type == kind and label.obligation_ids == ["SF2"]
        # Refilled: no corner is the black of an empty wedge.
        for xy in ((0, 0), (95, 0), (0, 71), (95, 71)):
            assert sum(im.getpixel(xy)) > 0, (kind, xy)
        assert out != textured(), kind


def test_camera_ops_refuse_out_of_range_strengths():
    for op in (
        {"kind": "tilt", "degrees": 30},
        {"kind": "zoom", "factor": 1.0},
        {"kind": "keystone", "strength": 0.5},
    ):
        try:
            ops.apply(op, textured(), generator=None, seed=1)
        except ValueError:
            continue
        raise AssertionError(op)


def test_delivery_damage_keeps_a_pass_a_pass_and_says_so():
    out, label = ops.apply(
        {"kind": "grade", "warmth": 0.1, "jpeg": 50, "resample": 0.5}, textured(), None, 1
    )
    assert load(out).size == (96, 72)
    assert label.expected == "pass"
    assert "JPEG at quality 50" in label.words and "50%" in label.notes["delivery"]


def test_an_edit_that_adds_names_what_was_added():
    class Gen:
        def edit(self, image, prompt, seed):
            im = load(image)
            im.paste((0, 255, 0), (30, 20, 50, 40))
            buf = io.BytesIO()
            im.save(buf, format="PNG")
            return buf.getvalue()

    _, label = ops.apply(
        {"kind": "edit", "prompt": "add a fourth dormer", "adds": "a fourth dormer"},
        textured(),
        Gen(),
        1,
    )
    assert label.expected == "fail" and label.failure_type == "a fourth dormer-added"
    assert label.words.startswith("the candidate has a fourth dormer")
