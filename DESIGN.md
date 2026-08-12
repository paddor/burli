# Design

TODO: fill this after codec internals exist.

This document should explain the implemented internals only:

- decode state machine;
- bit reader and Huffman table layout;
- ring buffer and copy semantics;
- static dictionary transform path;
- encoder quality policy;
- match finder design;
- block split and context modeling;
- entropy coding;
- SIMD dispatch;
- unsafe boundary;
- streaming state reuse.
