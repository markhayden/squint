"""`regions`: where an aligned pair differs, as high-recall pointers (PLAN task 5.4a).

Two frames in one grid (an aligned candidate and the source it was registered onto, or any pair
`align` produced) are compared two ways and the union is reported: a photometric change map (the
absolute difference of the locally contrast-normalised luminances, as `change_map` computes it)
and the segmenter's class disagreement where both sides are confident. Connected components of
the combined map become boxed regions on `frame`, labelled by the majority class on each side
("floor -> swimming pool"), scored by how strongly they changed, nudged by `hints.regions`
(`preserve` classes up, `ignore` classes down, never dropped), and capped at the manifest's
`max_regions` by score. One image comes back: `frame` with the boxes drawn.
"""

from __future__ import annotations

import time
from typing import Any

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

from . import segmenter
from .context import Context
from .envelope import bbox_location, envelope, invalid_args, png_image

MAX_REGIONS = 12
# The grid the change maps are compared on; the segmenter's maps (128 over the square input) are
# brought to it by nearest neighbour.
GRID = 256
# A normalised luminance residual beyond this counts as changed; it saturates the score.
CHANGED = 1.0
SATURATION = 3.0
# A blurred colour difference (0 to 255) beyond this counts as changed on its own.
COLOUR_CHANGED = 24.0
# Both sides must be at least this sure of their class for a disagreement to count.
CONFIDENT = 0.5
# Components smaller than this share of the frame are noise.
MIN_AREA = 0.001

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
            "description": "The frame to find changes in, usually the aligned candidate "
            "(aligned:candidate:t1): a frame id, or an image id (candidate) for its raw frame "
            "(raw:candidate); regions are reported on it.",
        },
        "against": {
            "type": "string",
            "description": "The frame it is compared with, in the same grid, usually the source: "
            "a frame id (raw:source), or an image id (source) for its raw frame.",
        },
    },
}

MANIFEST = {
    "manifest_version": 1,
    "name": "regions",
    "version": "1.0.0",
    "purpose": "Finds regions that changed between an aligned pair, with high recall: "
    "photometric change and segmentation class disagreement, boxed and labelled.",
    "when_to_use": "After align registers the candidate onto the source, before judging "
    "structure: to learn where to look.",
    "when_not_to_use": "Without a source image, or on a pair align could not register; the "
    "frames must share one grid.",
    "output_guidance": "Each region is a pointer, not a verdict: a change the rules allow "
    "shows up like one they forbid. Inspect regions with view before deciding.",
    "cost": {"tier": "expensive", "seconds": 5.0},
    "requires": ["align"],
    "pairs_with": ["view", "classes"],
    "image_args": [],
    "frame_args": ["frame", "against"],
    "accepts_hints": ["regions"],
    "evidence_kind": "change_region",
    "returns": RETURNS,
    "frames_out": ["crop"],
    "concurrent": False,
    "preconditions": "`frame` and `against` share one grid: an aligned frame and the frame it "
    "was aligned onto.",
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
                "description": "changed_fraction (ratio of the frame the combined map marks), "
                "photometric_fraction (ratio), class_disagreement (ratio of confident pixels "
                "whose class differs), candidates (count of components before the cap).",
            }
        },
    },
}


def box_blur(a: np.ndarray, radius: int) -> np.ndarray:
    """The mean over a (2r+1)² window, edges clamped, by summed-area table."""
    pad = np.pad(a, radius, mode="edge")
    s = np.zeros((pad.shape[0] + 1, pad.shape[1] + 1), np.float64)
    s[1:, 1:] = pad.cumsum(0).cumsum(1)
    k = 2 * radius + 1
    h, w = a.shape
    out = s[k : k + h, k : k + w] - s[:h, k : k + w] - s[k : k + h, :w] + s[:h, :w]
    return (out / (k * k)).astype(np.float32)


def normalised_luminance(rgb: np.ndarray, radius: int = 8) -> np.ndarray:
    """Luminance with the local mean removed and the local contrast divided out."""
    lum = rgb.astype(np.float32) @ np.array([0.299, 0.587, 0.114], np.float32)
    mean = box_blur(lum, radius)
    var = box_blur((lum - mean) ** 2, radius)
    return (lum - mean) / np.sqrt(var + 25.0)


