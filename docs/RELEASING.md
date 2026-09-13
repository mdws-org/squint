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
codesign -f -s - --deep build/Build/Products/Release/Squint.app
```

Do not send `xcodegen`'s output to `/dev/null`. It has failed on invalid YAML
while `xcodebuild` went on to succeed against the previous `Info.plist`, so the
build reported success and shipped the old menu titles. Any string containing a
colon must be quoted.

Then the two archives, from the same signed bundle:

```
ditto -c -k --keepParent build/Build/Products/Release/Squint.app Squint-<version>.zip
mkdir dmgroot && cp -R build/Build/Products/Release/Squint.app dmgroot/
ln -s /Applications dmgroot/Applications
hdiutil create -volname "Squint <version>" -srcfolder dmgroot -format UDZO Squint-<version>.dmg
```

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
gh release create v<version> --prerelease --target <full 40-character SHA> \
    --title "Squint <version>" --notes-file notes.md \
    Squint-<version>.dmg Squint-<version>.zip
```

A short SHA is rejected as an invalid target. Releases stay pre-releases while
the application is unsigned by a paid developer account: macOS blocks the first
launch and the notes carry the walkthrough.

## Keys

The EdDSA key that signs updates lives in 1Password, in the Fleet vault, as
`squint Sparkle EdDSA signing key`. Its public half is `SUPublicEDKey` in
`app/project.yml`, compiled into every build. Losing the private key means no
existing installation can be updated again: every user would have to download a
new build by hand. It is worth more than any single release.
