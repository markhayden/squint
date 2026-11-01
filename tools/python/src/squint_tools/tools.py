"""The tools this pack serves: their squint manifests and handlers."""

from __future__ import annotations

import math
from typing import Any

from . import __version__, classes, regions
from .context import Context
from .envelope import bbox_location, envelope, invalid_args, png_image

RETURNS = {
    "max_images": 1,
    "max_long_edge": 1024,
    "max_image_bytes": 4_000_000,
    "max_summary_bytes": 2048,
    "max_regions": 4,
    "max_region_label_bytes": 120,
    "max_measurements": 4,
    "max_transforms": 0,
    "max_envelope_bytes": 6_000_000,
}

ECHO_REGION_MODEL_SCHEMA = {
    "type": "object",
    "additionalProperties": False,
    "required": ["image", "bbox"],
    "properties": {
        "image": {
            "type": "string",
            "description": "The image to place the region on: its image id (candidate) or its raw "
            "frame id (raw:candidate).",
        },
        "bbox": {
            "type": "array",
            "items": {"type": "number"},
            "minItems": 4,
            "maxItems": 4,
            "description": "Normalised [x0, y0, x1, y1] in the image's raw frame.",
        },
        "frame": {
            "type": "string",
            "description": "A frame to cut the box from and return: a frame id (raw:candidate, or "
            "a crop a tool returned), or an image id (candidate) for its raw frame; the image's raw "
            "frame when absent.",
        },
    },
}

ECHO_REGION = {
    "manifest_version": 1,
    "name": "echo_region",
    "version": __version__,
    "purpose": "Echoes a box on an image as a change region and returns the box cut from a frame, to prove the pack transport.",
    "when_to_use": "Only to exercise the tool pack.",
    "when_not_to_use": "For any judgement about an image.",
    "output_guidance": "The region is exactly the box given and the image is exactly its pixels; neither says anything about the image.",
    "cost": {"tier": "cheap", "seconds": 0.0},
    "requires": [],
    "pairs_with": [],
    "image_args": ["image"],
    "frame_args": ["frame"],
    "accepts_hints": [],
    "evidence_kind": "change_region",
    "returns": RETURNS,
    "frames_out": ["crop"],
    "concurrent": False,
    "preconditions": "",
    "model_schema": ECHO_REGION_MODEL_SCHEMA,
    "input_schema": {
        **ECHO_REGION_MODEL_SCHEMA,
        "required": ["image", "bbox", "_squint"],
        "properties": {**ECHO_REGION_MODEL_SCHEMA["properties"], "_squint": {"type": "object"}},
    },
    "output_schema": {"type": "object"},
}

MANIFESTS = [ECHO_REGION, regions.MANIFEST, classes.MANIFEST]


def _bbox(value: Any) -> list[float] | None:
    if (
        not isinstance(value, list)
        or len(value) != 4
        or not all(isinstance(v, (int, float)) and not isinstance(v, bool) for v in value)
        or not (0.0 <= value[0] < value[2] <= 1.0 and 0.0 <= value[1] < value[3] <= 1.0)
    ):
        return None
    return [float(v) for v in value]


def echo_region(args: dict[str, Any], ctx: Context) -> dict[str, Any]:
    """One region at the given box on the named image's raw frame, and the box cut from the named
    frame (or that raw frame) as this call's one crop."""
    image = args.get("image")
    if not isinstance(image, str) or image not in ctx.images:
        return invalid_args("echo_region", f"image: unknown image id {image!r}")
    bbox = _bbox(args.get("bbox"))
    if bbox is None:
        return invalid_args(
            "echo_region", "bbox: four numbers, 0 <= x0 < x1 <= 1 and 0 <= y0 < y1 <= 1"
        )
    parent = args.get("frame", f"raw:{image}")
    pixels = ctx.frame_pixels(parent) if isinstance(parent, str) else None
    if pixels is None:
        return invalid_args("echo_region", f"frame: unknown frame id {parent!r}")
    rgb = pixels.load()
    h, w = rgb.shape[:2]
    # The box snaps outward to whole pixels; the mapping records the box actually cut.
    x0, y0 = math.floor(bbox[0] * w), math.floor(bbox[1] * h)
    x1, y1 = (
        min(w, max(x0 + 1, math.ceil(bbox[2] * w))),
        min(h, max(y0 + 1, math.ceil(bbox[3] * h))),
    )
    crop = rgb[y0:y1, x0:x1]
    region = {
        "id": "r1",
        "location": bbox_location(f"raw:{image}", bbox),
        "label": "echo",
        "score": 1.0,
    }
    cut = png_image(
        f"crop:{ctx.call_id}:0",
        parent,
        {"bbox": [x0 / w, y0 / h, x1 / w, y1 / h]},
        crop,
    )
    return envelope(
        "echo_region",
        "ok",
        f"Echoed the box on raw:{image} and cut it from {parent} ({x1 - x0}×{y1 - y0}).",
        regions=[region],
        images=[cut],
    )


HANDLERS = {"echo_region": echo_region, "regions": regions.regions, "classes": classes.classes}
