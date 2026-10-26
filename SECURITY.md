# Security

Report security issues privately first.

Security design goals:

- bounded-output decode API;
- no panics on malformed input;
- no `unsafe` code;
- fuzz targets for untrusted decode paths;
- differential tests against Google Brotli.
