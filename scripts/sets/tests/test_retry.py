"""Generator HTTP calls outlast a transient provider failure and give up on a real one."""

import io
import urllib.error

import pytest

from squint_sets import generators


def http_error(code):
    return urllib.error.HTTPError("https://x", code, "err", {}, io.BytesIO(b'{"error": "x"}'))


class Response(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


def scripted(monkeypatch, outcomes):
    calls = []

    def urlopen(req, timeout):
        calls.append(req)
        o = outcomes[len(calls) - 1]
        if isinstance(o, Exception):
            raise o
        return Response(o)

    monkeypatch.setattr(generators.urllib.request, "urlopen", urlopen)
    return calls


def test_a_503_then_success_is_retried_after_the_first_wait(monkeypatch):
    calls = scripted(monkeypatch, [http_error(503), urllib.error.URLError("reset"), b"ok"])
    waits = []
    assert generators._open("https://x", 5, "x", sleep=waits.append) == b"ok"
    assert len(calls) == 3 and waits == list(generators.RETRY_WAITS[:2])


def test_a_client_error_is_not_retried(monkeypatch):
    calls = scripted(monkeypatch, [http_error(401)])
    with pytest.raises(generators.GeneratorError, match="HTTP 401"):
        generators._open("https://x", 5, "x", sleep=lambda s: None)
    assert len(calls) == 1


def test_a_lasting_outage_gives_up_after_every_wait(monkeypatch):
    n = len(generators.RETRY_WAITS) + 1
    calls = scripted(monkeypatch, [http_error(503)] * n)
    with pytest.raises(generators.GeneratorError, match="HTTP 503"):
        generators._open("https://x", 5, "x", sleep=lambda s: None)
    assert len(calls) == n
