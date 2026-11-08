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
# Pieces of one kind, thin along one axis, level across it and no further apart along it than
# this share of the longer side are parts of one moved edge and are scored as one (`chained`).
CHAIN_GAP = 0.08
ALL = "all"

# The all-classes scan. Classes the segmenter reads as one another when the light or the season
# changes: a piece of one that the other image reads as a member of the same group is a
# relabelling, not a change of shape, and scores `RELABEL` of what it would.
GROUPS = [
    {"building", "house", "skyscraper", "tower", "hovel", "wall", "column", "tank", "awning"},
    {
        "floor",
        "road",
        "sidewalk",
        "earth",
        "path",
        "land",
        "sand",
        "field",
        "grass",
        "dirt track",
        "step",
        "stairs",
        "stairway",
        "rug",
    },
    {"tree", "plant", "grass", "flower", "palm", "field"},
    {"water", "sea", "river", "lake", "swimming pool", "waterfall"},
    {"sky", "mountain", "hill", "ceiling"},
    {"mountain", "hill", "rock", "earth", "land"},
    {"windowpane", "door", "screen door", "glass", "mirror", "painting", "poster", "wall"},
    {"awning", "canopy", "tent", "booth"},
    {"car", "truck", "van", "bus", "minibike"},
    {"boat", "ship"},
    {"pole", "streetlight", "signboard", "traffic light", "flag"},
    {"signboard", "trade name", "poster", "bulletin board"},
    {"railing", "bannister", "fence", "bridge", "pier", "sidewalk", "stairs", "stairway"},
    {"cabinet", "shelf", "counter", "countertop", "case", "bookcase", "buffet", "box"},
]
RELABEL = 0.35
# What a region dominantly is in the other image: at least this share of its pixels.
DOMINANT = 0.6
# Ground, sky and water: the background a removed thing leaves and an added thing covers. A
# piece of one of these that sits mostly inside a piece of another class is the same change,
# seen from the other side; it is folded into that piece.
STUFF = {
    "sky",
    "floor",
    "road",
    "grass",
    "sidewalk",
    "earth",
    "field",
    "sand",
    "path",
    "land",
    "water",
    "sea",
    "river",
    "lake",
    "ceiling",
    "mountain",
    "hill",
    "dirt track",
    "wall",
    "tree",
}
FOLD = 0.5
# Things that move or get moved: furniture, vehicles, plants in pots, people. A shift of one is
# not a change of structure, so in the scan these, like the background classes, count only when
# a piece is much of the class (it appeared or went), never for area or edge movement alone.
MOVABLE = {
    "chair",
    "armchair",
    "swivel chair",
    "sofa",
    "seat",
    "stool",
    "ottoman",
    "bench",
    "table",
    "coffee table",
    "desk",
    "cushion",
    "pillow",
    "towel",
    "blanket",
    "person",
    "animal",
    "car",
    "truck",
    "van",
    "bus",
    "bicycle",
    "minibike",
    "boat",
    "plant",
    "pot",
    "flower",
    "vase",
    "basket",
    "bag",
    "box",
    "barrel",
    "bottle",
    "ball",
    "plaything",
    "food",
    "tray",
    "ashcan",
    "apparel",
    "lamp",
}
# Vegetation and thin structures: fronds and rails whose few pixels of mask shift between any
# two renders, and whose sparse edges make boundary movement jump. Also scored on whole-object
# change only.
SPARSE = {"palm", "railing", "fence", "bannister", "pole", "streetlight", "traffic light", "flag"}
# The scan names at most this many classes in its summary.
MAX_NAMED = 8


def related(a: str, b: str) -> bool:
    return a == b or any(a in g and b in g for g in GROUPS)


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
            "the like map to the nearest of these), or all to compare every class the frames show. "
            "Defaults to the first protected class the rules name that the frames show, and to "
            "all when they name none the frames show.",
        },
    },
}

