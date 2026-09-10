# Signing status

No platform-signing credentials are configured. The CLI is the default install.
Desktop apps are optional.

| Download | Current status |
| --- | --- |
| CLI archives | Checksums and GitHub build provenance; no platform signing |
| Windows desktop installer | Unsigned |
| macOS desktop app | Ad-hoc signed; not Developer ID signed or notarized |

## No-cost options

- **Windows:** [SignPath Foundation](https://signpath.org/) provides free signing
  for approved open-source projects. texe has not applied or been approved.
  Review its [conditions](https://signpath.org/terms.html) before applying.
- **macOS:** Developer ID signing and notarization require the
  [Apple Developer Program](https://developer.apple.com/support/compare-memberships/),
  normally USD 99 per year. Apple offers
  [fee waivers](https://developer.apple.com/help/account/membership/fee-waivers)
  for eligible nonprofits, educational institutions and government entities.
  Open-source status alone does not qualify a project.

Checksums, provenance, self-signed certificates and ad-hoc signatures do not
replace a trusted platform signature. Do not describe the current desktop
installers as signed releases or promise that operating-system warnings will
not appear.

Signing is not required to publish the CLI. If credentials become available,
the [desktop packaging scripts](../desktop/README.md#signing) support Developer ID
and notarization on macOS, and a certificate accessible to SignTool on Windows.
A hosted signing provider needs its own CI integration before it can be used.
