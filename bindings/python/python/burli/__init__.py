"""Memory-safe Brotli compression and decompression.

The API matches Google's ``brotli`` module, so existing code can switch
with::

    import burli as brotli

Qualities 6 to 11 run as 5.
"""

from ._burli import (
    MODE_FONT,
    MODE_GENERIC,
    MODE_TEXT,
    Compressor,
    Decompressor,
    __version__,
    compress,
    decompress,
    error,
)

version = __version__

__all__ = [
    "MODE_FONT",
    "MODE_GENERIC",
    "MODE_TEXT",
    "Compressor",
    "Decompressor",
    "__version__",
    "compress",
    "decompress",
    "error",
    "version",
]