MANIFEST = {
    "manifest_version": 1,
    "name": "outline",
    "version": "0.3.0",
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


def _in_line(a: list[float], b: list[float], h: int, w: int) -> bool:
    """Two piece boxes (fractions) that are thin along one axis, level with each other across it
    and at most `CHAIN_GAP` apart along it: parts of one moved edge."""
    ax0, ay0, ax1, ay1 = a[0] * w, a[1] * h, a[2] * w, a[3] * h
    bx0, by0, bx1, by1 = b[0] * w, b[1] * h, b[2] * w, b[3] * h
    gap, near = CHAIN_GAP * max(h, w), 2 * JITTER
    across = lambda lo0, hi0, lo1, hi1: min(hi0, hi1) - max(lo0, lo1) >= -near
    along = lambda lo0, hi0, lo1, hi1: max(lo0, lo1) - min(hi0, hi1) <= gap
    wide = (ax1 - ax0) >= 2 * (ay1 - ay0) and (bx1 - bx0) >= 2 * (by1 - by0)
    tall = (ay1 - ay0) >= 2 * (ax1 - ax0) and (by1 - by0) >= 2 * (bx1 - bx0)
    return (wide and across(ay0, ay1, by0, by1) and along(ax0, ax1, bx0, bx1)) or (
        tall and across(ax0, ax1, bx0, bx1) and along(ay0, ay1, by0, by1)
    )


def chained(pieces: list[dict[str, Any]], h: int, w: int) -> list[dict[str, Any]]:
    """Pieces of one kind in line along one edge, joined into one: an edge moved 10 px comes
    back from the segmenter as several slivers, each too small to score, that are one change."""
    groups = [[p] for p in pieces]
    merged = True
    while merged:
        merged = False
        for i in range(len(groups)):
            for j in range(i + 1, len(groups)):
                if groups[i][0]["kind"] != groups[j][0]["kind"]:
                    continue
                if any(_in_line(p["bbox"], q["bbox"], h, w) for p in groups[i] for q in groups[j]):
                    groups[i] += groups.pop(j)
                    merged = True
                    break
            if merged:
                break
    out = []
    for g in groups:
        if len(g) == 1:
            out.append(g[0])
            continue
        shifts = [p["shift_px"] for p in g if p["shift_px"] is not None]
        out.append(
            {
                "kind": g[0]["kind"],
                "bbox": [
                    min(p["bbox"][0] for p in g),
                    min(p["bbox"][1] for p in g),
                    max(p["bbox"][2] for p in g),
                    max(p["bbox"][3] for p in g),
                ],
                "area": sum(p["area"] for p in g),
                "shift_px": max(shifts) if shifts else None,
                "edge": any(p["edge"] for p in g),
                "object": max(p["object"] for p in g),
                "parts": len(g),
                "_at": (
                    np.concatenate([p["_at"][0] for p in g]),
                    np.concatenate([p["_at"][1] for p in g]),
                ),
            }
        )
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
                    # The piece's pixels, for the all-classes scan; dropped before reporting.
                    "_at": (ys, xs),
                }
            )
    pieces = chained(pieces, h, w)
    extent = max(a0, a1)
    for p in pieces:
        p["extent"] = extent
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


def label_map(
    scores: np.ndarray, placement: segmenter.Placement, size: tuple[int, int]
) -> np.ndarray:
    """Each pixel's best class on a (height, width) grid over the image."""
    lab = placement.crop(scores).argmax(0).astype(np.uint8)
    return np.asarray(Image.fromarray(lab, "L").resize((size[1], size[0]), Image.NEAREST))


