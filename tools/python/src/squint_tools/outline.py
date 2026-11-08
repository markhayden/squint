"""`outline`: how one protected class's shape differs between an aligned pair.

`regions` finds where pixels changed; it does not say that a pool got wider or that its end was
redrawn round, because those show up as scattered slivers among ripples and reflections. This
tool segments one class (a swimming pool, a building) on both frames and compares the two
shapes: their overlap, their areas, how far the boundary moved and where, and the pieces added
or removed as regions on `frame`.

The class's extent is read from the segmenter's per-class scores (the class's score against the
best other class), upsampled bilinearly to the working grid before thresholding, so the boundary
falls between the segmenter's 4-pixel cells rather than on them. Boundary on the frame's own edge
is not boundary: a pool running off the image has no outline there.
"""

from __future__ import annotations

import time
from typing import Any

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

from . import segmenter
from .context import Context
from .envelope import bbox_location, envelope, invalid_args, png_image
from .regions import components

# The longer side of the grid the masks are compared on.
WORK = 1024
# A class covering less than this share of both frames is absent when the tool picks the class
# itself from the rules' hints; a class the caller names may be small (a clock face, a lamp post)
# and needs only `PRESENT_NAMED`.
PRESENT = 0.005
PRESENT_NAMED = 0.0005
# Added or removed pieces thinner than this many working pixels are boundary jitter: the mask
# difference is opened by a square of this side before components are taken.
JITTER = 5
# Fragments of a class mask are segmentation specks (a cushion read as pool) and are removed
# before the shapes are compared when they are smaller than `SPECK_REL` of the class's largest
# piece in either image, or than `SPECK_ABS` of the frame. Relative, so a pool loses its specks
# and a clock face, the whole of its class, survives; one bar for both images, so a fragment is
# kept or dropped on both sides alike.
SPECK_REL = 0.03
SPECK_ABS = 0.0003
# A piece that is this share of the class's extent (in the larger of the two images) is a
# change to the object itself, whatever its size in the frame: a removed bench scores 1.
WHOLE = 0.3
# A piece touching the frame's edge, smaller than `EDGE_SMALL` of the frame, and part of an
# object no bigger than `EDGE_OBJECT` of the frame is usually a partial object at the border cut
# differently in the two images (a shed half out of shot); its score is halved. A large object's
# real change at the edge (a pool running out of frame, its end redrawn) is not.
EDGE_SMALL = 0.005
EDGE_OBJECT = 0.02
# Pieces smaller than this share of the frame are dropped.
MIN_AREA = 0.0005
# Boundary points compared per side, at most (the distance matrix is chunked).
MAX_POINTS = 4000
MAX_REGIONS = 6

# Words for structures the segmenter has no class of its own for, and the class that covers them.
ALIASES = {
    "barn": "building",
    "silo": "building",
    "chimney": "building",
    "cottage": "house",
    "shed": "hovel",
    "hut": "hovel",
    "cabana": "hovel",
    "deck": "floor",
    "patio": "floor",
    "terrace": "floor",
    "pool": "swimming pool",
    "window": "windowpane",
    "lamp post": "streetlight",
    "street lamp": "streetlight",
    "narrowboat": "boat",
}

RETURNS = {
    "max_images": 1,
    "max_long_edge": 1024,
    "max_image_bytes": 4_000_000,
    "max_summary_bytes": 2048,
    "max_regions": MAX_REGIONS,
    "max_region_label_bytes": 120,
    "max_measurements": 8,
    "max_transforms": 0,
    "max_envelope_bytes": 6_000_000,
}

MODEL_SCHEMA = {
    "type": "object",
    "additionalProperties": False,
    "required": ["frame", "against"],
    "properties": {
        "frame": {
            "type": "string",
            "description": "The frame whose shape is compared, usually the aligned candidate "
            "(aligned:candidate:t1): a frame id, or an image id (candidate) for its raw frame "
            "(raw:candidate); regions are reported on it. When align could not register a pair "
            "that is plainly the same framing (a strong restyle), raw:candidate works too.",
        },
        "against": {
            "type": "string",
            "description": "The frame it is compared with, in the same grid, usually the source: "
            "a frame id (raw:source), or an image id (source) for its raw frame.",
        },
        "class": {
            "type": "string",
            "description": "The scene class whose outline to compare, as the segmenter names it "
            "(swimming pool, building, house, tower, bridge, column, wall; barn, silo, deck and "
            "the like map to the nearest of these). Defaults to the first protected class the "
            "rules name that the frames show.",
        },
    },
}

