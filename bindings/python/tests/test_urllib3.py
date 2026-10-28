"""urllib3 must decode Brotli responses through burli in place of
``brotli``, bounding each read by its size."""

import subprocess
import sys
import textwrap

import pytest

pytest.importorskip("brotli")
pytest.importorskip("urllib3")

SCRIPT = textwrap.dedent(
    """
    import http.server
    import random
    import sys
    import threading

    import brotli as google_brotli
    import burli

    sys.modules["brotli"] = burli
    import urllib3
    import urllib3.response

    assert urllib3.response.brotli is burli

    rng = random.Random(1)
    words = [b"brotli ", b"urllib3 ", b"response ", b"\\n"]
    body = b"".join(rng.choice(words) for _ in range(200_000)) + bytes(5_000_000)
    encoded = google_brotli.compress(body, quality=9)

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Encoding", "br")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_address[1]}/"

    assert urllib3.request("GET", url).data == body

    response = urllib3.request("GET", url, preload_content=False)
    parts = []
    while part := response.read(amt=4096):
        assert len(part) <= 4096
        parts.append(part)
    assert b"".join(parts) == body

    response = urllib3.request("GET", url, preload_content=False)
    assert b"".join(response.stream(65536)) == body
    server.shutdown()
    print("ok")
    """
)


def test_urllib3_decodes_brotli_responses_through_burli():
    result = subprocess.run(
        [sys.executable, "-c", SCRIPT], capture_output=True, text=True, timeout=120
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "ok"
