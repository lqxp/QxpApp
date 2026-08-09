# Tauri iOS build

## Reality check

**Xcode cannot build for a physical iOS device without at least a free Apple ID.** The `Personal Team` (free account) is the minimum Xcode requires to create a provisioning profile, even for development builds. `CODE_SIGN_IDENTITY=""` alone is not enough for device builds.

This CI supports two modes:

| Mode | CI secrets needed | Result | Installable? | Duration |
|---|---|---|---|---|
| **Free Apple ID** | `APPLE_ID` + `APPLE_ID_PASSWORD` | Signed IPA | Yes, immediately | 7 days |
| **No Apple ID** | None | Simulator .app | No | N/A |

**With a free Apple ID** (recommended): the CI produces a **7-day signed IPA** that users can install immediately. For long-term use, they re-sign with their own Apple ID via AltStore/SideStore/Sideloadly.

## Setting up the free Apple ID for CI

1. Go to https://appleid.apple.com
2. Create a new Apple ID (e.g. `qxp-ci@icloud.com`) — free, no payment needed
3. In Security → App-Specific Passwords, generate one (name: "GitHub CI")
4. In GitHub repo → Settings → Secrets and variables → Actions, add:
   - `APPLE_ID` = your CI Apple ID email
   - `APPLE_ID_PASSWORD` = the app-specific password
5. That's it. Xcode auto-creates a Personal Team + 7-day dev certificate on first build.

## Architecture

```
Git push tag vX.Y.Z / workflow_dispatch
        │
        ▼
GitHub Actions (macos-latest)
        │
        ├─ Rust stable + aarch64-apple-ios target
        ├─ Bun + Tauri CLI
        ├─ sync-runtime-config.mjs → dist/runtime-config.js
        ├─ tauri ios init + pod install
        ├─ IF APPLE_ID set: xcodebuild with -allowProvisioningUpdates
        │   → Personal Team auto-created → signed .app (7 days)
        ├─ IF no Apple ID: fallback iphonesimulator build
        │   → unsigned .app (simulator only)
        ├─ Package → .ipa (zip Payload/)
        └─ Upload artifact + attach to GitHub Release
                │
                ▼
        User downloads IPA → installs (if signed, 7 days)
        User re-signs with own Apple ID for long-term use
```

## Local build

iOS builds require macOS with the full Xcode app installed.

```bash
nix develop
bun run ios:build --export-method development
```

Without Nix:

```bash
./scripts/ios-build.sh --export-method development
```

### Build with free Apple ID

```bash
export APPLE_ID="your@email.com"
export APPLE_APP_SPECIFIC_PASSWORD="xxxx-xxxx-xxxx-xxxx"
bun run ios:build --export-method development --allow-provisioning-updates
```

### Build unsigned (simulator only)

```bash
export CODE_SIGN_IDENTITY=""
export CODE_SIGNING_REQUIRED=NO
export CODE_SIGNING_ALLOWED=NO
bun tauri ios build --target aarch64-apple-ios-sim
```

## CI/CD (GitHub Actions)

### `.github/workflows/ios.yml`

Triggers:
- `workflow_dispatch` — manual trigger from the Actions tab
- `push tags v*.*.*` — auto-build on release tags

Produces:
- `qxchat-ios` artifact with `.ipa` (signed or simulator) + `.app.zip`
- If a tag push, attaches artifacts to the GitHub Release

No secrets strictly required — but without `APPLE_ID`, the build targets simulator only.

### `.github/workflows/build-and-release.yml` (commented-out signed job)

The commented-out `build-ios` job in the main release workflow requires full Apple Developer secrets:
- `APPLE_DEVELOPMENT_TEAM`
- `APPLE_CERTIFICATE_P12_BASE64`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_PROVISIONING_PROFILE_BASE64`

These are only needed for **paid** Apple Developer distribution. The standalone `ios.yml` uses only the free Apple ID.

## User installation (sideloading)

### With free-signed IPA from CI (7 days)

1. Download the IPA from GitHub Releases
2. Install via AltStore / SideStore / Sideloadly with **any** Apple ID
3. App runs for 7 days
4. Re-install updated IPA to refresh

### Re-signing for long-term use

If the IPA is unsigned (no Apple ID in CI), or the 7 days expired:

1. Install [AltStore](https://altstore.io) on iPhone (requires AltServer on a computer)
2. Install [SideStore](https://sidestore.io) — no computer needed after setup
3. Or use [Sideloadly](https://sideloadly.io) on your computer with USB

All three sign the IPA with **your own** free Apple ID.

## Limitations

| Limit | Free Apple ID | Paid Developer |
|---|---|---|
| App lifetime | 7 days (renewable) | 1 year |
| Max apps per device | 3 | Unlimited |
| App Store distribution | No | Yes |
| TestFlight | No | Yes |
| Push notifications | No | Yes |
| App Groups / Extensions | Limited | Full |

### Important notes

- The 7-day limit is per-signature. AltStore/SideStore auto-refresh before expiry.
- A free Apple ID can sign 10 app IDs per 7 days.
- The app identifier (`com.qxp.client`) must match what the user signs with.
- The CI's Apple ID is only used for the build — users re-sign with their own ID.
- If the CI's certificate expires, just re-run the build (Xcode auto-renews).

## Verification

```bash
# Check .app exists
ls -la src-tauri/gen/apple/build/arm64/*.app

# Check signing status
codesign -dv src-tauri/gen/apple/build/arm64/*.app 2>&1

# Check IPA structure
unzip -l QxChat_*_*.ipa | head -20
# Should show Payload/QxChat.app/...

# Verify with xcodebuild
cd src-tauri/gen/apple
xcodebuild \
  -project QxChat.xcodeproj \
  -scheme QxChat \
  -configuration Release \
  -sdk iphoneos \
  -destination 'generic/platform=iOS' \
  -allowProvisioningUpdates \
  build
```
