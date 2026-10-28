"""burli must decode Google Brotli streams, write streams Google Brotli
decodes, and follow the API of Google's ``brotli`` module."""

import array
import random

import pytest

import burli

brotli = pytest.importorskip("brotli")


def text(size):
    words = [b"brotli ", b"window ", b"literal ", b"context ", b"copy ", b"the\n"]
    rng = random.Random(size)
    out = bytearray()
    while len(out) < size:
        out += rng.choice(words)
    return bytes(out[:size])


def noise(size):
    return random.Random(size).randbytes(size)


PAYLOADS = {
    "empty": b"",
    "byte": b"x",
    "text": text(100_000),
    "noise": noise(50_000),
    "zeros": bytes(200_000),
    "mixed": text(30_000) + noise(20_000) + text(30_000),
}


def pieces(data, seed, largest):
    rng = random.Random(seed)
    pos = 0
    while pos < len(data):
        size = rng.randint(1, largest)
        yield data[pos : pos + size]
        pos += size


@pytest.mark.parametrize("quality", range(12))
@pytest.mark.parametrize("name", PAYLOADS)
def test_google_decodes_burli_output(name, quality):
    payload = PAYLOADS[name]
    assert brotli.decompress(burli.compress(payload, quality=quality)) == payload


@pytest.mark.parametrize(
    ("mode", "lgwin", "lgblock"),
    [(burli.MODE_TEXT, 16, 0), (burli.MODE_FONT, 10, 16), (burli.MODE_GENERIC, 24, 24)],
)
def test_compress_takes_brotli_parameters(mode, lgwin, lgblock):
    payload = PAYLOADS["mixed"]
    encoded = burli.compress(payload, mode=mode, quality=4, lgwin=lgwin, lgblock=lgblock)
    assert brotli.decompress(encoded) == payload


@pytest.mark.parametrize("quality", range(12))
@pytest.mark.parametrize("lgwin", [10, 22])
@pytest.mark.parametrize("name", PAYLOADS)
def test_decodes_google_output(name, quality, lgwin):
    payload = PAYLOADS[name]
    if quality >= 10:
        payload = payload[:10_000]
    encoded = brotli.compress(payload, quality=quality, lgwin=lgwin)
    assert burli.decompress(encoded) == payload
    decompressor = burli.Decompressor()
    decoded = b"".join(decompressor.process(piece) for piece in pieces(encoded, quality, 999))
    assert decoded == payload
    assert decompressor.is_finished()


@pytest.mark.parametrize("quality", [0, 1, 5, 11])
def test_compressor_writes_streams_in_pieces(quality):
    payload = PAYLOADS["mixed"]
    compressor = burli.Compressor(quality=quality)
    encoded = b"".join(compressor.process(piece) for piece in pieces(payload, 1, 5000))
    encoded += compressor.finish()
    assert brotli.decompress(encoded) == payload


def test_compressor_flush_makes_input_so_far_decodable():
    compressor = burli.Compressor(quality=5)
    decompressor = brotli.Decompressor()
    decoded = b""
    for piece in pieces(PAYLOADS["text"], 2, 20_000):
        compressor.process(piece)
        decoded += decompressor.process(compressor.process(b"") + compressor.flush())
        assert PAYLOADS["text"].startswith(decoded)
    decoded += decompressor.process(compressor.finish())
    assert decoded == PAYLOADS["text"]
    assert decompressor.is_finished()


def test_compressor_takes_no_input_after_finish():
    compressor = burli.Compressor()
    compressor.finish()
    for call in (lambda: compressor.process(b"x"), compressor.flush, compressor.finish):
        with pytest.raises(burli.error):
            call()


@pytest.mark.parametrize(
    "parameters",
    [{"mode": 3}, {"quality": 12}, {"quality": -1}, {"lgwin": 9}, {"lgwin": 25}, {"lgblock": 15}],
)
def test_rejects_invalid_parameters(parameters):
    with pytest.raises(burli.error):
        burli.compress(b"x", **parameters)
    with pytest.raises(burli.error):
        burli.Compressor(**parameters)


@pytest.mark.parametrize("limit", [1, 1000, 65536])
def test_output_buffer_limit_bounds_each_call(limit):
    payload = PAYLOADS["zeros"] + PAYLOADS["text"]
    encoded = brotli.compress(payload, quality=11)
    decompressor = burli.Decompressor()
    decoded = b""
    # The loop urllib3 runs: drain while the decoder holds input or output.
    for piece in pieces(encoded, limit, 4000):
        chunk = decompressor.process(piece, output_buffer_limit=limit)
        assert len(chunk) <= limit
        decoded += chunk
        while not decompressor.can_accept_more_data():
            chunk = decompressor.process(b"", output_buffer_limit=limit)
            assert len(chunk) <= limit
            # Each call returns output or takes in all held input.
            assert chunk or decompressor.can_accept_more_data()
            decoded += chunk
    assert decoded == payload
    assert decompressor.is_finished()


def test_rejects_input_while_it_holds_input():
    encoded = brotli.compress(PAYLOADS["text"])
    decompressor = burli.Decompressor()
    decompressor.process(encoded, output_buffer_limit=100)
    assert not decompressor.can_accept_more_data()
    with pytest.raises(burli.error):
        decompressor.process(b"x")
    with pytest.raises(burli.error):
        decompressor.is_finished()


def test_rejects_data_after_the_stream():
    encoded = brotli.compress(b"payload")
    with pytest.raises(burli.error):
        burli.decompress(encoded + b"x")
    with pytest.raises(burli.error):
        burli.Decompressor().process(encoded + b"x")


# Reserved bits set, and a large-window header.
@pytest.mark.parametrize("corrupt", [b"\xff\xff\xff", b"\x11\x00"])
def test_rejects_malformed_streams(corrupt):
    with pytest.raises(burli.error):
        burli.decompress(corrupt)
    decompressor = burli.Decompressor()
    with pytest.raises(burli.error):
        decompressor.process(corrupt)
    with pytest.raises(burli.error):
        decompressor.process(b"")


def test_truncated_stream_is_not_finished():
    encoded = brotli.compress(PAYLOADS["text"])
    with pytest.raises(burli.error):
        burli.decompress(encoded[:-5])
    decompressor = burli.Decompressor()
    decoded = decompressor.process(encoded[:-5])
    assert PAYLOADS["text"].startswith(decoded)
    assert not decompressor.is_finished()


@pytest.mark.parametrize(
    "wrap", [bytes, bytearray, memoryview, lambda data: array.array("B", data)]
)
def test_takes_buffers(wrap):
    payload = PAYLOADS["text"][:5000]
    encoded = burli.compress(wrap(payload))
    assert burli.decompress(wrap(encoded)) == payload
    assert burli.Compressor().process(wrap(payload)) is not None
    assert burli.Decompressor().process(wrap(encoded)) == payload


def test_rejects_other_arguments():
    with pytest.raises(TypeError):
        burli.decompress("text")
    with pytest.raises(TypeError):
        burli.compress(memoryview(b"abcd")[::2])


def test_matches_brotli_module_names():
    for name in ("compress", "decompress", "Compressor", "Decompressor", "error"):
        assert hasattr(burli, name)
    assert (burli.MODE_GENERIC, burli.MODE_TEXT, burli.MODE_FONT) == (
        brotli.MODE_GENERIC,
        brotli.MODE_TEXT,
        brotli.MODE_FONT,
    )
    assert issubclass(burli.error, Exception)
