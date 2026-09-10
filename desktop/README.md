# Native welcome apps

The macOS and Windows apps create papers, open existing projects, build PDFs,
and launch VS Code or a browser preview. New papers get a named subfolder in
the selected directory. Existing project sources are preserved.

If VS Code is missing, the app offers to install it. Choose your own editor to
open the project folder and use the browser preview instead. Build output is
available through **Show details**. A failed build keeps the previous PDF.

See [installation](../docs/install.md).

## Building

First assemble the portable release suite with `cargo xtask package suite`.
Extract it, then run the matching script on its native platform, replacing
`X.Y.Z` with the release version:

```sh
# Apple Silicon Mac, Xcode command-line tools installed
./scripts/package-macos-app.sh /path/to/suite/bin dist/texe-aarch64-macos.dmg X.Y.Z
```

```powershell
# Windows x64, Inno Setup 6 installed; the .NET Framework compiler ships with Windows
./scripts/package-windows-app.ps1 -SuiteBin C:\suite\bin -OutputDir dist -Version X.Y.Z
```

## Signing

Without signing credentials, packaging produces an ad-hoc Mac signature or an
unsigned Windows installer.

On macOS, provide `TEXE_MACOS_SIGN_IDENTITY` for an installed Developer ID
Application identity. Provide `TEXE_MACOS_NOTARY_PROFILE` for credentials already
stored with `xcrun notarytool store-credentials`. Packaging signs each executable
and the bundle with the hardened runtime, notarizes and staples the app, then
signs, notarizes and staples the disk image. See Apple's
[notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).

On Windows, put `signtool` on PATH and set `TEXE_WINDOWS_CERT_THUMBPRINT` to a
certificate in the current user's certificate store, plus
`TEXE_WINDOWS_TIMESTAMP_URL` to your signing provider's HTTPS timestamp service.
Packaging signs the app and suite; Inno Setup signs the installer and uninstaller.
The private key is never copied into the bundle. See
[Inno Setup signing](https://jrsoftware.org/ishelp/topic_setup_signtool.htm).
