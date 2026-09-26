# Releasing

Releases are built on a Mac with Xcode. The engine, the application and the
update archives all come from one clean checkout of the tag being released.

## Before the build

A version bump touches four places, and CI fails if they disagree:

- `core/Cargo.toml`, the `version` field
- `core/Cargo.lock`, the `squint-core` entry (the `--locked` gate reads it)
- `app/project.yml`, `MARKETING_VERSION`
- `app/project.yml`, `CURRENT_PROJECT_VERSION` (an integer, one higher each time)

## Build

```
git clone https://github.com/mdws-org/squint.git && cd squint && git checkout <tag>
cd app
xcodegen
xcodebuild -project Squint.xcodeproj -scheme Squint -configuration Release -derivedDataPath build
sh ../tools/sign-release.sh build/Build/Products/Release/Squint.app \
    "Developer ID Application: Benjamin Meadows" squint-notary \
    ~/Library/Keychains/mdws-signing.keychain-db
```

Do not send `xcodegen`'s output to `/dev/null`. It has failed on invalid YAML
while `xcodebuild` went on to succeed against the previous `Info.plist`, so the
build reported success and shipped the old menu titles. Any string containing a
colon must be quoted.

`sign-release.sh` signs Sparkle's five nested pieces inside out and the application last, each with the hardened runtime and a timestamp, verifies every seal, submits the bundle to Apple's notary service and waits, staples the ticket, and finishes with `spctl`, which must answer `accepted` with `source=Notarized Developer ID`. Nothing after this step may modify the bundle: the archives are built from the stapled bundle, and a zip made before stapling ships without the ticket.

Do not sign with `--deep`. It is discouraged by Apple and refused by notarization for an application that embeds a framework, which is why the script names each piece.

`SIGN_ONLY=1` in the environment stops the script after verification. A new certificate, a new build machine, or a keychain that might prompt can all be proven that way without spending a notarization.

Then the two archives, from the same stapled bundle. The zip is what Sparkle downloads; it cannot be stapled itself, and the ticket travels inside it on the bundle. The dmg is what a person downloads, so it is signed, notarized and stapled in its own right, and its hash is taken only after stapling, which changes the file:

```
KC=~/Library/Keychains/mdws-signing.keychain-db
ditto -c -k --keepParent build/Build/Products/Release/Squint.app Squint-<version>.zip
mkdir dmgroot && cp -R build/Build/Products/Release/Squint.app dmgroot/
ln -s /Applications dmgroot/Applications
hdiutil create -volname "Squint <version>" -srcfolder dmgroot -format UDZO Squint-<version>.dmg
codesign --force --timestamp --keychain "$KC" --sign "Developer ID Application: Benjamin Meadows" Squint-<version>.dmg
xcrun notarytool submit Squint-<version>.dmg --keychain-profile squint-notary --keychain "$KC" --wait
xcrun stapler staple Squint-<version>.dmg
spctl -a -vv -t open --context context:primary-signature Squint-<version>.dmg
shasum -a 256 Squint-<version>.dmg Squint-<version>.zip
```

`spctl` must answer `accepted` with `source=Notarized Developer ID` for the dmg as well. An unsigned dmg is accepted by notarization and staples without complaint, and still answers `rejected` with `no usable signature`: notarization looks at the application inside, `spctl` at the container.

## Sign the update and write the appcast

Sparkle will not install an archive it cannot verify. The private key is not on
any build machine; fetch it for this one command and remove it afterwards.

```
export OP_SERVICE_ACCOUNT_TOKEN="$(cat ~/.config/architect/secrets/op-sa-fleet-token)"
umask 077
op document get "squint Sparkle EdDSA signing key" --vault Fleet --out-file /tmp/sparkle.key
mkdir -p releases && cp Squint-<version>.zip releases/
<sparkle-tools>/bin/generate_appcast --ed-key-file /tmp/sparkle.key \
    --download-url-prefix https://github.com/mdws-org/squint/releases/download/v<version>/ \
    releases/
rm -P /tmp/sparkle.key
```

`generate_appcast` writes `releases/appcast.xml`. Publish the release first,
then copy that file to the repository root and push it on `main`. An entry
naming a download that does not exist yet is worse than no entry, and the feed
URL in the application reads the file from `main`, so the appcast is the last
thing to move.

`generate_appcast` writes the whole file, including a `sparkle:hardwareRequirements`
line taken from the binary it signed. A build made on an Apple silicon Mac says
`arm64`, and an Intel Mac is then never offered that update. That is correct for
what is being shipped; it becomes wrong the day a release is built universal,
and the appcast is where it will show.

