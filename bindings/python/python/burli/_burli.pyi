from typing import Final

from typing_extensions import Buffer

__version__: Final[str]
MODE_GENERIC: Final[int]
MODE_TEXT: Final[int]
MODE_FONT: Final[int]

class error(Exception): ...

def compress(
    string: Buffer,
    mode: int = 0,
    quality: int = 5,
    lgwin: int = 22,
    lgblock: int = 0,
) -> bytes: ...
def decompress(string: Buffer) -> bytes: ...

class Compressor:
    def __init__(
        self,
        mode: int = 0,
        quality: int = 5,
        lgwin: int = 22,
        lgblock: int = 0,
    ) -> None: ...
    def process(self, data: Buffer) -> bytes: ...
    def flush(self) -> bytes: ...
    def finish(self) -> bytes: ...

class Decompressor:
    def __init__(self) -> None: ...
    def process(self, data: Buffer, output_buffer_limit: int | None = None) -> bytes: ...
    def is_finished(self) -> bool: ...
    def can_accept_more_data(self) -> bool: ...
