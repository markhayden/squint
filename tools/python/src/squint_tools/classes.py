"""`classes`: what the segmenter sees in one region of two frames in one grid (PLAN task 5.4b).

For a box on `frame`, the classes covering it on each side with their share of the box and the
segmenter's mean confidence there, as measurements the model can read, plus one region of kind
`class_difference` when the dominant class differs between the sides. It answers "what was this,
and what is it now" after `regions` has pointed somewhere; it decides nothing.
"""

from __future__ import annotations

import time
from typing import Any

import numpy as np

from . import segmenter
from .context import Context
from .envelope import bbox_location, envelope, invalid_args

# Classes covering less of the box than this are folded into the remainder.
MIN_SHARE = 0.05
MAX_CLASSES = 4

RETURNS = {
    "max_images": 0,
    "max_long_edge": 0,
    "max_image_bytes": 0,
    "max_summary_bytes": 2048,
    "max_regions": 1,
    "max_region_label_bytes": 120,
    "max_measurements": 8,
    "max_transforms": 0,
    "max_envelope_bytes": 65_536,
}

MODEL_SCHEMA = {
    "type": "object",
    "additionalProperties": False,
    "required": ["frame", "against", "bbox"],
    "properties": {
        "frame": {
            "type": "string",
            "description": "The frame the box is on, usually the aligned candidate "
            "(aligned:candidate:t1): a frame id, or an image id (candidate) for its raw frame "
            "(raw:candidate).",
        },
        "against": {
            "type": "string",
            "description": "The frame to compare it with, in the same grid, usually the source: "
            "a frame id (raw:source), or an image id (source) for its raw frame.",
        },
        "bbox": {
            "type": "array",
            "items": {"type": "number", "minimum": 0, "maximum": 1},
            "minItems": 4,
            "maxItems": 4,
            "description": "[left, top, right, bottom] as fractions of the frame: the region to read.",
        },
    },
}

MANIFEST = {
    "manifest_version": 1,
    "name": "classes",
    "version": "1.0.0",
    "purpose": "Names what the segmenter sees in one region of an aligned pair, before and after: "
    "the classes covering the box on each side with their shares.",
    "when_to_use": "On a region `regions` pointed at, or any box you want named, to learn what "
    "was there and what is there now.",
    "when_not_to_use": "To judge whether a change is allowed; the rules decide that. On frames "
    "that do not share one grid.",
    "output_guidance": "Shares are of the box, from a 150-class scene segmenter that can be "
    "wrong on unusual images; low confidence says so. A class difference is a pointer.",
    "cost": {"tier": "expensive", "seconds": 3.0},
    "requires": ["align"],
    "pairs_with": ["regions", "view"],
    "image_args": [],
    "frame_args": ["frame", "against"],
    "accepts_hints": [],
    "evidence_kind": "class_difference",
    "returns": RETURNS,
    "frames_out": [],
    "concurrent": False,
    "preconditions": "`frame` and `against` share one grid: an aligned frame and the frame it "
    "was aligned onto.",
    "model_schema": MODEL_SCHEMA,
    "input_schema": {
        **MODEL_SCHEMA,
        "required": ["frame", "against", "bbox", "_squint"],
        "properties": {**MODEL_SCHEMA["properties"], "_squint": {"type": "object"}},
    },
    "output_schema": {
        "type": "object",
        "properties": {
            "measurements": {
                "type": "array",
                "description": "before:<class> and after:<class> (ratio of the box each class "
                "covers, the largest first), confidence_before and confidence_after (ratio).",
            }
        },
    },
}


def composition(labels: np.ndarray, names: list[str]) -> list[tuple[str, float]]:
    """The classes covering a map, largest share first, those under `MIN_SHARE` folded away."""
    counts = np.bincount(labels.ravel(), minlength=len(names)) / labels.size
    order = np.argsort(-counts)
    out = [(names[i], float(counts[i])) for i in order[:MAX_CLASSES] if counts[i] >= MIN_SHARE]
    return out or [(names[int(order[0])], float(counts[order[0]]))]


