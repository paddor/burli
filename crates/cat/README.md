# burli-cat

Concat fragments for burli Brotli streams.

This crate builds normal Brotli streams from validated headerless fragments.
It does not provide threading. Callers may encode fragments with their own
thread pool and assemble them in order.

See `FORMAT.md` for the fragment format.
