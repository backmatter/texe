# Performance comparison: texe, Tectonic, and latexmk

## Save-to-visible-PDF check

The real editor harness now records three warm edits in `results.json` under
`saveToVisiblePdfMillis`. This includes saving, the debounce interval, the full
texe build, and rendered text appearing in LaTeX Workshop's PDF webview.

A Linux managed-provider run on 2026-09-08 using the optimized packaged suite
and pqty commit `109fe6054debfd6044351778bb216adef7822a41` recorded 2052, 1570,
and 1349 ms (median 1570 ms). An earlier run recorded 999, 1199, and 1094 ms.
These are small synthetic papers on a shared development machine, not a
cross-platform performance promise. CI records the same measurement for each
supported platform so regressions can be compared using its retained evidence.
The harness also verifies that LaTeX Workshop's Build button shares the
companion queue rather than starting an independent compiler.

## Follow-up implementation

The next texe/pqty pass implemented three changes:

- Local registry scans inspect each directory once, with a per-scan memory
  budget and ordinary file-lookup fallback. Nothing is cached across scans.
- Local locks reuse their resolved closure when registry identity, package
  requirements, verified store manifests and current package bytes match.
  Same-timestamp content changes invalidate reuse. Installation still verifies
  cached objects, and new runtime dependencies still converge normally.
- System directory queries use one Kpathsea invocation instead of five.

Interleaved release-binary comparisons against the suite used below:

| Fixture | Previous edit median | Updated edit median |
| --- | ---: | ---: |
| Small paper, five edits | 1.762 s | 1.336 s |
| Math, links, contents and cross-references, nine edits | 1.867 s | 1.463 s |

That is about 24% and 22% respectively. The first five-edit run of the longer
fixture was noisy (2.091 s versus 2.035 s); the nine-edit follow-up checked that
the change also helped beyond the smallest fixture. Other machine workloads
were active, so these are local measurements rather than performance promises.
Use `scripts/benchmark-build.py --fixture references` for the longer fixture.

Validation covers installed-file additions/removals, changed metadata and
requirements, same-timestamp package edits, corrupt store objects, symlinks,
unlistable directories and cache-budget fallback. The real TeX journey checks
warm edits, newly introduced runtime dependencies and failed-publication recovery.

Persistent session reuse and system no-op caching still require broader host
dependency tracking. This pass reduces preparation cost while continuing to
observe installed files on every build.

## Original comparison

Measured locally on 2026-09-08 after the warm-convergence fixes. Texe still has
substantial orchestration overhead on small documents. Reducing preparation
work is a better next target than reducing TeX passes further.

## Method and results

Linux, optimized texe/pqty suite, system TeX Live, Tectonic 0.17.0 official GNU
Linux binary, and latexmk 4.87. Each tool used a separate project. Texe and
Tectonic caches were isolated from the normal user setup and warmed before
measurement. Network downloads and format generation are excluded.

The document was `article` with one sentence, changing its numeric counter on
each edit. SyncTeX was enabled for every tool. Results are full process wall
time: median of five prose edits and three subsequent unchanged invocations.
The TeX Live cases were interleaved; the two Tectonic cases were interleaved in
a separate batch. No compilation ran alongside these measurements.

| Build command | Prose edit | Unchanged invocation |
| --- | ---: | ---: |
| texe, system pdfLaTeX | 1.671 s | 1.919 s |
| texe, system XeLaTeX | 1.898 s | 2.310 s |
| latexmk, pdfLaTeX | 0.360 s | 0.125 s |
| latexmk, XeLaTeX | 0.719 s | 0.113 s |
| Tectonic, default intermediate handling | 0.696 s | 0.660 s |
| Tectonic, `--keep-intermediates` | 0.584 s | 0.586 s |
| texe, system pdfLaTeX, `--frozen` | 0.604 s | 0.597 s |

Texe used `build --offline --json`; latexmk used `-norc`, `-pdf` or `-xelatex`,
`-interaction=nonstopmode -halt-on-error -synctex=1`; Tectonic used
`--only-cached --synctex`, with automatic reruns left enabled.

This is an orchestration microbenchmark, not a large-document ranking.
Tectonic uses its bundled XeTeX-derived engine and a different TeX distribution;
its output and engine costs are not identical to system pdfLaTeX or XeLaTeX.
Texe also verifies a locked package environment, which plain latexmk does not.
The managed provider's existing no-op cache was not measured here.

## Where the time goes

Across the eight measured system pdfLaTeX invocations, texe's median phases were
1.092 s for packages, 0.364 s for toolchain preparation, and 0.233 s for the
final engine phase. With `--frozen`, package preparation fell to 0.027 s.
These phase medians need not sum to the median command time.

All warm texe builds already used one TeX pass and zero convergence rounds.
Tectonic used two passes with its default intermediate handling and one when
retaining intermediates. Its driver caches formats, buffers intermediate files
in memory, and compares read/write digests to decide reruns. These are useful
techniques, but do not amount to incremental typesetting of changed paragraphs.
See the [Tectonic driver](https://github.com/tectonic-typesetting/tectonic/blob/tectonic%400.17.0/src/driver.rs)
and [CLI options](https://tectonic-typesetting.github.io/book/latest/ref/v1cli.html).

## Recommended work, in order

1. **Reuse package resolution for prose-only edits.** The current local lock
   path reloads the database, checks installed runfiles, resolves and hydrates
   again. Retain a validated environment when package requirements and the
   toolchain are unchanged. Invalidation must cover installed package changes,
   changed local inputs, and newly discovered runtime dependencies; cached
   store verification must remain effective. The frozen measurement estimates
   the opportunity, but making every editor build frozen would change behavior
   and prevent normal dependency discovery.
2. **Reuse preparation within a watch/editor session.** Toolchain resolution
   currently repeats executable identity checks, capabilities and several
   `kpsewhich` calls. A session can retain this state and invalidate it on
   manifest, executable, configuration or relevant environment changes. This
   is a smaller target than package refresh, but measurable.
3. **Skip unchanged system builds with validated dependency tracking.** The
   managed provider already has a no-op fast path; the system provider
   deliberately does not. Track actual inputs, outputs and host configuration
   before extending reuse to it. Latexmk demonstrates the user-visible benefit
   of this behavior; its [manual](https://www.cantab.net/users/johncollins/latexmk/latexmk-480.txt)
   describes dependency and change tracking.

Do not start by embedding another TeX engine, caching custom document preambles,
or parallelizing dependent TeX passes. The measured engine phase is already
short, and those changes introduce compatibility work without addressing the
largest delay. A sub-second warm edit is a reasonable engineering target for
this fixture, supported by the frozen result, not a promised general speedup.
