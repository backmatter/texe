# Privacy and network behavior

texe has no telemetry, account, analytics, advertising, crash upload, or
remote paper service. Source files, bibliography databases, titles, authors,
logs, and generated PDFs stay on the computer.

## Connections texe may make

- The first managed build downloads the selected LaTeX engine and required
  package archives from TeX Live mirrors. Every accepted download is checked
  against its recorded size and cryptographic digest.
- Choosing VS Code setup asks the local `code` command to install LaTeX
  Workshop when it is missing and to install texe's bundled layout companion.
  texe does not force-update an existing LaTeX Workshop installation; its own
  companion follows the installed texe version. Any extension-marketplace
  connection is made by VS Code; the bundled companion performs no networking.
- `texe watch --view` binds a random port on `127.0.0.1`. It serves only the
  pinned PDF.js viewer resources, its local reload/state bridge, the current
  PDF, and a generation counter. It has no CDN or analytics dependency and
  never serves project source or a directory listing. PDF.js help links or
  links in the paper can leave the local viewer only when the user clicks
  them.

`texe build --offline` forbids managed runtime, component, registry, and
package network access. It succeeds only when every required verified cache
entry is already present.

## Technical trust boundaries

Mirror transport is not trusted to choose build bytes: size and cryptographic
digest checks happen before an archive becomes a cache entry. Archive paths
are confined during extraction. Managed engine runs clear inherited TeX, Lua,
and font search variables and receive a `PATH` containing only managed
commands.

The explicit exceptions are the `system` provider,
`toolchain.shell_escape = true`, and command overrides paired with
`toolchain.allow_unmanaged_commands = true`. Managed mode rejects those
overrides without the explicit opt-out. Opted-out builds warn and do not use
the no-op build cache. The default managed command suite must be installed
beside texe, and its executable bytes participate in the cache identity.