def read(frame_rgb: np.ndarray, against_rgb: np.ndarray, bbox: list[float]) -> dict[str, Any]:
    """The composition of the box on each side and the mean confidence there."""
    seg = segmenter.shared()
    names = segmenter.class_names()
    (la, ca, pa), (lb, cb, pb) = seg.segment([frame_rgb, against_rgb])
    out: dict[str, Any] = {}
    for side, (labels, conf, place) in (("after", (la, ca, pa)), ("before", (lb, cb, pb))):
        # The box is read at the map's own resolution: the part of the map that covers the
        # image, then the box's cells within it, never the map blown up to the image.
        labels, conf = place.crop(labels), place.crop(conf)
        h, w = labels.shape
        y0, y1 = int(bbox[1] * h), max(int(bbox[1] * h) + 1, int(np.ceil(bbox[3] * h)))
        x0, x1 = int(bbox[0] * w), max(int(bbox[0] * w) + 1, int(np.ceil(bbox[2] * w)))
        out[side] = composition(labels[y0:y1, x0:x1], names)
        out[f"confidence_{side}"] = float(conf[y0:y1, x0:x1].mean())
    return out


def _bbox(value: Any) -> list[float] | None:
    ok = (
        isinstance(value, list)
        and len(value) == 4
        and all(isinstance(v, (int, float)) and not isinstance(v, bool) for v in value)
        and 0.0 <= value[0] < value[2] <= 1.0
        and 0.0 <= value[1] < value[3] <= 1.0
    )
    return [float(v) for v in value] if ok else None


def classes(args: dict[str, Any], ctx: Context) -> dict[str, Any]:
    started = time.monotonic()
    ids = [args.get(k) for k in ("frame", "against")]
    if not all(isinstance(i, str) for i in ids):
        return invalid_args("classes", "frame and against must be frame ids")
    frame_id, against_id = ids
    if frame_id == against_id:
        return invalid_args("classes", "frame and against must be two different frames")
    pair = [ctx.frame_pixels(i) for i in ids]
    for key, i, p in zip(("frame", "against"), ids, pair):
        if p is None:
            return invalid_args("classes", f"{key}: unknown frame id {i!r}")
    frame_px, against_px = pair
    if (
        frame_px.projection != against_px.projection
        or abs(
            (frame_px.width / max(frame_px.height, 1))
            / (against_px.width / max(against_px.height, 1))
            - 1.0
        )
        > 0.01
    ):
        return invalid_args(
            "classes",
            f"{frame_id!r} and {against_id!r} must share one grid (projection and aspect)",
        )
    bbox = _bbox(args.get("bbox"))
    if bbox is None:
        return invalid_args(
            "classes",
            "bbox must be [left, top, right, bottom] as fractions with left < right and top < bottom",
        )
    found = read(frame_px.load(), against_px.load(), bbox)
    # The two confidences first, then the shares by rank across both sides: with four classes a
    # side there are ten figures for a cap of eight, and the smallest shares are the ones to lose.
    measurements = [
        {
            "name": f"confidence_{side}",
            "value": round(found[f"confidence_{side}"], 3),
            "unit": "ratio",
            "frame": frame_id,
        }
        for side in ("before", "after")
    ]
    for rank in range(MAX_CLASSES):
        for side in ("before", "after"):
            if rank < len(found[side]):
                name, share = found[side][rank]
                measurements.append(
                    {
                        "name": f"{side}:{name}",
                        "value": round(share, 3),
                        "unit": "ratio",
                        "frame": frame_id,
                    }
                )
    measurements = measurements[: RETURNS["max_measurements"]]
    describe = lambda side: ", ".join(f"{n} {s:.0%}" for n, s in found[side])
    before, after = found["before"][0][0], found["after"][0][0]
    summary = (
        f"In the box on {frame_id}: before (on {against_id}) {describe('before')}; "
        f"after {describe('after')}. Confidence {found['confidence_before']:.2f} before, "
        f"{found['confidence_after']:.2f} after."
    )
    regions = []
    if before != after:
        regions.append(
            {
                "id": "r1",
                "location": bbox_location(frame_id, bbox),
                "label": f"{before} -> {after}"[:120],
                "score": round(min(found["confidence_before"], found["confidence_after"]), 3),
            }
        )
        summary += f" The dominant class changed: {before} -> {after}."
    else:
        summary += f" The dominant class is {after} on both sides."
    env = envelope(
        "classes",
        "ok",
        summary,
        regions=regions,
        measurements=measurements,
        version=MANIFEST["version"],
    )
    env["provenance"]["duration_ms"] = int((time.monotonic() - started) * 1000)
    return env
