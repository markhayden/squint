"""Construction operations: each turns a source image into a candidate and says what the change
means under the rules, which is the draft label. Deterministic ops need no generator; `edit` asks
the family's generator to change one thing and draws the region from the pixel difference.
"""

from __future__ import annotations

import io
import math
from dataclasses import dataclass, field
from typing import Any

from PIL import Image, ImageEnhance

from . import regions

#: Reality-check defects an edit can plant, and the rule each one breaks.
REALITY_DEFECTS = {"anatomy": "RE1", "shadow": "RE2", "text": "RE3", "merge": "RE4"}


@dataclass
class DraftLabel:
    expected: str
    failure_type: str | None
    obligation_ids: list[str]
    words: str
    region_kind: str | None = None
    bbox: list[float] | None = None
    notes: dict[str, Any] = field(default_factory=dict)


def _load(b: bytes) -> Image.Image:
    return Image.open(io.BytesIO(b)).convert("RGB")


def _png(im: Image.Image) -> bytes:
    buf = io.BytesIO()
    im.save(buf, format="PNG")
    return buf.getvalue()


def apply(op: dict[str, Any], source: bytes, generator, seed: int) -> tuple[bytes, DraftLabel]:
    """The op's candidate and draft label, then any delivery damage the op names (`jpeg`,
    `resample`), which changes no label: a re-saved or rescaled pass is still a pass."""
    out, label = _apply(op, source, generator, seed)
    if "jpeg" in op or "resample" in op:
        out, said = deliver(out, op.get("jpeg"), op.get("resample"))
        label.words += f"; delivered {said}"
        label.notes["delivery"] = said
    return out, label


def deliver(image: bytes, jpeg: Any = None, resample: Any = None) -> tuple[bytes, str]:
    """`image` as a delivery would damage it: scaled by `resample` and back (detail lost), then
    saved as JPEG at quality `jpeg`; returned as PNG at the original size, with what was done."""
    im = _load(image)
    said = []
    if resample is not None:
        f = float(resample)
        if not 0.25 <= f < 1.0:
            raise ValueError(f"resample must be in [0.25, 1), got {f}")
        w, h = im.size
        im = im.resize((max(1, round(w * f)), max(1, round(h * f))), Image.Resampling.LANCZOS)
        im = im.resize((w, h), Image.Resampling.BICUBIC)
        said.append(f"downscaled to {int(f * 100)}% and upscaled back")
    if jpeg is not None:
        q = int(jpeg)
        if not 20 <= q <= 95:
            raise ValueError(f"jpeg quality must be in [20, 95], got {q}")
        buf = io.BytesIO()
        im.save(buf, format="JPEG", quality=q)
        im = _load(buf.getvalue())
        said.append(f"re-saved as JPEG at quality {q}")
    return _png(im), " and ".join(said)


Mat = list[list[float]]


def _mul(a: Mat, b: Mat) -> Mat:
    return [[sum(a[i][k] * b[k][j] for k in range(3)) for j in range(3)] for i in range(3)]


def _inv(m: Mat) -> Mat:
    (a, b, c), (d, e, f), (g, h, i) = m
    det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)
    adj = [
        [e * i - f * h, c * h - b * i, b * f - c * e],
        [f * g - d * i, a * i - c * g, c * d - a * f],
        [d * h - e * g, b * g - a * h, a * e - b * d],
    ]
    return [[v / det for v in row] for row in adj]


def _about_centre(m: Mat, w: int, h: int) -> Mat:
    """`m` applied about the image centre rather than its corner."""
    to = [[1, 0, w / 2], [0, 1, h / 2], [0, 0, 1]]
    back = [[1, 0, -w / 2], [0, 1, -h / 2], [0, 0, 1]]
    return _mul(to, _mul(m, back))


def _filled(im: Image.Image, h: Mat) -> Image.Image:
    """`im` under the pixel homography `h` (source px to candidate px) at its own size, zoomed
    about the centre just enough that every output corner comes from inside the source, so the
    frame has no empty wedges."""
    w, hh = im.size
    inv: Mat = _inv(h)
    for step in range(400):
        z = 1 + 0.0025 * step
        inv = _inv(_mul(_about_centre([[z, 0, 0], [0, z, 0], [0, 0, 1]], w, hh), h))
        ok = True
        for x, y in ((0, 0), (w, 0), (0, hh), (w, hh)):
            px, py, pw = (inv[r][0] * x + inv[r][1] * y + inv[r][2] for r in range(3))
            if not (-0.5 <= px / pw <= w + 0.5 and -0.5 <= py / pw <= hh + 0.5):
                ok = False
                break
        if ok:
            break
    coeffs = [inv[r][c] / inv[2][2] for r in range(3) for c in range(3)][:8]
    return im.transform((w, hh), Image.Transform.PERSPECTIVE, coeffs, Image.Resampling.BICUBIC)


