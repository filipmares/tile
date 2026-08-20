# Releasing Tile

Releases are cut by pushing a `v*` tag. [`.github/workflows/release.yml`](../.github/workflows/release.yml)
creates a **draft** GitHub Release, builds a universal macOS `.dmg` and a Windows
NSIS setup `.exe`, verifies both, attaches a build provenance attestation, and —
once every platform job is green — publishes the release automatically. A tag
carrying a semver pre-release suffix (`v0.2.0-rc.1`) is published but never
marked `latest`.

Signing is opt-in on both platforms: when the secrets are configured the
artifacts are signed and the workflow asserts it; when they are not, the build
still succeeds and the release notes say the artifacts are unsigned.

- [Cutting a release](#cutting-a-release)
- [One-time macOS signing setup](#one-time-macos-signing-setup)
- [One-time Windows signing setup](#one-time-windows-signing-setup)
- [What the workflow verifies](#what-the-workflow-verifies)
- [Troubleshooting](#troubleshooting)

## Cutting a release

The version lives in three files — `Cargo.toml`, `apps/tile/tauri.conf.json` and
`apps/tile/ui/package.json` — and all three must equal the tag or the workflow
stops before publishing anything.

1. **Bump them together** with the
   [`Bump version`](../.github/workflows/bump.yml) workflow, which edits all
   three, refreshes `Cargo.lock` and opens a PR:

   ```sh
   gh workflow run bump.yml -f version=0.2.0
   ```

2. Review and merge the PR.
3. Tag `main` and push:

   ```sh
   git checkout main && git pull
   git tag v0.2.0
   git push origin v0.2.0
   ```

4. Watch the run: `gh run watch --workflow=release.yml`. The macOS job is the
   slow one — notarization typically takes a few minutes.
5. The release publishes itself when both platform jobs succeed. If one fails,
   the release stays a draft: fix the cause and re-run.

To rebuild an existing tag without re-tagging, run the workflow manually:
`gh workflow run release.yml -f tag=v0.2.0`. Assets are uploaded with
`--clobber`, so a re-run replaces them, and an already-published release stays
published.

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
workflow rejects a partial set up front with a clearer message. The same applies
to `APPLE_CERTIFICATE` without its password.

The workflow only describes a build as signed and notarized once
`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`
and `APPLE_TEAM_ID` are all present; with any of them missing it publishes
unsigned artifacts and the release notes carry the quarantine workaround
instead. `APPLE_SIGNING_IDENTITY` is not part of that check because the bundler
derives the identity from the imported `.p12`, but setting it is still
recommended: it makes the build fail loudly if the certificate is ever replaced
with the wrong kind.

Developer ID certificates expire after five years. Builds notarized before
expiry keep working, but new builds need a fresh certificate.

## One-time Windows signing setup

Windows signing uses **Azure Artifact Signing** (previously Azure Trusted
Signing / Azure Code Signing). It is roughly $10/month at the basic tier, and
unlike a traditional OV certificate it needs no hardware token, so it works
unattended in CI.

Signing does not make SmartScreen warnings disappear immediately: a new
certificate starts with no reputation and earns it as installs accumulate. What
signing buys straight away is a named publisher instead of "Unknown publisher".

> [!IMPORTANT]
> Do not run this on a Visual Studio subscription's monthly Azure credit. Those
> credits are licensed for development and testing only, and signing artifacts
> that are published to the public is production use. Use a pay-as-you-go
> subscription.

### 1. Create the signing account and certificate profile

Follow Microsoft's
[quickstart](https://learn.microsoft.com/en-us/azure/trusted-signing/quickstart):

1. Register the `Microsoft.CodeSigning` resource provider on the subscription.
2. Create an **Artifact Signing account**. Note its **region endpoint** — it
   looks like `https://wus2.codesigning.azure.net`.
3. Complete **identity validation**. Both organization and individual identities
   are supported; organizations validate faster, individuals need a government
   ID. This is the step with a human in the loop, so start it early.
4. Create a **certificate profile** of type **Public Trust**. Its name is the
   `-c` argument the signing CLI takes.

### 2. Create a service principal for GitHub Actions

In the Azure portal, **Microsoft Entra ID ▸ App registrations ▸ New
registration**. From the app's overview page take the **Application (client) ID**
and **Directory (tenant) ID**, then create a client secret under **Certificates &
secrets ▸ New client secret**.

Back on the Artifact Signing account, open **Access control (IAM)** and assign
the app two roles:

- **Trusted Signing Certificate Profile Signer** (named *Artifact Signing
  Certificate Profile Signer* in newer portal builds) — scoped to the
  certificate profile.
- **Reader** — scoped to the signing account.

> The signing CLI that Tauri documents authenticates with a client secret, not
> OIDC, which is why a secret is created here rather than a federated
> credential. Rotate it on the schedule the portal suggests. If the project later
> moves to a signing tool built on `Azure.Identity` — for example Microsoft's
> `sign` CLI driving the Trusted Signing dlib — the client secret can be replaced
> with a federated credential and `azure/login`.

### 3. Add the repository secrets

| Secret | Value |
| ------ | ----- |
| `AZURE_CLIENT_ID` | Application (client) ID of the app registration |
| `AZURE_CLIENT_SECRET` | Client secret value from step 2 |
| `AZURE_TENANT_ID` | Directory (tenant) ID |
| `AZURE_ARTIFACT_SIGNING_ENDPOINT` | Region endpoint, e.g. `https://wus2.codesigning.azure.net` |
| `AZURE_ARTIFACT_SIGNING_ACCOUNT` | Artifact Signing account name |
| `AZURE_ARTIFACT_SIGNING_CERTIFICATE_PROFILE` | Certificate profile name |

All six are required together. With any of them missing the workflow builds an
unsigned installer, says so in the release notes, and skips the signature
assertions — it never silently ships an unsigned build while claiming otherwise.

Signing is wired through Tauri's `bundle > windows > signCommand`, which the
workflow writes into a generated `apps/tile/tauri.signing.conf.json` overlay and
passes with `--config`. That indirection is deliberate: a `signCommand` committed
to `tauri.conf.json` would make every local `tauri build` fail on a missing tool.
Going through `signCommand` rather than signing the finished `.exe` afterwards
also means the `tile.exe` *inside* the installer is signed, which is where the
publisher name in the SmartScreen and Defender prompts comes from.

## What the workflow verifies

Silent signing failures are the main hazard: an unsigned build looks identical
to a signed one until a user downloads it. Because the release now publishes
itself, both platform jobs assert their end state rather than trusting the
bundler, and a failed assertion leaves the release as a draft.

**macOS:**

- `codesign --verify --deep --strict` on the `.app` and the `.dmg`
- the hardened runtime flag is present (notarization requires it)
- `spctl --assess` reports `accepted` / `source=Notarized Developer ID`
- `xcrun stapler validate` passes on both the `.app` and the `.dmg`

The Tauri bundler notarizes and staples the `.app` but only *signs* the `.dmg`,
so the workflow submits the `.dmg` to `notarytool` and staples it separately.
Without that, mounting a quarantined disk image forces Gatekeeper into an online
check that fails outright when the user is offline.

**Windows** — the job runs the installer the way a user would:

- silent install (`/S`) exits cleanly
- an Add/Remove Programs entry named `Tile` exists, carries an
  `InstallLocation` that exists on disk, and its `DisplayVersion` matches the
  tag
- launching `tile.exe` leaves it running after 15 seconds — Tile is tray-only
  and never exits on its own, so an early exit means a broken bundle or a
  missing WebView2 runtime
- silent uninstall exits cleanly

Two further assertions run **only when Windows signing is configured**, since
there is nothing to assert otherwise:

- the installer's Authenticode signature is `Valid` and timestamped
- the installed `tile.exe` is itself signed — catching the case where the
  installer was signed but the binary inside it was not

**Both** — every uploaded artifact gets a
[build provenance attestation](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations),
which anyone can check:

```sh
gh attestation verify Tile_0.2.0_x64-setup.exe --repo filipmares/tile
```

### Verifying the download by hand

Worth doing at least once per platform, on a machine that has never run Tile.

macOS:

```sh
# Simulate a downloaded file — this is the attribute Gatekeeper reacts to.
xattr -w com.apple.quarantine "0081;00000000;Safari;" Tile_0.2.0_universal.dmg
open Tile_0.2.0_universal.dmg
```

It should mount and the app should launch with no security dialog beyond the
standard "downloaded from the internet" confirmation and the Accessibility
permission prompt.

Windows:

```powershell
Get-AuthenticodeSignature .\Tile_0.2.0_x64-setup.exe | Format-List Status, SignerCertificate
```

Then run the installer from Explorer and confirm the publisher name in the
SmartScreen dialog is the identity from your certificate profile, not "Unknown
publisher".

## Troubleshooting

### macOS

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

### Windows

**A 403 or "certificate profile not found" from the signing CLI** — the service
principal is missing the *Certificate Profile Signer* role, or it was assigned at
the wrong scope. It must be granted on the certificate profile itself, not only
on the resource group.

**`AADSTS7000215: Invalid client secret provided`** — `AZURE_CLIENT_SECRET` holds
the secret's *ID* rather than its *value*, or the secret has expired. The value is
shown only once, immediately after creation.

**The installer is signed but `tile.exe` inside it is not** — the build ran
without the generated `tauri.signing.conf.json` overlay, so Tauri never invoked
`signCommand`. Check that all six Azure secrets are present; the workflow skips
the whole signing path if any is missing.

**The launch smoke test fails with an immediate exit** — most often a WebView2
problem. The installer uses `downloadBootstrapper`, so the runtime is fetched at
install time; a machine without outbound access to Microsoft's CDN installs Tile
successfully and then fails to start it.

**SmartScreen still warns on a signed build** — expected for a new certificate.
Reputation accrues with download volume. It cannot be bought, only waited out; an
EV certificate is the only way to skip the wait, and Azure Artifact Signing does
not issue those.