MANIFEST = {
    "manifest_version": 1,
    "name": "outline",
    "version": "0.2.0",
    "purpose": "Compares the shape of one protected class (a pool, a building) between an "
    "aligned pair: overlap, area, how far its boundary moved and where it grew or shrank.",
    "when_to_use": "After align, to check that permanent structure kept its shape and size: "
    "a pool widened, an end rounded, a building extended. regions sees pixels change; this "
    "sees the shape change.",
    "when_not_to_use": "For things that may move (furniture, people); on a pair align could "
    "not register; to judge texture or colour.",
    "output_guidance": "Shifts are in pixels of the frame. A few pixels of boundary movement "
    "is segmentation noise; tens of pixels along one stretch, or an added or removed piece, "
    "is a change in shape. Each region is a pointer: confirm it with view.",
    "cost": {"tier": "expensive", "seconds": 4.0},
    "requires": ["align"],
    "pairs_with": ["view", "regions"],
    "image_args": [],
    "frame_args": ["frame", "against"],
    "accepts_hints": ["outline"],
    "evidence_kind": "change_region",
    "returns": RETURNS,
    "frames_out": ["crop"],
    "concurrent": False,
    "preconditions": "`frame` and `against` share one grid: an aligned frame and the frame it "
    "was aligned onto, or two raw frames of the same size and framing when align could not "
    "register a restyled pair.",
    "model_schema": MODEL_SCHEMA,
    "input_schema": {
        **MODEL_SCHEMA,
        "required": ["frame", "against", "_squint"],
        "properties": {**MODEL_SCHEMA["properties"], "_squint": {"type": "object"}},
    },
    "output_schema": {
        "type": "object",
        "properties": {
            "measurements": {
                "type": "array",
                "description": "iou (ratio), area_before and area_after (ratio of the frame), "
                "area_change (ratio, after over before less one), shift_p95_px and shift_max_px "
                "(how far the boundary moved, in frame pixels), added and removed (ratio of the "
                "frame).",
            }
        },
    },
}


def _resize_f(a: np.ndarray, size: tuple[int, int]) -> np.ndarray:
    """A float map resized bilinearly to (height, width)."""
    return np.asarray(
        Image.fromarray(a.astype(np.float32), "F").resize((size[1], size[0]), Image.BILINEAR)
    )


def class_mask(
    scores: np.ndarray, placement: segmenter.Placement, k: int, size: tuple[int, int]
) -> np.ndarray:
    """Where class `k` beats every other class, on a (height, width) grid over the image."""
    seg = placement.crop(scores)
    own = seg[k]
    others = np.delete(seg, k, axis=0).max(0)
    return _resize_f(own - others, size) > 0.0


