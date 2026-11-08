"""Adversary sweeps for the comparison tools other than `outline`: planted edits of known size on
real sources, run through `squint tools call` (each tool after its prerequisites, as the model
gets it), to measure what each tool catches and what it reports that is not there.

  align     the source warped by a known homography (shift, rotation, zoom, keystone, mirror),
            alone and under a restyle, and another scene: registered or not, and the corner
            error of the recovered mapping in source pixels
  content   a patch of the source replaced by another part of it (swap) or recoloured (hue),
            s% of the frame, and edits that change nothing a rule cares about (a restyle,
            recompression, noise): for `regions` whether a strong region lands on the patch and
            how many land elsewhere; for `change_map` how much of the patch and of the rest is
            marked changed; for `edge_similarity` its value; for `classes` (asked about the
            patch) whether it reports a class difference

usage: python tools_sweep.py <out_dir> <config> <name>=<source.png> [...]
       (the pack's environment, with `squint` on PATH and the config's secrets in the env;
       `SWEEP_JOBS` calls at once, default 3)
"""

from __future__ import annotations

import csv
import json
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parent))
from outline_sweep import GRADES, grade, overlaps

STRONG = 0.5
SIZES = (0.001, 0.0025, 0.005, 0.01, 0.02, 0.04)
# Where the patches go, as fractions of the frame (centre); the donor is half a frame across.
SPOTS = ((0.3, 0.62), (0.7, 0.45))
SHIFTS = (2, 8, 32, 96)
ROTATIONS = (0.5, 2.0, 5.0, 10.0)
ZOOMS = (0.95, 0.85, 1.1)
KEYSTONE = (0.02, 0.06)
CONTENT_TOOLS = ("regions", "change_map", "edge_similarity", "classes")


def call(tool: str, source: Path, candidate: Path, out: Path, config: str, args=None) -> dict:
    cmd = ["squint", "tools", "call", tool, "--config", config, "--profile", "architecture"]
    cmd += ["--use-case", "source-fidelity", "--source", str(source), "--candidate", str(candidate)]
    cmd += ["--out", str(out)]
    if args:
        cmd += ["--args", json.dumps(args)]
    p = subprocess.run(cmd, capture_output=True, text=True, check=False)
    f = out / "result.json"
    if p.returncode != 0 or not f.is_file():
        return {"error": p.stderr.strip().splitlines()[-1:] or ["no result"]}
    return json.load(open(f))


def last(result: dict, tool: str) -> dict | None:
    calls = [c for c in result.get("calls", []) if c["tool"] == tool]
    return calls[-1] if calls else None


# --- planted geometry ----------------------------------------------------------------------


def about_centre(m: np.ndarray, w: int, h: int) -> np.ndarray:
    c = np.array([[1, 0, w / 2], [0, 1, h / 2], [0, 0, 1]])
    return c @ m @ np.linalg.inv(c)


def warp(img: Image.Image, h_true: np.ndarray) -> Image.Image:
    """The image warped by `h_true` (source pixels to candidate pixels), black outside."""
    inv = np.linalg.inv(h_true)
    inv = inv / inv[2, 2]
    return img.transform(img.size, Image.PERSPECTIVE, tuple(inv.ravel()[:8]), Image.BICUBIC)


def corner_error(params: list[float], h_true: np.ndarray, w: int, h: int) -> float:
    """Mean distance in source pixels, over a grid inside the candidate, between where the tool
    maps each point and where it truly came from."""
    est = np.array(params).reshape(3, 3)
    unit_c = np.diag([1 / w, 1 / h, 1])
    px_s = np.diag([w, h, 1])
    est_px = px_s @ est @ unit_c
    truth = np.linalg.inv(h_true)
    g = np.stack(np.meshgrid(np.linspace(0.2, 0.8, 5) * w, np.linspace(0.2, 0.8, 5) * h), -1)
    pts = np.concatenate([g.reshape(-1, 2), np.ones((25, 1))], 1).T
    a, b = est_px @ pts, truth @ pts
    return float(np.linalg.norm(a[:2] / a[2] - b[:2] / b[2], axis=0).mean())


def geometry_cases(w: int, h: int):
    yield "identity", "0", np.eye(3)
    for d in SHIFTS:
        yield "shift", f"{d}px", np.array([[1, 0, d], [0, 1, d / 2], [0, 0, 1]], float)
    for deg in ROTATIONS:
        r = np.deg2rad(deg)
        m = np.array([[np.cos(r), -np.sin(r), 0], [np.sin(r), np.cos(r), 0], [0, 0, 1]])
        yield "rotate", f"{deg}deg", about_centre(m, w, h)
    for z in ZOOMS:
        yield "zoom", f"{z}", about_centre(np.diag([z, z, 1.0]), w, h)
    for k in KEYSTONE:
        m = np.array([[1, 0, 0], [0, 1, 0], [k / w * 2, 0, 1]])
        yield "keystone", f"{k}", about_centre(m, w, h)
    yield "mirror", "h", about_centre(np.diag([-1.0, 1, 1]), w, h)