def components(mask: np.ndarray) -> tuple[np.ndarray, int]:
    """4-connected components of a boolean mask: labels (0 is background) and their count.

    Two passes over the runs of each row with a union-find, so the work is proportional to the
    number of runs, not to the length of any component's path: a thin serpentine outline costs
    the same as a blob of the same run count.
    """
    h, w = mask.shape
    parent: list[int] = [0]

    def find(i: int) -> int:
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    def union(a: int, b: int) -> None:
        a, b = find(a), find(b)
        if a != b:
            parent[max(a, b)] = min(a, b)

    runs: list[list[tuple[int, int, int]]] = []  # per row: (x0, x1, label)
    for y in range(h):
        row = mask[y]
        edges = np.flatnonzero(np.diff(np.concatenate(([False], row, [False]))))
        current = []
        for x0, x1 in zip(edges[0::2], edges[1::2]):
            label = len(parent)
            parent.append(label)
            if y > 0:
                for px0, px1, plabel in runs[y - 1]:
                    if px0 < x1 and x0 < px1:
                        union(label, plabel)
            current.append((int(x0), int(x1), label))
        runs.append(current)
    roots = {i: find(i) for i in range(1, len(parent))}
    dense = {root: n for n, root in enumerate(sorted(set(roots.values())), 1)}
    labels = np.zeros((h, w), np.int32)
    for y, row_runs in enumerate(runs):
        for x0, x1, label in row_runs:
            labels[y, x0:x1] = dense[roots[label]]
    return labels, len(dense)


def upsample(grid: np.ndarray, size: tuple[int, int]) -> np.ndarray:
    """Nearest-neighbour resampling of a 2-D map to (height, width)."""
    h, w = size
    ys = (np.arange(h) * grid.shape[0] / h).astype(int)
    xs = (np.arange(w) * grid.shape[1] / w).astype(int)
    return grid[ys[:, None], xs[None, :]]


def _working(rgb: np.ndarray, size: tuple[int, int]) -> np.ndarray:
    return np.asarray(Image.fromarray(rgb).resize((size[1], size[0]), Image.BILINEAR))


