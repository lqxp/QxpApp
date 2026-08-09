#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

echo "QXP_RUNTIME_CONFIG_URL=${QXP_RUNTIME_CONFIG_URL:-}"
echo "QXP_SERVER_ORIGIN=${QXP_SERVER_ORIGIN:-}"
echo "QXP_API_BASE_URL=${QXP_API_BASE_URL:-}"
echo "QXP_WS_URL=${QXP_WS_URL:-}"
echo "QXP_CALLS_ENABLED=${QXP_CALLS_ENABLED:-}"
echo "QXP_CALLS_UNAVAILABLE_REASON=${QXP_CALLS_UNAVAILABLE_REASON:-}"
echo "EXPECTED_API_BASE_URL=${EXPECTED_API_BASE_URL:-}"

HAS_APPLE_ID=false
if [[ -n "${APPLE_ID:-}" && -n "${APPLE_ID_PASSWORD:-}" ]]; then
  HAS_APPLE_ID=true
  echo "Apple ID: ${APPLE_ID} (free account — 7-day signing)"
else
  echo "No Apple ID provided — will produce unsigned build (not installable as-is)"
fi

# ── Dependencies ─────────────────────────────────────────────────

bun install --no-save
(cd client && bun install --no-save)
(cd client && node ./scripts/sync-runtime-config.mjs --out dist/runtime-config.js)

# ── Validate runtime config ──────────────────────────────────────

cat > client/dist/validate-runtime-config.cjs <<'NODESCRIPT'
const fs = require("fs");
const vm = require("vm");
const script = fs.readFileSync("client/dist/runtime-config.js", "utf8");
const sandbox = { window: {} };
vm.createContext(sandbox);
vm.runInContext(script, sandbox);
const runtime = sandbox.window.__QXP_RUNTIME__ || {};
const serverOrigin = String(runtime.serverOrigin || "");
const apiBaseUrl = String(runtime.apiBaseUrl || "");
const wsUrl = String(runtime.wsUrl || "");
const expectedApiBaseUrl = process.env.EXPECTED_API_BASE_URL;
console.log("Resolved:", JSON.stringify({ serverOrigin, apiBaseUrl, wsUrl }, null, 2));
if (!serverOrigin) throw new Error("runtime-config.js missing serverOrigin");
if (!apiBaseUrl) throw new Error("runtime-config.js missing apiBaseUrl");
if (!wsUrl) throw new Error("runtime-config.js missing wsUrl");
if (expectedApiBaseUrl && apiBaseUrl !== expectedApiBaseUrl) {
  throw new Error("Unexpected apiBaseUrl: " + apiBaseUrl + " (expected " + expectedApiBaseUrl + ")");
}
NODESCRIPT

node client/dist/validate-runtime-config.cjs
rm -f client/dist/validate-runtime-config.cjs

# ── Rust targets ─────────────────────────────────────────────────

rustup target add aarch64-apple-ios x86_64-apple-ios aarch64-apple-ios-sim

# ── CocoaPods ───────────────────────────────────────────────────

if ! command -v pod >/dev/null 2>&1; then
  brew install cocoapods
fi

# ── Init iOS project ─────────────────────────────────────────────

bun run ios:init

# ── Build ────────────────────────────────────────────────────────

if $HAS_APPLE_ID; then
  # ── Signed build (free Apple ID, 7-day certificate) ──────────
  # Pre-create a keychain so xcodebuild can store the signing identity.

  KEYCHAIN_NAME="qxp-ci.keychain"
  KEYCHAIN_PASSWORD="${APPLE_KEYCHAIN_PASSWORD:-ci-build-$(date +%s)}"
  security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_NAME"
  security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_NAME"
  security set-keychain-settings -lut 21600 "$KEYCHAIN_NAME"
  security list-keychain -d user -s "$KEYCHAIN_NAME" login.keychain

  # xcodebuild with free Apple ID auto-creates a Personal Team
  # provisioning profile via -allowProvisioningUpdates.
  # The app-specific password avoids 2FA issues.

  export APPLE_ID="$APPLE_ID"
  export APPLE_APP_SPECIFIC_PASSWORD="$APPLE_ID_PASSWORD"

  # Tauri passes these through to xcodebuild.
  # DEVELOPMENT_TEAM is auto-resolved from the Apple ID.
  bun tauri ios build \
    --export-method development \
    --allow-provisioning-updates

  security delete-keychain "$KEYCHAIN_NAME" 2>/dev/null || true

  SIGNED=true