# --- planted content -----------------------------------------------------------------------


def patch_box(spot, s, w, h):
    side = max(4, round((s * w * h) ** 0.5))
    x0 = int(min(max(0, spot[0] * w - side / 2), w - side))
    y0 = int(min(max(0, spot[1] * h - side / 2), h - side))
    return x0, y0, side


def swapped(rgb: np.ndarray, spot, s) -> tuple[np.ndarray, list[float]]:
    h, w = rgb.shape[:2]
    x0, y0, side = patch_box(spot, s, w, h)
    dx = int((x0 + w // 2) % (w - side))
    out = rgb.copy()
    out[y0 : y0 + side, x0 : x0 + side] = rgb[y0 : y0 + side, dx : dx + side]
    return out, [x0 / w, y0 / h, (x0 + side) / w, (y0 + side) / h]


def recoloured(rgb: np.ndarray, spot, s) -> tuple[np.ndarray, list[float]]:
    h, w = rgb.shape[:2]
    x0, y0, side = patch_box(spot, s, w, h)
    out = rgb.copy()
    hsv = np.asarray(Image.fromarray(rgb[y0 : y0 + side, x0 : x0 + side]).convert("HSV")).copy()
    hsv[..., 0] = (hsv[..., 0].astype(int) + 85) % 256
    hsv[..., 1] = np.maximum(hsv[..., 1], 90)
    out[y0 : y0 + side, x0 : x0 + side] = np.asarray(Image.fromarray(hsv, "HSV").convert("RGB"))
    return out, [x0 / w, y0 / h, (x0 + side) / w, (y0 + side) / h]


def changed(a: np.ndarray, b: np.ndarray, truth: list[float]) -> bool:
    """The edit really changed the pixels inside its truth box."""
    h, w = a.shape[:2]
    x0, y0, x1, y1 = (
        round(truth[0] * w),
        round(truth[1] * h),
        round(truth[2] * w),
        round(truth[3] * h),
    )
    d = np.abs(a[y0:y1, x0:x1].astype(int) - b[y0:y1, x0:x1].astype(int)).max(-1)
    return d.size > 0 and float((d > 30).mean()) >= 0.2


def recompressed(rgb: np.ndarray, path: Path) -> np.ndarray:
    Image.fromarray(rgb).save(path.with_suffix(".tmp.jpg"), quality=45)
    out = np.asarray(Image.open(path.with_suffix(".tmp.jpg")).convert("RGB"))
    path.with_suffix(".tmp.jpg").unlink()
    return out


def noisy(rgb: np.ndarray) -> np.ndarray:
    rng = np.random.default_rng(7)
    return (rgb.astype(np.float32) + rng.normal(0, 5, rgb.shape)).clip(0, 255).astype(np.uint8)


# --- judging -------------------------------------------------------------------------------


def judge_regions(result: dict, truth):
    c = last(result, "regions")
    if c is None:
        return {"status": "no call", "error": result.get("error")}
    env = c["envelope"]
    strong = [r for r in env.get("regions", []) if r["score"] >= STRONG]
    on = [r for r in strong if truth and overlaps(r["location"]["bbox"], truth)]
    return {
        "status": env["status"],
        "caught": bool(on) if truth else None,
        "false": len(strong) - len(on),
        "regions": [(r["score"], r["label"], r["location"].get("bbox")) for r in env["regions"]],
    }


def judge_change_map(result: dict, truth, out: Path):
    c = last(result, "change_map")
    if c is None:
        return {"status": "no call", "error": result.get("error")}
    env = c["envelope"]
    frac = next((m["value"] for m in env["measurements"] if m["name"] == "changed_fraction"), None)
    row = {"status": env["status"], "changed_fraction": frac}
    if env["status"] != "ok" or not env["images"]:
        return row
    frame = env["images"][0]["frame"].replace(":", "_")
    m = np.asarray(Image.open(out / "frames" / f"{frame}.png").convert("L")) > 127
    if truth:
        h, w = m.shape
        x0, y0, x1, y1 = (
            int(truth[0] * w),
            int(truth[1] * h),
            int(np.ceil(truth[2] * w)),
            int(np.ceil(truth[3] * h)),
        )
        inside = m[y0:y1, x0:x1]
        rest = m.copy()
        rest[y0:y1, x0:x1] = False
        row["inside"] = float(inside.mean()) if inside.size else 0.0
        row["outside"] = float(rest.sum() / max(1, m.size - inside.size))
        row["caught"] = row["inside"] >= 0.25
    return row


def judge_edges(result: dict):
    c = last(result, "edge_similarity")
    if c is None:
        return {"status": "no call", "error": result.get("error")}
    env = c["envelope"]
    v = next((m["value"] for m in env["measurements"] if m["name"] == "edge_similarity"), None)
    return {"status": env["status"], "edge_similarity": v}


def judge_classes(result: dict):
    c = last(result, "classes")
    if c is None:
        return {"status": "no call", "error": result.get("error")}
    env = c["envelope"]
    return {
        "status": env["status"],
        "difference": bool(env.get("regions")),
        "summary": env["summary"][:200],
    }


def judge_align(result: dict, h_true, w, h):
    c = last(result, "align")
    if c is None:
        return {"status": "no call", "error": result.get("error")}
    env = c["envelope"]
    row = {"status": env["status"], "summary": env["summary"][:160]}
    q = {m["name"]: m["value"] for m in env["measurements"]}
    row["ncc"], row["inliers"] = q.get("ncc"), q.get("inlier_fraction")
    if env["status"] == "ok" and env["transforms"] and h_true is not None:
        row["corner_err_px"] = round(corner_error(env["transforms"][0]["params"], h_true, w, h), 2)
    return row


# --- driver --------------------------------------------------------------------------------


def main() -> None:
    out, config = Path(sys.argv[1]), sys.argv[2]
    sources = dict(a.split("=", 1) for a in sys.argv[3:])
    jobs = int(os.environ.get("SWEEP_JOBS", "3"))
    work = []  # (row, tool, source path, candidate path, args, judge)

    for name, path in sources.items():
        src = Image.open(path).convert("RGB")
        raw = np.asarray(src)
        h, w = raw.shape[:2]
        d = out / name
        d.mkdir(parents=True, exist_ok=True)

        def add(
            sweep,
            param,
            extra,
            img,
            truth=None,
            h_true=None,
            tools=CONTENT_TOOLS,
            *,
            d=d,
            name=name,
            path=path,
            w=w,
            h=h,
        ):
            tag = f"{sweep}-{param}-{extra}".replace(" ", "")
            cand = d / f"{tag}.png"
            (img if isinstance(img, Image.Image) else Image.fromarray(img)).save(cand)
            for tool in tools:
                args = {"bbox": truth} if tool == "classes" else None
                if tool == "classes" and truth is None:
                    continue
                row = {"source": name, "sweep": sweep, "param": param, "extra": extra, "tool": tool}
                row["truth"] = truth
                work.append((row, tool, Path(path), cand, args, h_true, (w, h)))

        for kind, param, h_true in geometry_cases(w, h):
            add("align", param, kind, warp(src, h_true), h_true=h_true, tools=("align",))
        for kind in GRADES:
            h_true = np.array([[1, 0, 8], [0, 1, 4], [0, 0, 1]], float)
            add(
                "align",
                "8px",
                f"shift+{kind}",
                grade(np.asarray(warp(src, h_true)), kind, 1.0),
                h_true=h_true,
                tools=("align",),
            )
        other = next((p for n, p in sources.items() if n != name), None)
        if other:
            o = Image.open(other).convert("RGB").resize((w, h))
            add("align", "-", "other-scene", o, tools=("align",))

        for edit, fn in (("swap", swapped), ("hue", recoloured)):
            for i, spot in enumerate(SPOTS):
                for s in SIZES:
                    img, truth = fn(raw, spot, s)
                    if not changed(raw, img, truth):
                        print(f"{name:10} {edit} {s} spot{i}: the patch changed nothing; skipped")
                        continue
                    add(edit, s, f"spot{i}", img, truth)
        for kind in GRADES:
            for t in (0.5, 1.0):
                add("restyle", t, kind, grade(raw, kind, t))
        add("restyle", "q45", "jpeg", recompressed(raw, d / "x.png"))
        add("restyle", "sigma5", "noise", noisy(raw))

    print(f"{len(work)} calls", flush=True)

    def run(i_item):
        i, (row, tool, source, cand, args, h_true, (w, h)) = i_item
        o = out / "calls" / f"{i:05d}-{row['source']}-{tool}"
        result = call(tool, source, cand, o, config, args)
        truth = row["truth"]
        if tool == "align":
            row.update(judge_align(result, h_true, w, h))
        elif tool == "regions":
            row.update(judge_regions(result, truth))
        elif tool == "change_map":
            row.update(judge_change_map(result, truth, o))
        elif tool == "edge_similarity":
            row.update(judge_edges(result))
        else:
            row.update(judge_classes(result))
        row["call_dir"] = str(o)
        print(
            f"{i:5} {row['source']:10} {row['sweep']:8} {row['param']!s:8} {row['extra']:14} {tool}",
            flush=True,
        )
        return row

    with ThreadPoolExecutor(jobs) as ex:
        rows = list(ex.map(run, enumerate(work)))
    with open(out / "sweep.json", "w") as f:
        json.dump(rows, f, indent=1, default=str)
    cols = [
        "source",
        "sweep",
        "param",
        "extra",
        "tool",
        "status",
        "caught",
        "false",
        "corner_err_px",
        "changed_fraction",
        "inside",
        "outside",
        "edge_similarity",
        "difference",
    ]
    with open(out / "sweep.csv", "w", newline="") as f:
        wr = csv.DictWriter(f, fieldnames=cols, extrasaction="ignore")
        wr.writeheader()
        wr.writerows(rows)
    print(f"{len(rows)} rows -> {out / 'sweep.csv'}")


if __name__ == "__main__":
    main()
