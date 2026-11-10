"""Image generators behind one small interface: `generate(prompt, seed, size)` for a new image,
`edit(image, prompt, seed)` for one instruction-edit of one image, and `compose(images, prompt,
seed)` for a new scene carrying the subject of one or more references. All return PNG or JPEG
bytes. Credentials come from the environment only (FAL_KEY, OPENAI_API_KEY, GEMINI_API_KEY) and
are never written anywhere.
"""

from __future__ import annotations

import base64
import io
import json
import os
import sys
import time
import urllib.request
import uuid

from PIL import Image, ImageDraw


class GeneratorError(RuntimeError):
    pass


#: Statuses worth another try: rate limits and the provider's own transient failures.
RETRY_STATUSES = frozenset({408, 429, 500, 502, 503, 504})
#: Seconds to wait before each retry; a long build outlasts a short outage instead of dying on it.
RETRY_WAITS = (5, 20, 60, 120, 240)


def _open(req: urllib.request.Request | str, timeout: int, what: str, sleep=time.sleep) -> bytes:
    """The response body of `req`, retried on a transient failure (a status in
    `RETRY_STATUSES`, a dropped connection or a timeout) after each of `RETRY_WAITS`; any other
    HTTP error, or the last transient one, is a `GeneratorError` naming `what`."""
    for attempt in range(len(RETRY_WAITS) + 1):
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            detail = e.read()[:400].decode("utf-8", "replace")
            if e.code not in RETRY_STATUSES or attempt == len(RETRY_WAITS):
                raise GeneratorError(f"{what}: HTTP {e.code}: {detail}") from None
            reason = f"HTTP {e.code}"
        except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
            if attempt == len(RETRY_WAITS):
                raise GeneratorError(f"{what}: {e}") from None
            reason = type(e).__name__
        wait = RETRY_WAITS[attempt]
        print(f"{what}: {reason}, retrying in {wait}s", file=sys.stderr, flush=True)
        sleep(wait)
    raise AssertionError("unreachable")


def _post_json(url: str, body: dict, headers: dict, timeout: int = 300) -> dict:
    req = urllib.request.Request(
        url, data=json.dumps(body).encode(), headers={"Content-Type": "application/json", **headers}
    )
    return json.loads(_open(req, timeout, url))


def _get_bytes(url: str, timeout: int = 120) -> bytes:
    return _open(url, timeout, url)


