# burli-cat

Concat fragments for burli Brotli streams.

This crate builds normal Brotli streams from validated headerless fragments.
It does not provide threading. Callers may encode fragments with their own
thread pool and assemble them in order.

For RFC 7932 self-contained parts, use `assemble_rfc7932_parts`. It emits one
standard Brotli stream whose input parts use local history only and end on byte
boundaries. No Burli-specific wrapper is added.

See `FORMAT.md` for the fragment format.
