# Releasing Tile

Releases are cut by pushing a `v*` tag. [`.github/workflows/release.yml`](../.github/workflows/release.yml)
then creates a **draft** GitHub Release, builds a universal macOS `.dmg` and the
Windows `.msi`/`.exe`, signs and notarizes the macOS artifacts, verifies the
result, and uploads everything. A human publishes the draft.

- [One-time macOS signing setup](#one-time-macos-signing-setup)
- [Cutting a release](#cutting-a-release)
- [What the workflow verifies](#what-the-workflow-verifies)
- [Troubleshooting](#troubleshooting)

## One-time macOS signing setup

Signing and notarization require a paid **Apple Developer Program** membership
($99/year). Without it the workflow still builds and publishes, but the macOS
artifacts are unsigned and the release notes tell users to strip the quarantine
attribute by hand.

Everything below is done once. The workflow detects the secrets automatically —
no workflow edit is needed to turn signing on.

### 1. Create a Developer ID Application certificate

A *Developer ID Application* certificate is the only kind that allows
distribution outside the App Store. **Developer ID Installer**, *Apple
Development* and *Apple Distribution* certificates will not work.

1. On a Mac, open **Keychain Access ▸ Certificate Assistant ▸ Request a
   Certificate From a Certificate Authority…**. Enter your Apple ID e-mail,
   choose **Saved to disk**, and save the `.certSigningRequest` file.
2. Go to <https://developer.apple.com/account/resources/certificates/add>,
   choose **Developer ID Application**, upload the request, and download the
   resulting `.cer`.
3. Double-click the `.cer` to import it into your login keychain.

### 2. Export it as a `.p12` and base64-encode it

In Keychain Access, find the certificate under **My Certificates**, expand it to
confirm a private key is attached, then right-click ▸ **Export…** and save as
`.p12` with a password. That password becomes `APPLE_CERTIFICATE_PASSWORD`.

```sh
# The value for APPLE_CERTIFICATE — a single line, no wrapping.
base64 -i certificate.p12 | pbcopy
```

Delete the `.p12` from disk afterwards; it contains your private key.

### 3. Find the signing identity and team ID

```sh
security find-identity -v -p codesigning
# 1) A1B2C3... "Developer ID Application: Your Name (ABCDE12345)"
#                ^ APPLE_SIGNING_IDENTITY is this whole quoted string
#                                          ^ APPLE_TEAM_ID is this 10-char id
```

### 4. Create an app-specific password for notarization

Your real Apple ID password will not work. Generate a dedicated one at
<https://appleid.apple.com> ▸ **Sign-In and Security ▸ App-Specific Passwords**.
It looks like `abcd-efgh-ijkl-mnop` and becomes `APPLE_PASSWORD`.

### 5. Add the repository secrets

**Settings ▸ Secrets and variables ▸ Actions ▸ New repository secret:**

| Secret | Value |
| ------ | ----- |
| `APPLE_CERTIFICATE` | Base64 of the `.p12` from step 2 |
| `APPLE_CERTIFICATE_PASSWORD` | Password used when exporting the `.p12` |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Your Name (ABCDE12345)` |
| `APPLE_ID` | Apple ID e-mail of the developer account |
| `APPLE_PASSWORD` | App-specific password from step 4 |
| `APPLE_TEAM_ID` | 10-character team ID, e.g. `ABCDE12345` |

`APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID` are all-or-nothing: the Tauri
bundler fails the build when the first two are set without the third, so the
workflow rejects a partial set up front with a clearer message.

Developer ID certificates expire after five years. Builds notarized before
expiry keep working, but new builds need a fresh certificate.

## Cutting a release

1. **Bump the version in all three places** — they must agree with the tag or
   the workflow stops before publishing anything:
   - `Cargo.toml` (workspace `version`)
   - `apps/tile/tauri.conf.json` (`version`)
   - `apps/tile/ui/package.json` (`version`)
2. Commit the bump and merge it to `main`.
3. Tag and push:

   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```

4. Watch the run: `gh run watch --workflow=release.yml`. The macOS job is the
   slow one — notarization typically takes a few minutes.
5. Review the draft release, then publish it:

   ```sh
   gh release view v0.2.0
   gh release edit v0.2.0 --draft=false --latest
   ```

To rebuild an existing tag without re-tagging, run the workflow manually:
`gh workflow run release.yml -f tag=v0.2.0`. Assets are uploaded with
`--clobber`, so a re-run replaces them.

### Verifying the download by hand

Worth doing at least once, ideally on a Mac that has never run Tile:

```sh
# Simulate a downloaded file — this is the attribute Gatekeeper reacts to.
xattr -w com.apple.quarantine "0081;00000000;Safari;" Tile_0.2.0_universal.dmg
open Tile_0.2.0_universal.dmg
```

It should mount and the app should launch with no security dialog beyond the
standard "downloaded from the internet" confirmation and the Accessibility
permission prompt.

## What the workflow verifies

Silent signing failures are the main hazard: an unsigned build looks identical
to a signed one until a user downloads it. The macOS job therefore asserts the
end state rather than trusting the bundler, and fails the release if any check
does not hold:

- `codesign --verify --deep --strict` on the `.app` and the `.dmg`
- the hardened runtime flag is present (notarization requires it)
- `spctl --assess` reports `accepted` / `source=Notarized Developer ID`
- `xcrun stapler validate` passes on both the `.app` and the `.dmg`

The Tauri bundler notarizes and staples the `.app` but only *signs* the `.dmg`,
so the workflow submits the `.dmg` to `notarytool` and staples it separately.
Without that, mounting a quarantined disk image forces Gatekeeper into an online
check that fails outright when the user is offline.

## Troubleshooting

**`failed to import keychain certificate`** — `APPLE_CERTIFICATE` is not valid
base64 of a `.p12`, or `APPLE_CERTIFICATE_PASSWORD` is wrong. Re-export and
re-encode, and make sure the base64 is pasted as one line.

**`No signing identity found`** — `APPLE_SIGNING_IDENTITY` must match the
certificate's common name exactly, including the parenthesised team ID.

**`Team ID is required`** — `APPLE_TEAM_ID` is missing. Tauri v2 requires it
alongside `APPLE_ID`/`APPLE_PASSWORD`.

**Notarization is rejected** — ask Apple why:

```sh
xcrun notarytool history --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID"
xcrun notarytool log <submission-id> --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID"
```

The usual causes are a missing hardened runtime or a nested binary signed with a
different team ID. Tile bundles no sidecars, so neither should occur.

**Users report "Tile is damaged and can't be opened"** — the signature does not
validate on their machine, usually because the artifact was modified after
signing. Re-download and re-check with `spctl --assess`.

**Accessibility permission stops working after an update** — macOS keys the
grant to the code signature. Consistent signing across releases keeps it stable;
moving from unsigned to signed builds invalidates it once, and the user has to
remove and re-add Tile under **System Settings ▸ Privacy & Security ▸
Accessibility**.
