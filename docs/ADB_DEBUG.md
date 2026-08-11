# ADB Debug — QxChat Android

Guide for building, installing, and debugging the QxChat APK on an Android device via ADB.

## Prerequisites

```sh
# Enter the Nix shell (provides Android SDK, NDK, platform-tools, adb)
nix develop

# Verify adb is available
adb version
```

The Android device must be connected via USB with **USB debugging** enabled (Developer Options).

```sh
adb devices
# Should show a device as "unauthorized" → accept the popup on the phone
# Then:
adb devices
# Should show "device"
```

## Build

### Signed release build (recommended)

```sh
bun run build:android
```

The `scripts/build-android.sh` script:
1. Auto-detects the keystore (`lqxp-release.jks` at project root or `~/.config/qxchat/qxchat-release.jks`)
2. Prompts for the password if not set via env or `.env`
3. Compiles in release mode for `aarch64`
4. Outputs the APK to `src-tauri/gen/android/app/build/outputs/apk/release/`

### Creating a new keystore

If you lost the keystore password:

```sh
LQXP_ANDROID_CREATE_KEYSTORE=1 ANDROID_KEYSTORE_PASSWORD='YourPassword' bun run build:android
```

Store the password in a `.env` file to avoid typing it every time:

```env
ANDROID_KEYSTORE_PASSWORD=YourPassword
ANDROID_KEY_ALIAS=lqxp
```

## Installation

```sh
# Uninstall the old version (required if signature changed)
adb uninstall com.qxp.client

# Install the new APK
adb install src-tauri/gen/android/app/build/outputs/apk/release/app-release.apk

# Or install over existing (same signature only)
adb install -r src-tauri/gen/android/app/build/outputs/apk/release/app-release.apk
```

## Debugging with logcat

```sh
# Filter QxChat app logs
adb logcat -v time | grep -i qxp

# Filter Tauri / WebView logs
adb logcat -v time | grep -iE "tauri|webview|chromium|qxchat"

# Clear buffer before launching the app
adb logcat -c && adb logcat -v time | grep -i qxp

# Show crashes / errors only
adb logcat -v time *:E | grep -i qxp

# JS console logs (console.log from the WebView)
adb logcat -v time chromium:V *:S
```

## Inspecting the WebView (DevTools)

```sh
# List debuggable WebViews
adb shell cat /proc/net/unix | grep devtools

# Open chrome://inspect in Chromium/Chrome on the PC
# → The device and its WebView will appear
# → Click "Inspect"
```

## Useful commands

```sh
# Package name
adb shell pm list packages | grep qxp

# Installed version
adb shell dumpsys package com.qxp.client | grep versionName

# Clear app data (full reset)
adb shell pm clear com.qxp.client

# Force stop
adb shell am force-stop com.qxp.client

# Take a screenshot
adb exec-out screencap -p > screenshot.png

# Record screen (Ctrl+C to stop)
adb shell screenrecord /sdcard/demo.mp4
# Then pull:
adb pull /sdcard/demo.mp4 .

# View native crashes (tombstones)
adb shell ls /data/tombstones/
adb pull /data/tombstones/ .

# Battery / network info
adb shell dumpsys battery
adb shell dumpsys connectivity

# Simulate a notification
adb shell cmd notification post -S bigtext -t "QxChat Test" "Tag" "Test message"
```

## Signing & keystore

```sh
# Verify APK signature
apksigner verify --verbose --print-certs app-release.apk

# Display keystore certificate
keytool -list -v -keystore lqxp-release.jks -storepass YourPassword -alias lqxp

# Zipalign check
zipalign -c -P 16 -v 4 app-release.apk
```

## Troubleshooting

### `INSTALL_FAILED_UPDATE_INCOMPATIBLE`
The APK signature changed → run `adb uninstall com.qxp.client` first.

### `Keystore was tampered with, or password was incorrect`
The password doesn't match the keystore. Create a new one:
```sh
LQXP_ANDROID_CREATE_KEYSTORE=1 ANDROID_KEYSTORE_PASSWORD='NewPassword' bun run build:android
```

### `adb: no devices/emulators found`
- Check USB connection and debugging mode
- `adb kill-server && adb start-server`
- On some devices, switch to PTP (photo transfer) mode instead of MTP

### `ANDROID_SDK_ROOT/ANDROID_HOME is not set`
Run the build from the Nix shell: `nix develop` then `bun run build:android`.

### WebView not loading / blank screen
```sh
adb logcat -v time chromium:V *:S
```
Look for `ERR_CONNECTION_REFUSED` or `ERR_NAME_NOT_RESOLVED` errors.