The Sparkle tools are in the release tarball at
`https://github.com/sparkle-project/Sparkle/releases`, matching the version
pinned in `app/project.yml`. The 2.9.6 tarball is sha256
`52bf9e88cdd972fc0c81501377a880e90d47031bd8ca5462488f843e2609e192`.

## Checking what the Sparkle pin resolved to

The project file is generated, so `Package.resolved` is not in the repository
and `xcodebuild` writes a fresh one on every build. Read it after a build and
compare it against the revision recorded here; a version tag can be moved, a
revision cannot.

```
cat app/Squint.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved
```

2.9.6 resolves to revision `ac2def288cbff5cfc7df3ffef6abdf45b72bcb0a`.

## Publish

```
gh release create v<version> --target <full 40-character SHA> \
    --title "Squint <version>" --notes-file notes.md \
    Squint-<version>.dmg Squint-<version>.zip
```

A short SHA is rejected as an invalid target. Releases before 0.7.1 were
pre-releases because they were unsigned and macOS blocked the first launch.
0.7.1, the first signed build, was held as a pre-release until an installed
earlier build had taken it through Check for Updates, because it was the first
update to cross from an ad-hoc signature to a Developer ID one; Sparkle allows
that when the EdDSA key is unchanged, and it did. `--prerelease` is now for a
release that has a specific reason to be held, not the default.

## One-time signing setup on the build machine

The Developer ID Application certificate and its private key live in `~/Library/Keychains/mdws-signing.keychain-db` on svk, not in the login keychain. The private key was generated there and is escrowed in 1Password as `squint Developer ID private key` in the Fleet vault; the certificate is issued by Apple against the CSR from that key and can be reissued from the portal if lost, but the key cannot.

Installing the pair, once:

```
KC=~/Library/Keychains/mdws-signing.keychain-db
security import ~/squint-signing/developerid.key -k "$KC" -T /usr/bin/codesign -T /usr/bin/security
security import ~/squint-signing/developerid.cer -k "$KC" -T /usr/bin/codesign
curl -O https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer
security import DeveloperIDG2CA.cer -k "$KC"
security find-identity -v -p codesigning "$KC"
```

The identity must appear as valid. `find-identity` reports no valid identity at all, with the key and certificate both imported and matching, until Apple's `Developer ID Certification Authority` (G2) intermediate is in the same keychain; Xcode does not put it anywhere that `security` searches. `security verify-cert -c developerid.cer -k "$KC" -p codeSign` confirms the chain.

`codesign` in a session with no window server cannot answer a keychain prompt, and fails as `errSecInternalComponent`. The keychain is unlocked and its key's partition list is set once so that no prompt is raised. The password is read from 1Password into the environment and never appears on a command line:

```
# The item is 'MDWS Dev Signing — self-signed code-sign cert (svk+btm)'; it is
# addressed by ID so a renamed title cannot break a release, and the password is
# its attached file, not a field.
KEYCHAIN_PW="$(op read 'op://Fleet/rimse6i35takecjdcilqb6j7xa/keychain-pass')"
security unlock-keychain -p "$KEYCHAIN_PW" "$KC"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PW" "$KC"
```

Unlocking is needed again in each new session; the partition list persists.

## Notary credentials

Notarization authenticates with an App Store Connect API key: an Issuer ID, a Key ID, and a `.p8` file that App Store Connect lets you download exactly once. The key is a Team key, generated at App Store Connect under Users and Access, Integrations, App Store Connect API, with the Developer role, which is all notarization needs; the first visit asks for access to the API and acceptance of its terms before the Team Keys tab exists. All three values live in the Fleet vault as `squint App Store Connect API key`. They are stored into the signing keychain under the profile name the script expects, and `store-credentials` checks them against Apple before it saves anything:

```
xcrun notarytool store-credentials squint-notary \
    --key ~/squint-signing/AuthKey_<KEYID>.p8 --key-id <KEYID> --issuer <ISSUER> \
    --keychain ~/Library/Keychains/mdws-signing.keychain-db
```

`xcrun notarytool history --keychain-profile squint-notary --keychain "$KC"` confirms the profile works before a release depends on it.

## Keys

The EdDSA key that signs updates lives in 1Password, in the Fleet vault, as
`squint Sparkle EdDSA signing key`. Its public half is `SUPublicEDKey` in
`app/project.yml`, compiled into every build. Losing the private key means no
existing installation can be updated again: every user would have to download a
new build by hand. It is worth more than any single release.
