# Security policy

## Supported versions

Only the latest release on the
[Releases](https://github.com/filipmares/tile/releases) page receives security
fixes. Tile checks for updates itself, so a fix ships as a new release.

## Reporting a vulnerability

Please report vulnerabilities privately. Do not open a public issue, pull
request or discussion.

1. Go to the repository's **Security** tab.
2. Choose **Report a vulnerability**, or use
   <https://github.com/filipmares/tile/security/advisories/new> directly.
3. Describe the problem, the affected version and platform, and the steps to
   reproduce it.

You should hear back within a week. Once the problem is confirmed, a fix is
prepared in a private advisory, released, and the advisory is published with
credit to you unless you ask otherwise.

## Scope

In scope: the Tile app, its installers, the update channel, and the release
workflow in this repository. Tile can install a low-level keyboard hook on Windows
and needs the Accessibility permission on macOS, so anything that lets another
process read keystrokes through Tile or move windows it should not is in scope.

Out of scope: problems in third-party dependencies that are already public
(report those upstream), and builds you compiled or modified yourself.