def _apply(op: dict[str, Any], source: bytes, generator, seed: int) -> tuple[bytes, DraftLabel]:
    kind = op.get("kind")
    if kind == "as_is":
        return source, DraftLabel(
            expected="pass",
            failure_type=None,
            obligation_ids=[],
            words="the generated image as it is; confirm nothing in it breaks the rules",
        )
    if kind == "generate":
        # The candidate from its own prompt (reality checks: a defect the generators will draw when
        # asked for it directly but ignore when asked to edit it in). No region: whole image.
        if generator is None:
            raise ValueError("generate: the family's generator is needed")
        prompt = op["prompt"]
        defect = op.get("defect")
        out = generator.generate(prompt, seed, int(op.get("size", 1024)))
        if defect:
            rule = REALITY_DEFECTS.get(defect)
            if rule is None:
                raise ValueError(
                    f"generate: defect must be one of {sorted(REALITY_DEFECTS)}, got {defect!r}"
                )
            return out, DraftLabel(
                expected="fail",
                failure_type=defect,
                obligation_ids=[rule],
                words=op.get("words") or f"a generated {defect} defect: {prompt}",
                region_kind="whole_image",
                notes={"prompt": prompt},
            )
        return out, DraftLabel(
            expected="pass",
            failure_type=None,
            obligation_ids=[],
            words="generated clean from its own prompt; confirm nothing in it breaks the rules",
            notes={"prompt": prompt},
        )
    if kind == "grade":
        warmth = float(op.get("warmth", 0.1))
        im = _load(source)
        r, g, b = im.split()
        r = r.point(lambda v: min(255, int(v * (1 + warmth))))
        b = b.point(lambda v: max(0, int(v * (1 - warmth))))
        im = Image.merge("RGB", (r, g, b))
        im = ImageEnhance.Contrast(im).enhance(1 + warmth / 2)
        return _png(im), DraftLabel(
            expected="pass",
            failure_type=None,
            obligation_ids=[],
            words=f"a mild colour grade (warmth {warmth:+.2f}); structure and subjects unchanged",
        )
    if kind == "mirror":
        im = _load(source).transpose(Image.Transpose.FLIP_LEFT_RIGHT)
        return _png(im), DraftLabel(
            expected="fail",
            failure_type="mirrored",
            obligation_ids=["SF2"],
            words="the candidate is the source mirrored left to right",
            region_kind="whole_image",
        )
    if kind == "rotate":
        degrees = int(op.get("degrees", 90))
        if degrees not in (90, 180, 270):
            raise ValueError(f"rotate: degrees must be 90, 180 or 270, got {degrees}")
        im = _load(source).rotate(degrees, expand=True)
        return _png(im), DraftLabel(
            expected="fail",
            failure_type="rotated",
            obligation_ids=["SF2"],
            words=f"the candidate is the source rotated by {degrees} degrees",
            region_kind="whole_image",
        )
    if kind == "crop":
        keep = float(op.get("keep", 0.75))
        if not 0.2 <= keep < 1.0:
            raise ValueError(f"crop: keep must be in [0.2, 1), got {keep}")
        im = _load(source)
        w, h = im.size
        cw, ch = round(w * keep), round(h * keep)
        x0, y0 = (w - cw) // 2, (h - ch) // 2
        im = im.crop((x0, y0, x0 + cw, y0 + ch))
        return _png(im), DraftLabel(
            expected="fail",
            failure_type="cropped",
            obligation_ids=["SF2"],
            words=f"the candidate is the central {int(keep * 100)}% of the source; the edges are cropped away",
            region_kind="whole_image",
        )
    if kind == "tilt":
        # A small camera roll, filled to the frame: what a regenerated shot drifts by.
        degrees = float(op.get("degrees", 2.0))
        if not 0.5 <= abs(degrees) <= 10:
            raise ValueError(f"tilt: degrees must be 0.5 to 10 either way, got {degrees}")
        im = _load(source)
        r = math.radians(degrees)
        m = [[math.cos(r), -math.sin(r), 0], [math.sin(r), math.cos(r), 0], [0, 0, 1]]
        return _png(_filled(im, _about_centre(m, *im.size))), DraftLabel(
            expected="fail",
            failure_type="tilted",
            obligation_ids=["SF2"],
            words=f"the candidate is the source rolled by {degrees:+.1f} degrees and refilled to the frame",
            region_kind="whole_image",
        )
    if kind == "zoom":
        factor = float(op.get("factor", 1.1))
        if not 1.03 <= factor <= 1.5:
            raise ValueError(f"zoom: factor must be in [1.03, 1.5], got {factor}")
        im = _load(source)
        m = [[factor, 0, 0], [0, factor, 0], [0, 0, 1]]
        return _png(_filled(im, _about_centre(m, *im.size))), DraftLabel(
            expected="fail",
            failure_type="zoomed",
            obligation_ids=["SF2"],
            words=f"the candidate is the source zoomed in {factor:.2f}x at the same size; the edges are lost",
            region_kind="whole_image",
        )
    if kind == "keystone":
        # A horizontal keystone: one side of the frame nearer the camera than the other.
        k = float(op.get("strength", 0.04))
        if not 0.01 <= abs(k) <= 0.15:
            raise ValueError(f"keystone: strength must be 0.01 to 0.15 either way, got {k}")
        im = _load(source)
        w, h = im.size
        m = [[1, 0, 0], [0, 1, 0], [2 * k / w, 0, 1]]
        return _png(_filled(im, _about_centre(m, w, h))), DraftLabel(
            expected="fail",
            failure_type="keystoned",
            obligation_ids=["SF2"],
            words=f"the candidate is the source under a {abs(k) * 100:.0f}% keystone (the {'right' if k > 0 else 'left'} side pushed back), refilled to the frame",
            region_kind="whole_image",
        )
    if kind == "restyle":
        if generator is None:
            raise ValueError("restyle: the family's generator is needed")
        prompt = op["prompt"]
        out = generator.edit(source, prompt, seed)
        changes = regions.difference_regions(source, out)
        label = DraftLabel(
            expected="pass",
            failure_type=None,
            obligation_ids=[],
            words=f"a permitted restyle ({prompt.split('.')[0].strip().lower()}); structure and every protected subject must be unchanged, confirm against the source",
            notes={"prompt": prompt},
        )
        label.notes["changed_area"] = (
            "whole_image" if changes.whole or changes.primary is None else changes.primary
        )
        if changes.secondary:
            label.notes["secondary_changes"] = changes.secondary
        return out, label
    if kind == "edit":
        if generator is None:
            raise ValueError("edit: the family's generator is needed")
        prompt = op["prompt"]
        removes = op.get("removes")
        alters = op.get("alters")
        adds = op.get("adds")
        defect = op.get("defect")
        out = generator.edit(source, prompt, seed)
        changes = regions.difference_regions(source, out)
        box = changes.primary
        if defect:
            # A planted reality-check defect: the rule it breaks is the defect's.
            rule = REALITY_DEFECTS.get(defect)
            if rule is None:
                raise ValueError(
                    f"edit: defect must be one of {sorted(REALITY_DEFECTS)}, got {defect!r}"
                )
            words = op.get("words") or f"a planted {defect} defect: {prompt}"
            failure_type = defect
            obligations = [rule]
        else:
            if adds:
                words = f"the candidate has {adds} that the source does not"
            elif removes:
                words = f"the {removes} is gone from the candidate"
            elif alters:
                words = f"the {alters} is changed in the candidate"
            else:
                words = f"edited: {prompt}"
            if adds:
                failure_type = f"{adds}-added"
            elif removes or alters:
                failure_type = f"{removes or alters}-changed"
            else:
                failure_type = "edited"
            obligations = ["SF1"]
        label = DraftLabel(
            expected="fail",
            failure_type=failure_type,
            obligation_ids=obligations,
            words=words,
            notes={"prompt": prompt},
        )
        if box is None:
            label.region_kind = "whole_image"
        else:
            label.region_kind = "bbox"
            label.bbox = list(box)
        if changes.secondary:
            label.notes["secondary_changes"] = changes.secondary
            label.words += (
                f"; the generator also changed {len(changes.secondary)} other area(s), check them"
            )
        return out, label
    raise ValueError(f"unknown op kind {kind!r}")
