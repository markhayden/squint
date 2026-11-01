"""Model artifacts: the records this pack ships, where fetched artifacts live, and `fetch`.

A record (``models/<name>.json``, written by the export script, see ``MODEL.md``) names the
artifact, its SHA-256 and size, and the goldens published beside it. ``fetch`` downloads every
file of a record from its release (or copies it from a directory), hashing as it goes, and keeps
nothing that does not hash to the record. Nothing else in the pack downloads anything.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import sys
import tempfile
import urllib.request
from dataclasses import dataclass
from importlib import resources
from pathlib import Path
from typing import Any

SEGMENTER = "segmenter-v1"
RELEASES = "https://github.com/markhayden/squint/releases/download"


class FetchError(RuntimeError):
    """A file could not be fetched, or its bytes do not hash to the record."""


@dataclass(frozen=True)
class Record:
    name: str
    data: dict[str, Any]

    @staticmethod
    def load(name: str = SEGMENTER) -> Record:
        text = resources.files("squint_tools").joinpath("models", f"{name}.json").read_text()
        return Record(name, json.loads(text))

    @property
    def artifact(self) -> str:
        return self.data["artifact"]

    @property
    def files(self) -> dict[str, str]:
        """Every published file with its SHA-256: the artifact first, then the goldens."""
        files = {self.data["artifact"]: self.data["sha256"]}
        for g in self.data.get("goldens", []):
            files[g["file"]] = g["sha256"]
        return files

    def release_url(self, file: str) -> str:
        return f"{RELEASES}/{self.name}/{file}"


def models_dir() -> Path:
    """``$XDG_CACHE_HOME/squint/models`` (SPEC "Python tool pack")."""
    cache = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(cache) / "squint" / "models"


def artifact_path(record: Record, models: Path | None = None) -> Path:
    return (models or models_dir()) / record.name / record.artifact


def artifact_problem(record: Record, models: Path | None = None) -> str | None:
    """Why the record's artifact cannot be loaded, naming the file and how to fetch it, or None.

    A cheap check (present, at the recorded size) for the server's ``initialize``; the hash is
    ``fetch``'s business.
    """
    path = artifact_path(record, models)
    if not path.is_file():
        return f"{path} is missing; run `squint-tools fetch` to download and verify it"
    size = path.stat().st_size
    if size != record.data["size_bytes"]:
        return (
            f"{path} is {size} bytes, the record says {record.data['size_bytes']}; "
            "run `squint-tools fetch` again"
        )
    return None


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def _open(source: str, file: str):
    """A readable stream for one file of the source: a directory or an http(s) URL prefix."""
    base = Path(source)
    if base.is_dir():
        return (base / file).open("rb")
    if not source.startswith(("https://", "http://")):
        raise FetchError(f"{source!r} is neither a directory nor an http(s) URL")
    return urllib.request.urlopen(f"{source.rstrip('/')}/{file}")


def fetch(
    record: Record,
    models: Path | None = None,
    source: str | None = None,
    log=sys.stderr,
) -> Path:
    """Fetches every file of the record into ``<models>/<record name>/``, verifying each hash.

    A file already present with the right hash is kept. A download whose bytes do not hash to the
    record is discarded before it lands under its name, and the whole fetch fails. Returns the
    directory the files were placed in.
    """
    target = (models or models_dir()) / record.name
    target.mkdir(parents=True, exist_ok=True)
    for file, want in record.files.items():
        dest = target / file
        if dest.exists() and sha256_of(dest) == want:
            print(f"fetch: {file} present", file=log)
            continue
        where = source if source is not None else record.release_url("").rstrip("/")
        print(f"fetch: {file} from {where}", file=log)
        h = hashlib.sha256()
        with tempfile.NamedTemporaryFile(dir=target, prefix=f".{file}.", delete=False) as tmp:
            try:
                with _open(where, file) as stream:
                    for chunk in iter(lambda: stream.read(1 << 20), b""):
                        h.update(chunk)
                        tmp.write(chunk)
                got = h.hexdigest()
                if got != want:
                    raise FetchError(
                        f"{file}: sha256 {got} is not the recorded {want}; nothing kept"
                    )
            except FetchError:
                os.unlink(tmp.name)
                raise
            except (OSError, ValueError) as e:
                # Whatever broke the stream, or an address urllib cannot open: nothing is kept.
                os.unlink(tmp.name)
                raise FetchError(f"{file}: cannot fetch: {e}") from e
        shutil.move(tmp.name, dest)
    return target