else
  # ── Unsigned fallback (simulator-only, or fails for device) ──

  export CODE_SIGN_IDENTITY=""
  export CODE_SIGNING_REQUIRED=NO
  export CODE_SIGNING_ALLOWED=NO

  bun tauri ios build --export-method development 2>&1 || {
    echo "::warning::Device build failed (expected without Apple ID)."
    echo "::warning::Falling back to simulator build..."

    # Build for simulator as last resort — produces a .app for inspection
    bun tauri ios build --target aarch64-apple-ios-sim 2>&1 || {
      echo "::error::iOS build failed entirely."
      exit 1
    }
  }

  SIGNED=false
fi

# ── Find the .app ────────────────────────────────────────────────

APP_PATH="$(find src-tauri/gen/apple/build -name "*.app" -type d 2>/dev/null | head -n1)"

if [[ -z "$APP_PATH" || ! -d "$APP_PATH" ]]; then
  echo "::error::No .app bundle found after build."
  exit 1
fi

echo "App bundle: $APP_PATH"

# Check if actually signed
if $SIGNED; then
  codesign -dv "$APP_PATH" 2>/dev/null && echo "App IS signed" || echo "App is NOT signed (unexpected)"
fi

# ── Package IPA ──────────────────────────────────────────────────

IPA_DIR="$RUNNER_TEMP/ipa"
rm -rf "$IPA_DIR"
mkdir -p "$IPA_DIR/Payload"
cp -R "$APP_PATH" "$IPA_DIR/Payload/"

SUFFIX="unsigned"
if $SIGNED; then SUFFIX="free-signed-7day"; fi

IPA_NAME="QxChat_${APP_VERSION:-${GITHUB_REF_NAME:-ios}}_${SUFFIX}.ipa"
IPA_PATH="$IPA_DIR/$IPA_NAME"

cd "$IPA_DIR"
zip -qr "$IPA_NAME" Payload
cd - >/dev/null

# ── Also create .app.zip ─────────────────────────────────────────

APP_ZIP="QxChat_${APP_VERSION:-${GITHUB_REF_NAME:-ios}}_app.zip"
(cd "$(dirname "$APP_PATH")" && zip -qr "$RUNNER_TEMP/$APP_ZIP" "$(basename "$APP_PATH")")

# ── Output ───────────────────────────────────────────────────────

echo "IPA_PATH=$IPA_PATH" >> "$GITHUB_ENV"
echo "APP_ZIP=$RUNNER_TEMP/$APP_ZIP" >> "$GITHUB_ENV"
echo "IPA_NAME=$IPA_NAME" >> "$GITHUB_ENV"
echo "IOS_SIGNED=$SIGNED" >> "$GITHUB_ENV"
echo "IPA: $IPA_PATH"
echo "App zip: $RUNNER_TEMP/$APP_ZIP"
echo "Signed: $SIGNED"

echo
if $SIGNED; then
  echo "── free-signed IPA (7 days) ──────────────────────────"
  echo "Installable immediately with a free Apple ID."
  echo "Re-sign with AltStore/SideStore/Sideloadly for long-term use."
else
  echo "── unsigned IPA ───────────────────────────────────────"
  echo "NOT installable as-is. Must be signed by the user:"
  echo "  AltStore  : https://altstore.io"
  echo "  SideStore : https://sidestore.io"
  echo "  Sideloadly: https://sideloadly.io"
fi
