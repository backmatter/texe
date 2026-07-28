# Managed toolchain recipes

texe ships a closed catalog of reviewed TeX Live recipes. Project
configuration chooses a catalog alias or an exact recipe ID:

```toml
[toolchain]
provider = "managed"
engine = "pdflatex"
channel = "stable"
```

`toolchains/catalog.toml` owns policy aliases. A file below
`toolchains/recipes/` owns all release data for one immutable snapshot:

- the dated tlnet base and equivalent download sources;
- the decompressed registry SHA-256;
- engine executable, format identity, and bootstrap providers;
- runtime container SHA-512 and size for every supported target;
- the pinned Biber component and any platform bootstrap data.

Recipe files are embedded in the binary. At first use, texe parses the catalog
once and validates its schema, aliases, snapshot/file naming, dated HTTPS URL,
digests, portable identifiers, unique providers, and complete
engine-by-platform matrix. Project configuration cannot inject a URL or an
unreviewed recipe.

## Adding a snapshot

Generate the complete supported matrix:

```console
snapshot="<snapshot-id>"
archive_url="<dated-tlnet-url>"
cargo xtask snapshot \
  --snapshot "$snapshot" \
  --base "$archive_url" \
  > "toolchains/recipes/$snapshot.toml"
```

Then:

1. Review the source tree and every emitted digest.
2. Add or move aliases in `toolchains/catalog.toml`.
3. Pin the expected engine fingerprints in the toolchain tests.
4. Run the managed pdfLaTeX, LuaLaTeX, common-package, bibliography, and index
   xtask verification cases from clean caches.

Keep old recipe documents while released projects may still select them.
Removing one makes its runtime and downloads eligible for cache cleanup.