def _areas(mask: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    labels, n = components(mask)
    return labels, np.bincount(labels.ravel(), minlength=n + 1) / mask.size


def despeckled(mask: np.ndarray, bar: float | None = None) -> np.ndarray:
    """The mask without fragments under `bar` of the frame; by default the bar is relative to
    the mask's own largest fragment (see `SPECK_REL`)."""
    labels, area = _areas(mask)
    if len(area) <= 1:
        return mask
    if bar is None:
        bar = max(SPECK_ABS, SPECK_REL * float(area[1:].max()))
    keep = area >= bar
    keep[0] = False
    return keep[labels]


def despeckled_pair(before: np.ndarray, after: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """Both masks under one speck bar, from the largest fragment of either."""
    largest = max(
        (float(a[1:].max()) for a in (_areas(before)[1], _areas(after)[1]) if len(a) > 1),
        default=0.0,
    )
    bar = max(SPECK_ABS, SPECK_REL * largest)
    return despeckled(before, bar), despeckled(after, bar)


def _filter(mask: np.ndarray, f: ImageFilter.Filter) -> np.ndarray:
    return np.asarray(Image.fromarray(mask.astype(np.uint8) * 255).filter(f)) > 0


def opened(mask: np.ndarray, side: int) -> np.ndarray:
    return _filter(_filter(mask, ImageFilter.MinFilter(side)), ImageFilter.MaxFilter(side))


def boundary(mask: np.ndarray) -> np.ndarray:
    """The mask's own edge pixels, without those on the frame's edge."""
    edge = mask & ~_filter(mask, ImageFilter.MinFilter(3))
    edge[:2, :] = edge[-2:, :] = False
    edge[:, :2] = edge[:, -2:] = False
    return edge


def _points(edge: np.ndarray) -> np.ndarray:
    ys, xs = np.nonzero(edge)
    pts = np.stack([xs, ys], 1).astype(np.float32)
    if len(pts) > MAX_POINTS:
        pts = pts[np.linspace(0, len(pts) - 1, MAX_POINTS).astype(int)]
    return pts


def nearest(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    """For each point of `a`, the distance to the nearest point of `b` (inf when `b` is empty)."""
    if len(b) == 0:
        return np.full(len(a), np.inf, np.float32)
    out = np.empty(len(a), np.float32)
    for i in range(0, len(a), 512):
        d = a[i : i + 512, None, :] - b[None, :, :]
        out[i : i + 512] = np.sqrt((d**2).sum(-1).min(1))
    return out


def compare(before: np.ndarray, after: np.ndarray, scale: float) -> dict[str, Any]:
    """Two masks on one grid: measurements, and the added and removed pieces as boxes with the
    largest boundary shift inside each. `scale` converts grid pixels to frame pixels."""
    h, w = before.shape
    inter = float((before & after).sum())
    union = float((before | after).sum())
    a0, a1 = float(before.mean()), float(after.mean())
    pb, pa = _points(boundary(before)), _points(boundary(after))
    # Symmetric: how far the new boundary is from the old, and the old from the new, so a piece
    # removed shows as much as a piece added.
    da, db = nearest(pa, pb), nearest(pb, pa)
    shifts = np.concatenate([da, db])
    pts = np.concatenate([pa, pb])
    finite = np.isfinite(shifts)
    out: dict[str, Any] = {
        "iou": inter / union if union else 1.0,
        "area_before": a0,
        "area_after": a1,
        "area_change": (a1 / a0 - 1.0) if a0 else (0.0 if not a1 else float("inf")),
        "shift_p95_px": float(np.percentile(shifts[finite], 95)) * scale if finite.any() else 0.0,
        "shift_max_px": float(shifts[finite].max()) * scale if finite.any() else 0.0,
    }
    # The objects of the class, either image's extent joined: what each piece is part of.
    whole_labels, whole_areas = _areas(before | after)
    pieces = []
    for kind, diff in (("added", after & ~before), ("removed", before & ~after)):
        diff = opened(diff, JITTER)
        out[kind] = float(diff.mean())
        labels, n = components(diff)
        for k in range(1, n + 1):
            ys, xs = np.nonzero(labels == k)
            area = len(ys) / (h * w)
            if area < MIN_AREA:
                continue
            x0, x1, y0, y1 = xs.min(), xs.max() + 1, ys.min(), ys.max() + 1
            inside = (pts[:, 0] >= x0) & (pts[:, 0] < x1) & (pts[:, 1] >= y0) & (pts[:, 1] < y1)
            local = shifts[inside & finite]
            pieces.append(
                {
                    "kind": kind,
                    "bbox": [x0 / w, y0 / h, x1 / w, y1 / h],
                    "area": area,
                    # None: no edge of the other image near it to measure against (the class is
                    # gone from, or new to, that part of the frame).
                    "shift_px": float(local.max()) * scale if len(local) else None,
                    "edge": bool(x0 <= 1 or y0 <= 1 or x1 >= w - 1 or y1 >= h - 1),
                    "object": float(whole_areas[np.unique(whole_labels[ys, xs])].max()),
                }
            )
    extent = max(a0, a1)
    for p in pieces:
        # Large or far-moved pieces first: a sliver along a long edge that moved 30 px matters
        # as much as a blob; and a piece that is much of the object is a change to the object.
        moved = p["shift_px"] or 0.0
        score = 0.5 * min(1.0, p["area"] / 0.005) + 0.5 * min(1.0, moved / 30.0)
        if extent:
            score = max(score, min(1.0, p["area"] / extent / WHOLE))
        if p["edge"] and p["area"] < EDGE_SMALL and p["object"] <= EDGE_OBJECT:
            score *= 0.5
        p["score"] = round(min(1.0, score), 3)
    pieces.sort(key=lambda p: (-p["score"], -p["area"]))
    out["pieces"] = pieces
    return out


def overlay(
    frame_rgb: np.ndarray, before: np.ndarray, after: np.ndarray, pieces: list[dict[str, Any]]
) -> np.ndarray:
    """The frame with the removed area tinted red, the added area green, the old outline cyan,
    the new outline yellow and each piece's box and id, at most 1024 on the longer side."""
    h, w = before.shape
    img = np.asarray(Image.fromarray(frame_rgb).resize((w, h), Image.BILINEAR)).astype(np.float32)
    for mask, colour in ((before & ~after, (255, 40, 40)), (after & ~before, (40, 220, 40))):
        img[mask] = 0.45 * img[mask] + 0.55 * np.array(colour, np.float32)
    thick = lambda m: _filter(boundary(m), ImageFilter.MaxFilter(3))
    img[thick(before)] = (0, 230, 255)
    img[thick(after)] = (255, 230, 0)
    out = Image.fromarray(img.clip(0, 255).astype(np.uint8))
    draw = ImageDraw.Draw(out)
    for p in pieces:
        x0, y0, x1, y1 = p["bbox"]
        draw.rectangle([x0 * w, y0 * h, x1 * w - 1, y1 * h - 1], outline=(255, 255, 255), width=2)
        draw.text((x0 * w + 4, y0 * h + 2), p["id"], fill=(255, 255, 255))
    return np.asarray(out)


def outline(args: dict[str, Any], ctx: Context) -> dict[str, Any]:
    started = time.monotonic()
    ids = [args.get(k) for k in ("frame", "against")]
    if not all(isinstance(i, str) for i in ids):
        return invalid_args("outline", "frame and against must be frame ids")
    frame_id, against_id = ids
    if frame_id == against_id:
        return invalid_args("outline", "frame and against must be two different frames")
    pair = [ctx.frame_pixels(i) for i in ids]
    for key, i, p in zip(("frame", "against"), ids, pair):
        if p is None:
            return invalid_args("outline", f"{key}: unknown frame id {i!r}")
    frame_px, against_px = pair
    aspect = lambda p: p.width / max(p.height, 1)
    if (
        frame_px.projection != against_px.projection
        or abs(aspect(frame_px) / aspect(against_px) - 1.0) > 0.01
    ):
        return invalid_args(
            "outline",
            f"{frame_id!r} and {against_id!r} must share one grid (projection and aspect)",
        )
    names = segmenter.class_names()
    asked = said = args.get("class")
    if isinstance(asked, str):
        asked = ALIASES.get(asked.strip().lower(), asked.strip().lower())
    if asked is not None and (not isinstance(asked, str) or asked not in names):
        return invalid_args(
            "outline",
            f"class: {said!r} is not a segmenter class; for example swimming pool, building, "
            "house, tower, bridge, column, wall",
        )
    frame_rgb, against_rgb = frame_px.load(), against_px.load()
    (sf, pf), (sa, pa) = segmenter.shared().scores([frame_rgb, against_rgb])
    h, w = frame_rgb.shape[:2]
    size = (round(WORK * h / max(h, w)), round(WORK * w / max(h, w)))
    scale = max(h, w) / WORK

    def masks(name: str) -> tuple[np.ndarray, np.ndarray]:
        k = names.index(name)
        return despeckled_pair(class_mask(sa, pa, k, size), class_mask(sf, pf, k, size))

    hints = ctx.hints.get("outline") if isinstance(ctx.hints, dict) else None
    hinted = (
        [c for c in (hints or {}).get("classes", []) if c in names]
        if isinstance(hints, dict)
        else []
    )
    if asked is not None:
        chosen = asked
        before, after = masks(chosen)
    else:
        chosen = None
        for name in hinted:
            before, after = masks(name)
            if max(before.mean(), after.mean()) >= PRESENT:
                chosen = name
                break
        if chosen is None:
            return invalid_args(
                "outline",
                "class: no class to compare; name one, for example swimming pool"
                + (f" (none of {', '.join(hinted)} is in the frames)" if hinted else ""),
            )
    bar = PRESENT_NAMED if asked is not None else PRESENT
    if max(before.mean(), after.mean()) < bar:
        env = envelope(
            "outline",
            "measured_nothing",
            f"No {chosen} on {frame_id} or {against_id} (under {bar:.2%} of either frame); "
            "nothing to compare.",
            version=MANIFEST["version"],
        )
        env["provenance"]["duration_ms"] = int((time.monotonic() - started) * 1000)
        return env

    found = compare(before, after, scale)
    kept = found["pieces"][:MAX_REGIONS]
    for n, p in enumerate(kept, 1):
        p["id"] = f"r{n}"
    measurements = [
        {
            "name": k,
            "value": round(found[k], 4),
            "unit": "px" if k.endswith("_px") else "ratio",
            "frame": frame_id,
        }
        for k in (
            "iou",
            "area_before",
            "area_after",
            "area_change",
            "shift_p95_px",
            "shift_max_px",
            "added",
            "removed",
        )
    ]
    regions = [
        {
            "id": p["id"],
            "location": bbox_location(frame_id, p["bbox"]),
            "label": (
                f"{chosen} {p['kind']}, boundary moved up to {p['shift_px']:.0f} px"
                if p["shift_px"] is not None
                else f"{chosen} {p['kind']}, with no edge of it in the other image"
            )[:120],
            "score": p["score"],
        }
        for p in kept
    ]
    images = [
        png_image(
            f"crop:{ctx.call_id}:0",
            frame_id,
            {"bbox": [0.0, 0.0, 1.0, 1.0]},
            overlay(frame_rgb, before, after, kept),
        )
    ]
    named = chosen
    if isinstance(said, str) and said.strip().lower() != chosen:
        named = f"{said} (compared as {chosen})"
    absent = "before" if not found["area_before"] else "after" if not found["area_after"] else None
    change = (
        ("gone from the candidate" if absent == "after" else "new in the candidate")
        if absent
        else f"{found['area_change']:+.1%}"
    )
    moved = (
        "boundary movement does not apply (the class is in only one image)"
        if absent
        else f"boundary moved {found['shift_p95_px']:.0f} px at the 95th percentile, "
        f"{found['shift_max_px']:.0f} px at most"
    )
    summary = (
        f"{named} on {frame_id} against {against_id}: overlap (IoU) {found['iou']:.2f}; area "
        f"{found['area_before']:.1%} -> {found['area_after']:.1%} of the frame ({change}); {moved}."
    )

    def piece_words(p: dict[str, Any]) -> str:
        at = f", up to {p['shift_px']:.0f} px" if p["shift_px"] is not None else ""
        return f"{p['id']} {p['kind']} ({p['area']:.2%} of the frame{at})"

    if kept:
        summary += (
            " "
            + "; ".join(piece_words(p) for p in kept)
            + ". In the image: removed red, added green, old outline cyan, new outline yellow."
        )
    else:
        summary += " No piece was added or removed beyond boundary noise."
    env = envelope(
        "outline",
        "ok",
        summary,
        regions=regions,
        measurements=measurements,
        images=images,
        version=MANIFEST["version"],
    )
    env["provenance"]["duration_ms"] = int((time.monotonic() - started) * 1000)
    return env
