# Bundled PDF.js distribution

texe embeds the official modern PDF.js generic viewer so that PDF navigation,
zoom, search, printing, and accessibility remain owned by the upstream viewer
rather than reimplemented by texe.

- Version: `5.7.284`
- Release: <https://github.com/mozilla/pdf.js/releases/tag/v5.7.284>
- Asset: `pdfjs-5.7.284-dist.zip`
- SHA-256: `6d1b81252d76358df5831567d7d551f40ebae0cd8e0a554694bc4df0d3db8715`
- License: Apache License 2.0; see `LICENSE`

The archive is kept intact. At runtime texe exposes only the generic viewer's
required modules, styles, localization files, images, fonts, CMaps, ICC
profiles, and WebAssembly resources on its loopback-only server. Source maps
and the example PDF in the distribution are not served.
