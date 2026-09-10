# Install texe

texe installs as one small command suite: the `texe` command you use and the
`pqty` and `pqty-fls` helpers it calls. You do not need TeX Live, Rust, Git,
Python, or Node.js.

## Check that your computer is supported

| Computer | How to recognize it |
| --- | --- |
| Linux x86-64 | A 64-bit Intel or AMD computer. `uname -m` prints `x86_64`. |
| Windows x86-64 | Settings → System → About shows an x64-based processor, not ARM. |
| macOS Apple Silicon | About This Mac shows an Apple chip such as M1, M2, M3, or M4, not Intel. |

Intel Macs, ARM Linux, and Windows on ARM are not supported.

## Install

On macOS and Linux, run this in Terminal:

```sh
curl -LsSf https://github.com/backmatter/texe/releases/latest/download/install.sh | bash
```

On Windows, run this in PowerShell:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/backmatter/texe/releases/latest/download/install.ps1 | iex"
```

The installer picks the build for your computer, verifies its checksum, and
installs below `~/.local/bin` on macOS and Linux or into
`%LOCALAPPDATA%\Programs\texe` on Windows. It never needs `sudo` or an
administrator prompt, does not install Homebrew, and does not ask you to disable
Gatekeeper. It adds its directory to your command path only when that directory
is not already there.

Open a new terminal window afterward, then check the installation:

```sh
texe --version
```

Once installed, run `texe` to [create a paper](../README.md#get-started).

## Other ways to install

The installer is a plain script, so you can read it before running it:

```sh
curl -LsSf https://github.com/backmatter/texe/releases/latest/download/install.sh | less
```

On Debian or Ubuntu, the `.deb` package installs the commands into `/usr/bin`
instead:

```sh
release_url="$(curl -fsSL -o /dev/null -w '%{url_effective}' \
  https://github.com/backmatter/texe/releases/latest)"
tag="${release_url##*/}"
curl -fLO "https://github.com/backmatter/texe/releases/download/${tag}/texe_${tag#v}_amd64.deb"
sudo apt install "./texe_${tag#v}_amd64.deb"
```

To install on a computer without network access, download the archive for that
computer from the
[latest release](https://github.com/backmatter/texe/releases/latest) and point
the installer at it:

```sh
bash install.sh --from texe-aarch64-macos.tar.gz
```

```powershell
.\install.ps1 -From .\texe-x86_64-windows.zip
```

Both scripts accept `--prefix`, or `-Prefix` on Windows, to install somewhere
other than the default location.

## Optional desktop apps

The Windows and macOS apps give you graphical setup and bundle their own private
command suite. They do not add `texe` to your shell PATH, so use the
instructions above as well if you want the terminal command.

Releases that ship desktop installers list these assets:

- Windows: `texe-x86_64-windows-setup.exe`. Run it, then open texe from Start.
- macOS: `texe-aarch64-macos.dmg`. Open it and drag texe into Applications.

In the app, click **New paper**, enter the title and author, and confirm the
editor, engine, and location. **Change…** picks the parent folder; texe creates
a subfolder named after the paper. **Create paper** offers to install VS Code if
it is missing and reports **Your paper is ready** when the first build finishes;
trust the folder in VS Code when asked.

Afterwards, **Open a paper…** reopens an existing project, **Build again**
rebuilds it, and **Show files** opens its folder.

## Verify a release

Every release includes `SHA256SUMS` and GitHub build provenance, which verify
file integrity and build origin. They are not Apple or Windows code signatures:
the Windows installer is unsigned, and the Mac app is ad-hoc signed but not
notarized.

```sh
gh attestation verify <archive> -R backmatter/texe   # check a downloaded archive
texe doctor --verify-toolchain                       # recheck an installation
```

## Upgrade and uninstall

texe never updates itself. To upgrade, run the install command again; it
replaces the commands in place. With the Debian package or a desktop app,
upgrade the same way you installed it.

Uninstalling keeps your papers and PDFs. It also keeps the downloaded TeX
runtimes and caches, so a reinstall does not download them again. Run
`texe clean --all` first if you want those removed too.

- Windows app: **Settings → Apps → Installed apps → texe → Uninstall**.
- macOS app: quit texe and move **texe.app** to the Trash.
- Debian or Ubuntu: `sudo apt remove texe`.

If you used the install command, run the matching uninstall script. It removes
only the application files and their path entries:

```sh
curl -LsSf https://github.com/backmatter/texe/releases/latest/download/uninstall-unix.sh | bash
```

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/backmatter/texe/releases/latest/download/uninstall-windows.ps1 | iex"
```

## Troubleshooting

- **“Command not found” right after installing:** open a new Terminal or
  PowerShell window so the updated path is loaded.
- **`apt` is unavailable:** use the install command at the top of this page. The
  Debian package is only for Debian-based systems such as Ubuntu.
- **The installer reports an unsupported computer:** check the table at the top
  of this page.
- **The install command cannot reach GitHub:** download the archive on another
  computer and install it with `--from`, as described above.
- **The first build cannot download:** check your internet connection and retry.
- **A managed component fails verification:** run
  `texe doctor --verify-toolchain`. It names the damaged cache and what to do.
- **VS Code does not open:** start it normally and open the project folder, then
  run `texe editor` again once the `code` command works.
- **VS Code opens in Restricted Mode:** trust the project folder.

If the problem remains, run `texe doctor --verbose` and open a
[bug report](https://github.com/backmatter/texe/issues/new?template=bug_report.yml).
Remove confidential paper content, credentials, and private paths before
posting.