def find(
    frame_rgb: np.ndarray,
    against_rgb: np.ndarray,
    seg_frame: tuple[np.ndarray, np.ndarray],
    seg_against: tuple[np.ndarray, np.ndarray],
    hints: dict[str, Any] | None,
    names: list[str],
) -> tuple[list[dict[str, Any]], dict[str, float]]:
    """Regions on the frame (normalised boxes, labels, scores, sorted by score) and the
    measurements behind them. The segmentations are (labels, confidence) over the frame only."""
    h, w = frame_rgb.shape[:2]
    size = (GRID, max(1, round(GRID * w / h))) if h >= w else (max(1, round(GRID * h / w)), GRID)
    fa, fb = _working(frame_rgb, size), _working(against_rgb, size)
    # Two photometric views: the locally contrast-normalised residual, which sees texture change
    # through a lighting change, and the plain colour difference of the blurred images, which
    # sees a flat area replaced by another flat colour (the normalised residual is zero inside
    # both). Either marks a pixel changed; the normalised residual scores it.
    a, b = normalised_luminance(fa), normalised_luminance(fb)
    residual = np.abs(a - b)
    colour = np.stack(
        [
            np.abs(
                box_blur(fa[..., c].astype(np.float32), 2)
                - box_blur(fb[..., c].astype(np.float32), 2)
            )
            for c in range(3)
        ]
    ).max(0)
    photometric = (residual > CHANGED) | (colour > COLOUR_CHANGED)
    residual = np.maximum(residual, colour / COLOUR_CHANGED)

    la, ca = (upsample(x, size) for x in seg_frame)
    lb, cb = (upsample(x, size) for x in seg_against)
    confident = (ca >= CONFIDENT) & (cb >= CONFIDENT)
    disagree = (la != lb) & confident

    # Two passes so a photometric box stays tight: the photometric components first, then the
    # confident class disagreements that no photometric box already covers. Each map is closed
    # by a 3×3 dilation and opened by a 5×5 erosion-dilation to drop specks.
    def clean(mask: np.ndarray) -> np.ndarray:
        img = Image.fromarray(mask.astype(np.uint8) * 255)
        img = img.filter(ImageFilter.MaxFilter(3)).filter(ImageFilter.MinFilter(5))
        return np.asarray(img.filter(ImageFilter.MaxFilter(5))) > 0

    photometric, disagree = clean(photometric), clean(disagree)
    changed = photometric | disagree
    hints = hints if isinstance(hints, dict) else {}
    preserve = set(hints.get("preserve") or [])
    ignore = set(hints.get("ignore") or [])
    found: list[dict[str, Any]] = []
    cells = size[0] * size[1]
    for source_mask, by_class in ((photometric, False), (disagree, True)):
        labels, n = components(source_mask)
        if n == 0:
            continue
        flat = labels.ravel()
        # Every per-component figure in one pass over the grid: areas, boxes, residual and
        # disagreement sums, and the majority class on each side.
        area = np.bincount(flat, minlength=n + 1)[1:] / cells
        ys, xs = np.divmod(np.arange(cells), size[1])
        x_min = np.full(n + 1, size[1])
        np.minimum.at(x_min, flat, xs)
        x_max = np.zeros(n + 1, int)
        np.maximum.at(x_max, flat, xs)
        y_min = np.full(n + 1, size[0])
        np.minimum.at(y_min, flat, ys)
        y_max = np.zeros(n + 1, int)
        np.maximum.at(y_max, flat, ys)
        residual_sum = np.bincount(flat, weights=residual.ravel(), minlength=n + 1)
        disagree_sum = np.bincount(flat, weights=disagree.ravel().astype(float), minlength=n + 1)
        count = np.bincount(flat, minlength=n + 1).astype(float)
        classes = len(names)
        majority = {}
        for side, side_labels in (("before", lb), ("after", la)):
            joint = np.bincount(flat * classes + side_labels.ravel(), minlength=(n + 1) * classes)
            majority[side] = joint.reshape(n + 1, classes).argmax(1)
        for k in np.flatnonzero(area >= MIN_AREA) + 1:
            box = [
                float(x_min[k] / size[1]),
                float(y_min[k] / size[0]),
                float((x_max[k] + 1) / size[1]),
                float((y_max[k] + 1) / size[0]),
            ]
            if by_class and any(_iou(box, r["bbox"]) > 0.3 for r in found):
                continue
            strength = float(np.clip(residual_sum[k] / count[k] / SATURATION, 0.0, 1.0))
            share = float(disagree_sum[k] / count[k])
            before = names[int(majority["before"][k])]
            after = names[int(majority["after"][k])]
            score = min(1.0, 0.5 * strength + 0.5 * max(strength, share))
            if before in preserve or after in preserve:
                score = min(1.0, score * 1.25)
            if before in ignore and after in ignore:
                score *= 0.5
            label = f"{before} -> {after}" if before != after else f"{after} changed"
            found.append(
                {
                    "bbox": box,
                    "label": label[:120],
                    "score": round(score, 3),
                    "area": float(area[k - 1]),
                }
            )
    found.sort(key=lambda r: (-r["score"], -r["area"]))
    measurements = {
        "changed_fraction": float(changed.mean()),
        "photometric_fraction": float(photometric.mean()),
        "class_disagreement": float(disagree[confident].mean()) if confident.any() else 0.0,
        "candidates": float(len(found)),
    }
    return found, measurements


def _iou(a: list[float], b: list[float]) -> float:
    ix = max(0.0, min(a[2], b[2]) - max(a[0], b[0]))
    iy = max(0.0, min(a[3], b[3]) - max(a[1], b[1]))
    inter = ix * iy
    union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter
    return inter / union if union > 0 else 0.0