def fold(pieces: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """One change seen from both sides is reported once. A piece that sits mostly inside a piece
    of another class of the opposite kind is the same change: a ground, sky or water piece goes
    into the thing (the sky a removed tower leaves, the wall a new shrub covers); between two
    things, the piece that is less of its own class goes into the one that is more of its own (a
    new sign over a shed: the sign is all new, the shed lost a part)."""
    w = 1 + max((int(p["_at"][1].max()) for p in pieces), default=0)
    flat = [p["_at"][0].astype(np.int64) * w + p["_at"][1] for p in pieces]
    share = [p["area"] / p["extent"] if p["extent"] else 1.0 for p in pieces]
    dropped = set()
    for i, p in enumerate(pieces):
        for j, q in enumerate(pieces):
            if j == i or j in dropped or q["kind"] == p["kind"] or q["class"] == p["class"]:
                continue
            stuff_p, stuff_q = p["class"] in STUFF, q["class"] in STUFF
            if stuff_p and stuff_q:
                continue
            if stuff_q or (not stuff_p and share[i] >= share[j]):
                continue
            if np.isin(flat[i], flat[j], assume_unique=True).mean() >= FOLD:
                dropped.add(i)
                break
    return [p for i, p in enumerate(pieces) if i not in dropped]


def scan(
    before: tuple[np.ndarray, segmenter.Placement],
    after: tuple[np.ndarray, segmenter.Placement],
    size: tuple[int, int],
    scale: float,
    names: list[str],
) -> tuple[list[tuple[str, dict[str, Any]]], list[dict[str, Any]]]:
    """Every class either image shows, compared: (class, comparison) for each, and the pieces of
    all of them, folded and scored, best first. Each piece carries its class, what the other
    image reads there, and whether that is a related class (a relabelling)."""
    (sb, pb), (sa, pa) = before, after
    lb, la = label_map(sb, pb, size), label_map(sa, pa, size)
    nb = np.bincount(lb.ravel(), minlength=len(names)) / lb.size
    na = np.bincount(la.ravel(), minlength=len(names)) / la.size
    compared, pieces = [], []
    for k in np.nonzero(np.maximum(nb, na) >= PRESENT)[0]:
        name = names[k]
        b, a = despeckled_pair(class_mask(sb, pb, k, size), class_mask(sa, pa, k, size))
        if max(b.mean(), a.mean()) < PRESENT:
            continue
        found = compare(b, a, scale)
        compared.append((name, found))
        for p in found["pieces"]:
            ys, xs = p["_at"]
            there = (la if p["kind"] == "removed" else lb)[ys, xs]
            votes = np.bincount(there, minlength=len(names))
            j = int(votes.argmax())
            p["class"] = name
            p["other"] = names[j] if votes[j] >= DOMINANT * len(ys) and j != k else None
            p["relabel"] = p["other"] is not None and related(name, p["other"])
            if name in STUFF or name in MOVABLE or name in SPARSE:
                p["score"] = (
                    round(min(1.0, p["area"] / p["extent"] / WHOLE), 3) if p["extent"] else 0.0
                )
            if p["relabel"]:
                p["score"] = round(p["score"] * RELABEL, 3)
            pieces.append(p)
    pieces = fold(pieces)
    pieces.sort(key=lambda p: (-p["score"], -p["area"]))
    return compared, pieces


def piece_label(p: dict[str, Any], cls: str) -> str:
    other = p.get("other")
    seen = ""
    if other:
        seen = f", now read as {other}" if p["kind"] == "removed" else f", where it was {other}"
    moved = (
        f", boundary moved up to {p['shift_px']:.0f} px"
        if p["shift_px"] is not None
        else ", with no edge of it in the other image"
    )
    return f"{cls} {p['kind']}{seen}{moved}"[:120]


def overlay_pieces(
    frame_rgb: np.ndarray, size: tuple[int, int], pieces: list[dict[str, Any]]
) -> np.ndarray:
    """The frame with each piece tinted (removed red, added green, a relabelling grey), its box
    and its id, at most 1024 on the longer side."""
    h, w = size
    img = np.asarray(Image.fromarray(frame_rgb).resize((w, h), Image.BILINEAR)).astype(np.float32)
    img *= 0.75
    for p in pieces:
        ys, xs = p["_at"]
        colour = (
            (150, 150, 150)
            if p["relabel"]
            else (255, 40, 40)
            if p["kind"] == "removed"
            else (40, 220, 40)
        )
        img[ys, xs] = 0.4 * img[ys, xs] + 0.6 * np.array(colour, np.float32)
    out = Image.fromarray(img.clip(0, 255).astype(np.uint8))
    draw = ImageDraw.Draw(out)
    for p in pieces:
        x0, y0, x1, y1 = p["bbox"]
        draw.rectangle([x0 * w, y0 * h, x1 * w - 1, y1 * h - 1], outline=(255, 255, 255), width=2)
        draw.text((x0 * w + 4, y0 * h + 2), f"{p['id']} {p['class']}", fill=(255, 255, 255))
    return np.asarray(out)


def outline_all(
    frame_id: str,
    against_id: str,
    frame_rgb: np.ndarray,
    scores: tuple[tuple[np.ndarray, segmenter.Placement], tuple[np.ndarray, segmenter.Placement]],
    size: tuple[int, int],
    scale: float,
    call_id: str,
    started: float,
) -> dict[str, Any]:
    names = segmenter.class_names()
    after, before = scores
    compared, pieces = scan(before, after, size, scale, names)
    if not compared:
        env = envelope(
            "outline",
            "measured_nothing",
            f"No class covers {PRESENT:.1%} of {frame_id} or {against_id}; nothing to compare.",
            version=MANIFEST["version"],
        )
        env["provenance"]["duration_ms"] = int((time.monotonic() - started) * 1000)
        return env
    kept = pieces[:MAX_REGIONS]
    for n, p in enumerate(kept, 1):
        p["id"] = f"r{n}"
    real = [p for p in pieces if not p["relabel"]]
    worst = min(compared, key=lambda c: c[1]["iou"])
    measurements = [
        {"name": n, "value": round(v, 4), "unit": u, "frame": frame_id}
        for n, v, u in (
            ("classes", float(len(compared)), "count"),
            ("min_iou", worst[1]["iou"], "ratio"),
            ("pieces", float(sum(p["score"] >= 0.5 for p in pieces)), "count"),
            ("added", sum(p["area"] for p in real if p["kind"] == "added"), "ratio"),
            ("removed", sum(p["area"] for p in real if p["kind"] == "removed"), "ratio"),
            ("relabelled", sum(p["area"] for p in pieces if p["relabel"]), "ratio"),
        )
    ]
    regions = [
        {
            "id": p["id"],
            "location": bbox_location(frame_id, p["bbox"]),
            "label": piece_label(p, p["class"]),
            "score": p["score"],
        }
        for p in kept
    ]
    by_size = sorted(compared, key=lambda c: -max(c[1]["area_before"], c[1]["area_after"]))

    def class_words(c: tuple[str, dict[str, Any]]) -> str:
        name, f = c
        if not f["area_after"]:
            return f"{name} gone"
        if not f["area_before"]:
            return f"{name} new"
        return f"{name} {f['iou']:.2f} ({f['area_change']:+.0%})"

    least = sorted(compared, key=lambda c: c[1]["iou"])[:MAX_NAMED]
    summary = (
        f"Every class on {frame_id} against {against_id}: compared {len(compared)} "
        f"({', '.join(c[0] for c in by_size[:MAX_NAMED])}"
        + (", ..." if len(by_size) > MAX_NAMED else "")
        + "). Least overlap (IoU, area change): "
        + "; ".join(class_words(c) for c in least)
        + "."
    )
    if kept:
        summary += (
            " "
            + "; ".join(
                f"{p['id']} {p['class']} {p['kind']} ({p['area']:.2%} of the frame"
                + (", relabelled" if p["relabel"] else "")
                + ")"
                for p in kept
            )
            + ". A relabelled piece is one the other image reads as a related class (house for "
            "building, road for pavement): usually the light or season, not the shape. In the "
            "image: removed red, added green, relabelled grey."
        )
    else:
        summary += " No piece of any class was added or removed beyond boundary noise."
    images = [
        png_image(
            f"crop:{call_id}:0",
            frame_id,
            {"bbox": [0.0, 0.0, 1.0, 1.0]},
            overlay_pieces(frame_rgb, size, kept),
        )
    ]
    env = envelope(
        "outline",
        "ok",
        summary[: RETURNS["max_summary_bytes"]],
        regions=regions,
        measurements=measurements,
        images=images,
        version=MANIFEST["version"],
    )
    env["provenance"]["duration_ms"] = int((time.monotonic() - started) * 1000)
    return env


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
    if asked is not None and (not isinstance(asked, str) or (asked not in names and asked != ALL)):
        return invalid_args(
            "outline",
            f"class: {said!r} is not a segmenter class; for example swimming pool, building, "
            "house, tower, bridge, column, wall, or all",
        )
    frame_rgb, against_rgb = frame_px.load(), against_px.load()
    (sf, pf), (sa, pa) = segmenter.shared().scores([frame_rgb, against_rgb])
    h, w = frame_rgb.shape[:2]
    size = (round(WORK * h / max(h, w)), round(WORK * w / max(h, w)))
    scale = max(h, w) / WORK

    def masks(name: str) -> tuple[np.ndarray, np.ndarray]:
        k = names.index(name)
        return despeckled_pair(class_mask(sa, pa, k, size), class_mask(sf, pf, k, size))

    def everything() -> dict[str, Any]:
        return outline_all(
            frame_id, against_id, frame_rgb, ((sf, pf), (sa, pa)), size, scale, ctx.call_id, started
        )

    if asked == ALL:
        return everything()

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
            # Nothing named, or nothing named is in the frames: compare every class instead.
            return everything()
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
