"""Adversary sweeps for `outline`: planted edits of controlled size on real sources, run through
the tool's own comparison, to measure what it catches and what it reports that is not there.

Three sweeps, each in both modes (the named class, and every class):
  shift    the pool's front edge moved n px (pool wider), n from 2 to 60
  removal  a patch of deck pasted over the pool's edge, s% of the frame, mid-edge or at the end
  restyle  a golden-hour, fog or dusk grade at strength t, on an unchanged candidate (what it
           reports that is not there) and on a 30 px edge move (whether the grade hides it)

A case is caught when a piece scoring at least 0.5 overlaps the truth box; every other piece at
0.5 or more is a false one.

usage: python outline_sweep.py <out_dir> <name>=<source.png> [...]
       (the pack's environment; needs the segmenter)
"""

from __future__ import annotations

import csv
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageFilter

from squint_tools import outline, segmenter

NAMES = segmenter.class_names()
POOL = NAMES.index("swimming pool")
STRONG = 0.5
PAD = 0.01

SHIFTS = (2, 4, 6, 8, 10, 12, 16, 20, 30, 45, 60)
SIZES = (0.0005, 0.001, 0.002, 0.005, 0.01, 0.02, 0.04)
STRENGTHS = (0.25, 0.5, 0.75, 1.0)
GRADES = ("golden", "fog", "dusk")


def pool_mask(rgb: np.ndarray) -> np.ndarray:
    ((scores, placement),) = segmenter.shared().scores([rgb])
    h, w = rgb.shape[:2]
    return outline.despeckled(outline.class_mask(scores, placement, POOL, (h, w)), 0.005)


def resample_rows(img: np.ndarray, a: int, b: int, c: int, n: int) -> np.ndarray:
    """Rows a..b stretched to a..b+n and rows b..c squeezed to b+n..c: an edge at b moves n down."""
    out = img.copy()
    pil = Image.fromarray(img)
    top = pil.crop((0, a, img.shape[1], b)).resize((img.shape[1], b + n - a), Image.BICUBIC)
    bot = pil.crop((0, b, img.shape[1], c)).resize((img.shape[1], c - b - n), Image.BICUBIC)
    out[a : b + n] = np.asarray(top)
    out[b + n : c] = np.asarray(bot)
    return out


def grade(rgb: np.ndarray, kind: str, t: float) -> np.ndarray:
    """A restyle of strength t in [0, 1]: light and weather, never geometry."""
    x = rgb.astype(np.float32) / 255.0
    if kind == "golden":
        x = x * np.array([1 + 0.30 * t, 1 + 0.05 * t, 1 - 0.30 * t]) + 0.04 * t
        x = x ** (1 - 0.25 * t)
    elif kind == "fog":
        x = x * (1 - 0.6 * t) + np.array([0.86, 0.87, 0.89]) * 0.6 * t
    elif kind == "dusk":
        x = x * (1 - 0.55 * t) * np.array([1 - 0.1 * t, 1 - 0.05 * t, 1 + 0.15 * t])
    out = Image.fromarray((x.clip(0, 1) * 255).astype(np.uint8))
    if kind == "fog" and t > 0:
        out = out.filter(ImageFilter.GaussianBlur(1.5 * t))
    return np.asarray(out)


def pieces(source: np.ndarray, candidate: np.ndarray) -> dict[str, list[dict]]:
    """The tool's pieces for the pair in both modes, as (bbox, score, kind, class)."""
    (sa, pa), (sb, pb) = segmenter.shared().scores([source, candidate])
    h, w = source.shape[:2]
    size = (round(outline.WORK * h / max(h, w)), round(outline.WORK * w / max(h, w)))
    scale = max(h, w) / outline.WORK
    before, after = outline.despeckled_pair(
        outline.class_mask(sa, pa, POOL, size), outline.class_mask(sb, pb, POOL, size)
    )
    named = outline.compare(before, after, scale)["pieces"]
    for p in named:
        p["class"] = "swimming pool"
    _, every = outline.scan((sa, pa), (sb, pb), size, scale, NAMES)
    keep = ("bbox", "score", "kind", "class", "area")
    return {
        "named": [{k: p[k] for k in keep} for p in named[: outline.MAX_REGIONS]],
        "all": [{k: p[k] for k in keep} for p in every[: outline.MAX_REGIONS]],
    }


def overlaps(a: list[float], b: list[float]) -> bool:
    return not (
        a[2] <= b[0] - PAD or b[2] + PAD <= a[0] or a[3] <= b[1] - PAD or b[3] + PAD <= a[1]
    )


def judge(found: list[dict], truth: list[float] | None) -> tuple[bool | None, int, float]:
    """(caught, false pieces, best score on the truth) for one mode's pieces."""
    strong = [p for p in found if p["score"] >= STRONG]
    if truth is None:
        return None, len(strong), 0.0
    on = [p for p in found if overlaps(p["bbox"], truth)]
    best = max((p["score"] for p in on), default=0.0)
    return best >= STRONG, sum(1 for p in strong if not overlaps(p["bbox"], truth)), best


