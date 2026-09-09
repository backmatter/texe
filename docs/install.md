# Install texe

texe installs as one small command suite: the `texe` command you use and the
`pqty` and `pqty-fls` helper commands it calls. You do not need a separate TeX
Live installation, Rust, Git, Python, or Node.js.

## Check that your computer is supported

texe supports:

| Computer | How to recognize it |
| --- | --- |
| Linux x86-64 | A 64-bit Intel or AMD computer. `uname -m` prints `x86_64`. |
| Windows x86-64 | Settings → System → About shows an x64-based processor, not ARM. |
| macOS Apple Silicon | About This Mac shows an Apple chip such as M1, M2, M3, or M4, not Intel. |

If your computer is not in this table, read the
[support matrix](support.md) before installing.

## Linux x86-64

### Debian or Ubuntu

Open the [latest texe release](https://github.com/backmatter/texe/releases/latest),
expand **Assets**, and download the versioned package named
`texe_VERSION_amd64.deb`. Open the downloaded package with the system software
installer and choose **Install**. When it finishes, open Terminal and run:

```sh
texe --version
```

For a terminal-only installation that automatically finds the current version:

```sh
release_url="$(curl -fsSL -o /dev/null -w '%{url_effective}' \
  https://github.com/backmatter/texe/releases/latest)"
tag="${release_url##*/}"
version="${tag#v}"
package="texe_${version}_amd64.deb"
curl -fLO "https://github.com/backmatter/texe/releases/download/${tag}/${package}"
sudo apt install "./${package}"
texe --version
```

Every release keeps its version in the Debian filename; no unversioned Debian
asset is published.

### Other Linux distributions or no administrator access

Use the portable installer:

```sh
curl -fLO https://github.com/backmatter/texe/releases/latest/download/texe-x86_64-linux.tar.gz
curl -fLO https://github.com/backmatter/texe/releases/latest/download/SHA256SUMS
curl -fLo install-texe.sh https://github.com/backmatter/texe/releases/latest/download/install-linux.sh
grep '  texe-x86_64-linux.tar.gz$' SHA256SUMS | sha256sum -c -
sh install-texe.sh --from texe-x86_64-linux.tar.gz
```

This installs below `~/.local/bin` without `sudo`. Open a new Terminal window
afterward so the updated command path is loaded. The installer changes
`~/.profile` only when `~/.local/bin` is not already available.

## Windows x86-64

Download [texe for Windows](https://github.com/backmatter/texe/releases/latest/download/texe-x86_64-windows-setup.exe),
open the installer, and choose **Install**. Leave **Open texe** selected when it
finishes, or launch **texe** from the Start menu. Installation is for your user
account and does not require administrator access.

## macOS on Apple Silicon

Download [texe for Mac](https://github.com/backmatter/texe/releases/latest/download/texe-aarch64-macos.dmg),
open the disk image, and drag **texe** into **Applications**. Eject the disk
image, then open **texe** from Applications.

These graphical downloads are produced starting with the next release containing
the native apps. Older releases have only the command-line installers below.
Signing depends on the release's configured credentials; unsigned downloads may
show operating-system security prompts. Release notes should state whether a
build is signed and, on macOS, notarized.

## Create your first paper in the app

1. Enter a folder name, paper title, and author.
2. Choose **VS Code** or **My own editor + browser preview**. VS Code must
   already be installed; the app finds its usual installation location.
3. Choose **Create a paper…** and pick the folder that should contain it.
4. Watch setup and build progress in the window. The first build downloads the
   required LaTeX tools and packages automatically.

With VS Code, texe configures the project, installs missing extensions, and opens
its source and PDF. Trust your project folder when VS Code asks. If editor setup
needs attention, the details remain in the welcome window and your PDF is kept.

With your own editor, texe opens the project folder and a live PDF preview in your
browser. Open the `.tex` source in your preferred editor and keep texe running;
saving the source rebuilds the preview. Closing texe stops its live preview.

Use **Open a paper…** for an existing project. Select its folder, then its main
`.tex` file if the project does not yet have `texe.toml`. Existing projects keep
their configured engine. New projects default to pdfLaTeX; LuaLaTeX is also
available. **Build again** retries a failed build or applies a changed editor
choice. **Show files** returns to the project folder.

No Terminal or PowerShell steps are required for this workflow. The graphical
apps bundle their own command suite; they do not change your shell PATH. The
CLI installers below remain available for shell use.

## Advanced: command-line installation

<details>
<summary>Windows and macOS command-line installers</summary>

### Windows command-line installation

The GitHub release works immediately through the direct PowerShell method
below. After the generated manifest has been accepted into the public WinGet
repository, you can instead run:

```powershell
winget install --exact --id Backmatter.Texe
texe --version
```

WinGet checks the release archive and installs the three commands for the
current user.

#### Direct PowerShell installation

Copy this complete block into PowerShell:

```powershell
Invoke-WebRequest https://github.com/backmatter/texe/releases/latest/download/texe-x86_64-windows.zip -OutFile texe.zip
Invoke-WebRequest https://github.com/backmatter/texe/releases/latest/download/SHA256SUMS -OutFile SHA256SUMS
Invoke-WebRequest https://github.com/backmatter/texe/releases/latest/download/install-windows.ps1 -OutFile install-texe.ps1
$line = Get-Content SHA256SUMS | Where-Object { $_ -match '  texe-x86_64-windows\.zip$' }
$expected = $line.Split()[0]
if ((Get-FileHash texe.zip -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw "Checksum mismatch" }
.\install-texe.ps1 -From .\texe.zip
```

Open a new PowerShell window afterward, then run `texe --version`. The direct
installer uses `%LOCALAPPDATA%\Programs\texe` and changes only the current
user's command path.

### macOS command-line installation

The one-off installer below works immediately from the GitHub release:

```sh
curl -fLo install-texe.sh https://github.com/backmatter/texe/releases/latest/download/install-macos.sh
bash install-texe.sh
```

After the generated formula has been published to the backmatter tap, Homebrew
users can instead run:

```sh
brew install backmatter/tap/texe
texe --version
```

The one-off script downloads and verifies the archive and each command, then installs
below `~/.local/bin`. It does not use `sudo`, install Homebrew, or ask you to
disable Gatekeeper. Open a new Terminal window afterward.

For a previously downloaded or offline archive, use:

```sh
bash install-texe.sh --from texe-aarch64-macos.tar.gz
```

</details>

## Create your first paper from the command line

After a command-line installation:

1. Open a new Terminal window, or PowerShell on Windows.
2. Run `texe --version` to confirm the command is available.
3. Move into the folder that should contain the new paper.
4. Run `texe`.
5. Follow the guided setup to create and build the first PDF.

The first build needs an internet connection and takes longer because it
downloads the required LaTeX runtime and packages. texe explains the download
before it begins. When the build finishes, it prints the paths to `main.tex`
and `main.pdf` and suggests the next command.

Return to the [first-paper guide](../README.md#create-your-first-paper) for the
writing workflow.

## Verify a release

This section is optional. Every release includes one aggregate `SHA256SUMS`
file and GitHub build provenance for all release assets. Portable Windows and macOS
archives are not platform-signed. Native app signatures depend on the release
configuration; see that release’s notes.

Advanced users can independently verify a downloaded archive with:

```sh
gh attestation verify <archive> -R backmatter/texe
```

After installing, this command rechecks the complete managed runtime and
command-suite compatibility:

```sh
texe doctor --verify-toolchain
```

## Upgrade and uninstall

Upgrade texe through the same package manager or installer used for the
original installation. texe does not check for or install application updates
itself.

To remove downloaded managed runtimes and caches too, run this before
uninstalling the command suite:

```sh
texe clean --all
```

Graphical uninstall:

- Windows: open **Settings → Apps → Installed apps**, find **texe**, and choose
  **Uninstall**.
- macOS: quit texe and move **texe.app** from Applications to the Trash.

These actions keep your papers, PDFs, and downloaded TeX caches.

Command-line package uninstall:

- Debian or Ubuntu: `sudo apt remove texe`
- Windows WinGet: `winget uninstall --exact --id Backmatter.Texe`
- macOS Homebrew: `brew uninstall texe`

Portable uninstall on Linux or macOS:

```sh
curl -fLo uninstall-texe.sh https://github.com/backmatter/texe/releases/latest/download/uninstall-unix.sh
sh uninstall-texe.sh
```

Portable uninstall on Windows:

```powershell
Invoke-WebRequest https://github.com/backmatter/texe/releases/latest/download/uninstall-windows.ps1 -OutFile uninstall-texe.ps1
.\uninstall-texe.ps1
```

The uninstall scripts remove only application files and their PATH entries.
Managed runtimes and caches remain so a reinstall does not need to download
them again. Projects and their PDFs are never removed.

## Troubleshooting

- **“Command not found” immediately after a portable install:** open a new
  Terminal or PowerShell window so the updated command path is loaded.
- **`apt` is unavailable:** use the portable Linux installation; the Debian
  package is only for Debian-based systems such as Ubuntu.
- **WinGet cannot find texe:** use the direct PowerShell installation.
- **The installer reports an unsupported computer:** compare the computer with
  the [support matrix](support.md). ARM Windows and Linux, and Intel Macs, are
  not currently supported.
- **The first build cannot download:** check the internet connection and retry.
  After the caches are populated, `texe build --offline` forbids network use.
- **A managed component fails verification:** run
  `texe doctor --verify-toolchain`; the error names the damaged cache and the
  next action.
- **VS Code does not open:** start it normally and open the project folder. Run
  `texe editor` again after the `code` command becomes available.
- **VS Code opens in Restricted Mode:** trust the project folder to enable its
  texe and LaTeX Workshop integration.

If the problem remains, run:

```sh
texe doctor --verbose
```

Then open a
[bug report](https://github.com/backmatter/texe/issues/new?template=bug_report.yml).
Remove confidential paper content, credentials, and private filesystem paths
before posting.
