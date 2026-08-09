# Tauri iOS build

## Reality check

**Xcode cannot build for a physical iOS device without at least a free Apple ID.** The `Personal Team` (free account) is the minimum Xcode requires to create a provisioning profile, even for development builds.

This CI uses Tauri's `--no-sign` flag to skip code signing entirely.
- If Xcode allows device builds without signing → unsigned IPA for `iphoneos` (ideal)
- If Xcode refuses → falls back to `iphonesimulator` build (unsigned, simulator only)

The resulting IPA is **unsigned** — users must sign it with their own Apple ID via AltStore / SideStore / Sideloadly.

**Zero Apple secrets required.** No Apple ID, no Developer account, no certificates, no provisioning profiles.

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
        ├─ tauri ios build --no-sign
        │   ├─ Success → unsigned .app for iphoneos
        │   └─ Failure → fallback iphonesimulator
        ├─ Package → .ipa (zip Payload/) + .app.zip
        └─ Upload artifact + attach to GitHub Release
                │
                ▼
        User downloads IPA
        User signs with own Apple ID via AltStore/SideStore/Sideloadly
```

## Local build

iOS builds require macOS with the full Xcode app installed.

```bash
# Signed (requires Apple ID in Xcode)
bun run ios:build --export-method development

# Unsigned
bun tauri ios build --no-sign
```

## CI/CD (GitHub Actions)

### `.github/workflows/ios.yml`

Triggers:
- `workflow_dispatch` — manual trigger from the Actions tab
- `push tags v*.*.*` — auto-build on release tags

Produces:
- `qxchat-ios-unsigned` artifact with `.ipa` + `.app.zip`
- If a tag push, attaches artifacts to the GitHub Release

**Zero secrets needed** for the iOS build itself. The runtime config uses `QXP_SERVER_ORIGIN` etc. if set.

### `.github/workflows/build-and-release.yml` (commented-out signed job)

The commented-out `build-ios` job in the main release workflow requires full Apple Developer secrets for a **signed** IPA (App Store distribution).

## User installation (sideloading)

1. Download the unsigned IPA from GitHub Releases
2. Sign it with **your own free Apple ID** using:
   - [AltStore](https://altstore.io) — requires AltServer on a computer, auto-refreshes
   - [SideStore](https://sidestore.io) — no computer needed after setup
   - [Sideloadly](https://sideloadly.io) — USB install from computer
3. App runs for 7 days, renewable

## Limitations

| Limit | Free Apple ID | Paid Developer |
|---|---|---|
| App lifetime | 7 days (renewable) | 1 year |
| Max apps per device | 3 | Unlimited |
| App Store distribution | No | Yes |
| TestFlight | No | Yes |
| Push notifications | No | Yes |

### Important notes

- The 7-day limit is per-signature. AltStore/SideStore auto-refresh before expiry.
- A free Apple ID can sign 10 app IDs per 7 days.
- The app identifier (`com.qxp.client`) must match what the user signs with.
- If the CI produces a simulator-only build, users must build locally for their device.

## Verification

```bash
# Check .app exists
ls -la src-tauri/gen/apple/build/arm64/*.app

# Check signing status (should be unsigned)
codesign -dv src-tauri/gen/apple/build/arm64/*.app 2>&1

# Check IPA structure
unzip -l QxChat_*_unsigned.ipa | head -20
```