def main() -> None:
    out = Path(sys.argv[1])
    (out / "img").mkdir(parents=True, exist_ok=True)
    sources = dict(a.split("=", 1) for a in sys.argv[2:])
    rows = []

    def case(name, sweep, param, src, cand, truth, extra=""):
        found = pieces(src, cand)
        tag = f"{name}-{sweep}-{param}".replace(" ", "")
        Image.fromarray(cand).save(out / "img" / f"{tag}.jpg", quality=80)
        for mode in ("named", "all"):
            caught, false, best = judge(found[mode], truth)
            rows.append(
                {
                    "source": name,
                    "sweep": sweep,
                    "param": param,
                    "extra": extra,
                    "mode": mode,
                    "caught": caught,
                    "false": false,
                    "best": round(best, 3),
                    "truth": truth,
                    "pieces": found[mode],
                    "image": f"img/{tag}.jpg",
                }
            )
        print(f"{name:8} {sweep:8} {param!s:14} {extra:10}", flush=True)

    for name, path in sources.items():
        raw = np.asarray(Image.open(path).convert("RGB"))
        h, w = raw.shape[:2]
        m = pool_mask(raw)
        lowest = np.nonzero(m)[0].max()
        # The edge moved is the pool's front (lowest) edge; where the pool runs off the bottom of
        # the frame that is no edge, so the work is done upside down, on its back edge.
        flip = lowest >= h - 2
        base, m = (raw[::-1].copy(), m[::-1]) if flip else (raw, m)
        ys, xs = np.nonzero(m)
        cols = np.unique(xs)
        bottom = int(np.median([ys[xs == x].max() for x in cols[:: max(1, len(cols) // 200)]]))
        span = int(0.06 * h)
        below = min(h, bottom + 2 * span)
        edge = "back" if flip else "front"

        def out_of(img: np.ndarray, flip=flip) -> np.ndarray:
            return np.ascontiguousarray(img[::-1]) if flip else img

        def box(x0, y0, x1, y1, flip=flip, w=w, h=h) -> list[float]:
            if flip:
                y0, y1 = h - y1, h - y0
            return [x0 / w, y0 / h, x1 / w, y1 / h]

        def shifted(n: int, base=base, bottom=bottom, span=span, below=below):
            """The edge moved n px outward, or None where the frame has no room for it."""
            if below - bottom - n < 4:
                return None
            return out_of(resample_rows(base, bottom - span, bottom, below, n))

        def shift_box(n: int, bottom=bottom, w=w) -> list[float]:
            return box(0, bottom, w, bottom + n)

        def planted(img: np.ndarray, truth: list[float], raw=raw, w=w, h=h) -> bool:
            """The edit really changed the pixels inside its truth box."""
            x0, y0, x1, y1 = (
                round(truth[0] * w),
                round(truth[1] * h),
                round(truth[2] * w),
                round(truth[3] * h),
            )
            d = np.abs(img[y0:y1, x0:x1].astype(int) - raw[y0:y1, x0:x1].astype(int)).max(-1)
            return d.size > 0 and float((d > 30).mean()) >= 0.2

        for n in SHIFTS:
            moved = shifted(n)
            if moved is None:
                print(f"{name:8} shift    {n:<14} skipped: no room past the {edge} edge")
                continue
            case(name, "shift", n, raw, moved, shift_box(n), edge)

        # A patch of deck pasted over the pool's edge: the pool loses that piece. The deck is the
        # nearest square below the edge with no pool in it.
        front = [(int(x), int(ys[xs == x].max())) for x in cols[:: max(1, len(cols) // 400)]]
        for where in ("middle", "end"):
            x_at, y_at = front[len(front) // 2] if where == "middle" else front[-1]
            for s in SIZES:
                side = max(4, round((s * h * w) ** 0.5))
                x0 = min(
                    max(0, x_at - side // 2 if where == "middle" else x_at - side + 1), w - side
                )
                y0 = min(max(0, y_at - side + side // 4), h - side)
                deck_y = next(
                    (
                        y
                        for y in range(y_at + 4, h - side + 1, max(1, side // 4))
                        if m[y : y + side, x0 : x0 + side].mean() < 0.05
                    ),
                    None,
                )
                truth = box(x0, y0, x0 + side, y0 + side)
                if deck_y is None:
                    print(f"{name:8} removal  {s:<14} skipped: no deck below the {edge} edge")
                    continue
                img = base.copy()
                img[y0 : y0 + side, x0 : x0 + side] = base[deck_y : deck_y + side, x0 : x0 + side]
                img = out_of(img)
                if not planted(img, truth):
                    print(f"{name:8} removal  {s:<14} skipped: the patch changed nothing")
                    continue
                case(name, "removal", s, raw, img, truth, where)

        for kind in GRADES:
            for t in STRENGTHS:
                case(name, "restyle", t, raw, grade(raw, kind, t), None, kind)
                moved = shifted(30)
                if moved is not None:
                    case(name, "restyled", t, raw, grade(moved, kind, t), shift_box(30), kind)

    with open(out / "sweep.json", "w") as f:
        json.dump(rows, f)
    with open(out / "sweep.csv", "w", newline="") as f:
        cols = ["source", "sweep", "param", "extra", "mode", "caught", "false", "best"]
        wr = csv.DictWriter(f, fieldnames=cols, extrasaction="ignore")
        wr.writeheader()
        wr.writerows(rows)
    print(f"{len(rows)} rows -> {out / 'sweep.csv'}")


if __name__ == "__main__":
    main()