def _env(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise GeneratorError(f"{name} is not set")
    return value


def _mime(image: bytes) -> str:
    return "image/png" if image[:8] == b"\x89PNG\r\n\x1a\n" else "image/jpeg"


def _data_uri(image: bytes) -> str:
    return f"data:{_mime(image)};base64," + base64.b64encode(image).decode()


class FalKlein:
    """FLUX.2 [klein] 4B on fal: text to image, instruction edits and multi-reference scenes,
    all seeded."""

    name = "fal"
    model = "fal-ai/flux-2-klein/4b"
    edit_model = "fal-ai/flux-2/klein/4b/edit"

    def _call(self, model: str, body: dict) -> bytes:
        d = _post_json(
            f"https://fal.run/{model}", body, {"Authorization": "Key " + _env("FAL_KEY")}
        )
        images = d.get("images") or []
        if not images:
            raise GeneratorError(f"{model}: no image in the response")
        return _get_bytes(images[0]["url"])

    def generate(self, prompt: str, seed: int, size: int) -> bytes:
        return self._call(
            self.model,
            {
                "prompt": prompt,
                "seed": seed,
                "image_size": {"width": size, "height": size},
                "num_images": 1,
                "output_format": "png",
            },
        )

    def edit(self, image: bytes, prompt: str, seed: int) -> bytes:
        return self.compose([image], prompt, seed)

    def compose(self, images: list[bytes], prompt: str, seed: int) -> bytes:
        return self._call(
            self.edit_model,
            {
                "prompt": prompt,
                "image_urls": [_data_uri(i) for i in images],
                "seed": seed,
                "num_images": 1,
                "output_format": "png",
            },
        )


class OpenAiImage:
    """gpt-image-2 through the Images API; no seed, so the record carries prompt and date."""

    name = "openai"
    model = "gpt-image-2"

    def generate(self, prompt: str, seed: int, size: int) -> bytes:
        d = _post_json(
            "https://api.openai.com/v1/images/generations",
            {
                "model": self.model,
                "prompt": prompt,
                "size": f"{size}x{size}",
                "quality": "medium",
                "output_format": "png",
                "n": 1,
            },
            {"Authorization": "Bearer " + _env("OPENAI_API_KEY")},
        )
        return base64.b64decode(d["data"][0]["b64_json"])

    def edit(self, image: bytes, prompt: str, seed: int) -> bytes:
        return self.compose([image], prompt, seed)

    def compose(self, images: list[bytes], prompt: str, seed: int) -> bytes:
        boundary = "----squint" + uuid.uuid4().hex
        parts = []
        fields = (
            ("model", self.model),
            ("prompt", prompt),
            ("quality", "medium"),
            ("output_format", "png"),
        )
        for name, value in fields:
            parts.append(
                f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n'.encode()
            )
        for n, image in enumerate(images, start=1):
            head = (
                f'--{boundary}\r\nContent-Disposition: form-data; name="image[]"; '
                f'filename="input-{n}.png"\r\nContent-Type: {_mime(image)}\r\n\r\n'
            )
            parts.append(head.encode() + image + b"\r\n")
        parts.append(f"--{boundary}--\r\n".encode())
        req = urllib.request.Request(
            "https://api.openai.com/v1/images/edits",
            data=b"".join(parts),
            headers={
                "Authorization": "Bearer " + _env("OPENAI_API_KEY"),
                "Content-Type": f"multipart/form-data; boundary={boundary}",
            },
        )
        d = json.loads(_open(req, 300, "images/edits"))
        return base64.b64decode(d["data"][0]["b64_json"])


class GeminiImage:
    """Gemini 3.1 Flash Image through generateContent; no seed."""

    name = "gemini"
    model = "gemini-3.1-flash-image"

    def _call(self, parts: list[dict]) -> bytes:
        d = _post_json(
            f"https://generativelanguage.googleapis.com/v1beta/models/{self.model}:generateContent",
            {
                "contents": [{"parts": parts}],
                "generationConfig": {
                    "responseModalities": ["IMAGE"],
                    "imageConfig": {"aspectRatio": "1:1"},
                },
            },
            {"x-goog-api-key": _env("GEMINI_API_KEY")},
        )
        for p in d.get("candidates", [{}])[0].get("content", {}).get("parts", []):
            if "inlineData" in p:
                return base64.b64decode(p["inlineData"]["data"])
        raise GeneratorError(f"{self.model}: no image part in the response")

    def generate(self, prompt: str, seed: int, size: int) -> bytes:
        return self._call([{"text": prompt}])

    def edit(self, image: bytes, prompt: str, seed: int) -> bytes:
        return self.compose([image], prompt, seed)

    def compose(self, images: list[bytes], prompt: str, seed: int) -> bytes:
        parts: list[dict] = [
            {"inlineData": {"mimeType": _mime(i), "data": base64.b64encode(i).decode()}}
            for i in images
        ]
        parts.append({"text": prompt})
        return self._call(parts)


class FakeGenerator:
    """Deterministic pictures for tests and dry runs without a network: a coloured scene with a
    few shapes; `edit` paints over a patch so the difference region is well defined; `compose`
    carries the first reference's subject patch into a new background."""

    name = "fake"
    model = "fake-model"

    def generate(self, prompt: str, seed: int, size: int) -> bytes:
        im = Image.new("RGB", (size, size), (120 + seed % 60, 140, 160))
        d = ImageDraw.Draw(im)
        d.rectangle((size * 0.3, size * 0.4, size * 0.7, size * 0.9), fill=(170, 60, 50))
        d.rectangle((size * 0.45, size * 0.65, size * 0.55, size * 0.9), fill=(20, 80, 40))
        d.ellipse((size * 0.05, size * 0.1, size * 0.3, size * 0.5), fill=(60, 120, 50))
        d.ellipse((size * 0.72, size * 0.8, size * 0.9, size * 0.9), fill=(40, 60, 200))
        buf = io.BytesIO()
        im.save(buf, format="PNG")
        return buf.getvalue()

    def edit(self, image: bytes, prompt: str, seed: int) -> bytes:
        im = Image.open(io.BytesIO(image)).convert("RGB")
        w, h = im.size
        ImageDraw.Draw(im).rectangle((w * 0.7, h * 0.78, w * 0.92, h * 0.92), fill=(120, 140, 160))
        buf = io.BytesIO()
        im.save(buf, format="PNG")
        return buf.getvalue()

    def compose(self, images: list[bytes], prompt: str, seed: int) -> bytes:
        first = Image.open(io.BytesIO(images[0])).convert("RGB")
        w, h = first.size
        im = Image.new("RGB", (w, h), (90 + seed % 80, 70, 110))
        im.paste(
            first.crop((int(w * 0.3), int(h * 0.4), int(w * 0.7), int(h * 0.9))),
            (int(w * 0.2), int(h * 0.3)),
        )
        buf = io.BytesIO()
        im.save(buf, format="PNG")
        return buf.getvalue()


def live() -> dict:
    return {
        "fal": FalKlein(),
        "openai": OpenAiImage(),
        "gemini": GeminiImage(),
        "fake": FakeGenerator(),
    }