def overlay(frame_rgb: np.ndarray, regions: list[dict[str, Any]]) -> np.ndarray:
    """The frame with each region's box and id drawn, at most 1024 on the longer side."""
    img = Image.fromarray(frame_rgb)
    scale = min(1.0, 1024 / max(img.size))
    if scale < 1.0:
        img = img.resize((max(1, round(img.width * scale)), max(1, round(img.height * scale))))
    draw = ImageDraw.Draw(img)
    w, h = img.size
    for r in regions:
        x0, y0, x1, y1 = r["bbox"]
        draw.rectangle([x0 * w, y0 * h, x1 * w - 1, y1 * h - 1], outline=(255, 40, 40), width=3)
        draw.text((x0 * w + 4, y0 * h + 2), r["id"], fill=(255, 40, 40))
    return np.asarray(img)


def regions(args: dict[str, Any], ctx: Context) -> dict[str, Any]:
    started = time.monotonic()
    ids = [args.get(k) for k in ("frame", "against")]
    if not all(isinstance(i, str) for i in ids):
        return invalid_args("regions", "frame and against must be frame ids")
    frame_id, against_id = ids
    if frame_id == against_id:
        return invalid_args("regions", "frame and against must be two different frames")
    pair = [ctx.frame_pixels(i) for i in ids]
    for key, i, p in zip(("frame", "against"), ids, pair):
        if p is None:
            return invalid_args("regions", f"{key}: unknown frame id {i!r}")
    frame_px, against_px = pair
    if frame_px.projection != against_px.projection:
        return invalid_args(
            "regions",
            f"{frame_id!r} is {frame_px.projection} and {against_id!r} is "
            f"{against_px.projection}; the frames must share one grid",
        )
    aspect = lambda p: p.width / max(p.height, 1)
    if abs(aspect(frame_px) / aspect(against_px) - 1.0) > 0.01:
        return invalid_args(
            "regions",
            f"{frame_id!r} is {frame_px.width}×{frame_px.height} and {against_id!r} is "
            f"{against_px.width}×{against_px.height}; the frames must share one grid",
        )
    frame_rgb, against_rgb = frame_px.load(), against_px.load()
    seg = segmenter.shared()
    (la, ca, pa), (lb, cb, pb) = seg.segment([frame_rgb, against_rgb])
    names = segmenter.class_names()
    found, measured = find(
        frame_rgb,
        against_rgb,
        (pa.crop(la), pa.crop(ca)),
        (pb.crop(lb), pb.crop(cb)),
        (ctx.hints or {}).get("regions") if isinstance(ctx.hints, dict) else None,
        names,
    )
    kept = found[:MAX_REGIONS]
    for n, r in enumerate(kept, 1):
        r["id"] = f"r{n}"
    out_regions = [
        {
            "id": r["id"],
            "location": bbox_location(frame_id, r["bbox"]),
            "label": r["label"],
            "score": r["score"],
        }
        for r in kept
    ]
    measurements = [
        {
            "name": k,
            "value": round(v, 4),
            "unit": "count" if k == "candidates" else "ratio",
            "frame": frame_id,
        }
        for k, v in measured.items()
    ]
    images = [
        png_image(
            f"crop:{ctx.call_id}:0",
            frame_id,
            {"bbox": [0.0, 0.0, 1.0, 1.0]},
            overlay(frame_rgb, kept),
        )
    ]
    if not kept:
        summary = (
            f"No region of {frame_id} differs from {against_id} beyond noise "
            f"({measured['changed_fraction']:.1%} of the frame marked, none large enough)."
        )
        env = envelope(
            "regions",
            "measured_nothing",
            summary,
            measurements=measurements,
            images=images,
            version=MANIFEST["version"],
        )
    else:
        named = "; ".join(f"{r['id']} {r['label']} (score {r['score']:.2f})" for r in kept)
        dropped = len(found) - len(kept)
        summary = (
            f"{len(kept)} region{'s' if len(kept) != 1 else ''} of {frame_id} differ from "
            f"{against_id}, {measured['changed_fraction']:.1%} of the frame in all: {named}."
            + (f" {dropped} weaker candidates were left out." if dropped else "")
            + " Each is a pointer, not a verdict; look with view before deciding."
        )
        env = envelope(
            "regions",
            "ok",
            summary,
            regions=out_regions,
            measurements=measurements,
            images=images,
            version=MANIFEST["version"],
        )
    env["provenance"]["duration_ms"] = int((time.monotonic() - started) * 1000)
    return env
