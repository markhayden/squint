"""The segmenter: the ONNX export recorded in ``MODEL.md``, run with onnxruntime on the CPU.

This module runs the artifact and turns its two heads into a semantic map; preprocessing an
image of any shape into the fixed 512×512 input is the tools' business (task 5.4a). The artifact
is loaded from the models directory that ``fetch`` filled; a missing or wrong-sized file is a
``SegmenterUnavailable``. The server checks for it at ``initialize`` whenever it serves a tool in
``tools.NEEDS_SEGMENTER``, so the harness ends the run at resolve, before any spend.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

import numpy as np

from . import models

INPUT = "pixel_values"
SIDE = 512
PROVIDER = "CPUExecutionProvider"
MEAN = np.array([0.485, 0.456, 0.406], np.float32)
STD = np.array([0.229, 0.224, 0.225], np.float32)
# How the tools bring an image of any shape to the square input (`MODEL.md`, preprocessing
# version 1 plus this): the longer edge is resized to 512 (bilinear), the shorter padded to 512
# with the image's mean colour, centred; the semantic map is read back from the same placement.
# Recorded in MODEL.md and the tools' documentation; an envelope's provenance holds only the
# harness's fixed fields.
PREPROCESSING_VERSION = "1-pad"


@dataclass(frozen=True)
class Placement:
    """Where an image landed inside the square input: offset and size in input pixels."""

    x: int
    y: int
    width: int
    height: int

    def crop(self, grid: np.ndarray) -> np.ndarray:
        """The part of a map over the square input (any resolution) that covers the image."""
        scale = grid.shape[-1] / SIDE
        x0, y0 = round(self.x * scale), round(self.y * scale)
        x1 = max(x0 + 1, round((self.x + self.width) * scale))
        y1 = max(y0 + 1, round((self.y + self.height) * scale))
        return grid[..., y0:y1, x0:x1]


def prepare(rgb: np.ndarray) -> tuple[np.ndarray, Placement]:
    """An RGB uint8 image of any shape as one `pixel_values` entry `[3, 512, 512]`."""
    from PIL import Image

    h, w = rgb.shape[:2]
    scale = SIDE / max(h, w)
    width, height = max(1, round(w * scale)), max(1, round(h * scale))
    resized = np.asarray(Image.fromarray(rgb).resize((width, height), Image.BILINEAR))
    square = np.empty((SIDE, SIDE, 3), np.uint8)
    square[:] = rgb.reshape(-1, 3).mean(0).round().astype(np.uint8)
    x, y = (SIDE - width) // 2, (SIDE - height) // 2
    square[y : y + height, x : x + width] = resized
    pixel_values = ((square.astype(np.float32) / 255.0 - MEAN) / STD).transpose(2, 0, 1)
    return pixel_values.astype(np.float32), Placement(x, y, width, height)


class SegmenterUnavailable(RuntimeError):
    """The artifact is not where ``fetch`` puts it, or is not the recorded file."""


class Segmenter:
    def __init__(self, models_dir: Path | None = None) -> None:
        import onnxruntime as ort

        self.record = models.Record.load()
        problem = models.artifact_problem(self.record, models_dir)
        if problem is not None:
            raise SegmenterUnavailable(problem)
        path = models.artifact_path(self.record, models_dir)
        self.session = ort.InferenceSession(str(path), providers=[PROVIDER])
        self.classes = int(self.record.data.get("classes", 150))

    def run(self, pixel_values: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        """The two heads for a float32 batch ``[b, 3, 512, 512]``: class logits ``[b, q, c + 1]``
        and mask logits ``[b, q, 128, 128]``."""
        if pixel_values.shape[1:] != (3, SIDE, SIDE) or pixel_values.dtype != np.float32:
            raise ValueError(
                f"pixel_values must be float32 [batch, 3, {SIDE}, {SIDE}], got "
                f"{pixel_values.dtype} {pixel_values.shape}"
            )
        class_logits, mask_logits = self.session.run(None, {INPUT: pixel_values})
        return class_logits, mask_logits

    def segment(self, images: list[np.ndarray]) -> list[tuple[np.ndarray, np.ndarray, Placement]]:
        """Each image's semantic map and per-pixel confidence over the quarter-resolution grid of
        its square input, with where the image sits in it."""
        prepared = [prepare(rgb) for rgb in images]
        c, m = self.run(np.stack([pv for pv, _ in prepared]))
        labels, confidence = semantic_map(c, m, with_confidence=True)
        return [(labels[i], confidence[i], placement) for i, (_, placement) in enumerate(prepared)]


def semantic_map(class_logits: np.ndarray, mask_logits: np.ndarray, with_confidence: bool = False):
    """The class of each pixel (preprocessing version 1, ``MODEL.md``): softmax over classes
    without the no-object column, sigmoid over masks, their product summed over queries, argmax.
    With `with_confidence`, also the winning class's share of the summed scores at each pixel."""
    c = class_logits - class_logits.max(-1, keepdims=True)
    probs = np.exp(c)
    probs = (probs / probs.sum(-1, keepdims=True))[..., :-1]
    masks = 0.5 * (1.0 + np.tanh(0.5 * mask_logits))  # the sigmoid, without exp overflow
    seg = np.einsum("bqc,bqhw->bchw", probs, masks, optimize=True)
    labels = seg.argmax(1)
    if not with_confidence:
        return labels
    best = np.take_along_axis(seg, labels[:, None], 1)[:, 0]
    return labels, best / np.maximum(seg.sum(1), 1e-6)


def class_names() -> list[str]:
    """The 150 ADE20K class names, in the model's label order."""
    import json
    from importlib import resources

    text = resources.files("squint_tools").joinpath("models", "ade20k-classes.json").read_text()
    return json.loads(text)["classes"]


_shared: Segmenter | None = None


def shared() -> Segmenter:
    """The one loaded segmenter of this process: the artifact is read once, the tools share it."""
    global _shared
    if _shared is None:
        _shared = Segmenter()
    return _shared
