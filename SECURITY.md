# Security policy

## Supported versions

The latest release and the current `main` branch receive security fixes. Older
releases may require upgrading rather than receiving a backport.

## Reporting a vulnerability

Do not publish vulnerability details in an issue, pull request, or discussion.
Use GitHub's
[private vulnerability report](https://github.com/backmatter/texe/security/advisories/new)
to send the maintainers:

- the affected version and platform;
- the smallest reproduction that is safe to share;
- the expected and observed trust boundary;
- potential impact and any known workarounds.

Private vulnerability reporting must be enabled before this repository is made
public. If GitHub does not show the form, do not submit details publicly. A
public issue may say only that the private form is unavailable so maintainers
can restore it.

The most sensitive areas are project-controlled command execution, archive and
path confinement, managed-runtime integrity, cleanup, the loopback viewer, and
the pqty command-suite boundary.

## What to expect

Maintainers aim to acknowledge a usable report within three business days.
They will investigate privately, keep the reporter informed at meaningful
milestones, and coordinate a fix and disclosure appropriate to the impact.
Supported-platform verification may be required before publication.

Please allow maintainers a reasonable opportunity to remediate a confirmed
issue before public disclosure. Credit and disclosure timing will be agreed
with the reporter whenever practical.
