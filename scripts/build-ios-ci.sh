#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

echo "QXP_SERVER_ORIGIN=${QXP_SERVER_ORIGIN:-}"
echo "QXP_API_BASE_URL=${QXP_API_BASE_URL:-}"
echo "QXP_WS_URL=${QXP_WS_URL:-}"
echo "QXP_CALLS_ENABLED=${QXP_CALLS_ENABLED:-}"
echo "QXP_CALLS_UNAVAILABLE_REASON=${QXP_CALLS_UNAVAILABLE_REASON:-}"

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
console.log("Resolved:", JSON.stringify({
  serverOrigin: String(runtime.serverOrigin || ""),
  apiBaseUrl: String(runtime.apiBaseUrl || ""),
  wsUrl: String(runtime.wsUrl || "")
}, null, 2));
if (!runtime.serverOrigin) throw new Error("runtime-config.js missing serverOrigin");
if (!runtime.apiBaseUrl) throw new Error("runtime-config.js missing apiBaseUrl");
if (!runtime.wsUrl) throw new Error("runtime-config.js missing wsUrl");
NODESCRIPT
node client/dist/validate-runtime-config.cjs
rm -f client/dist/validate-runtime-config.cjs

# ── Rust targets ─────────────────────────────────────────────────

rustup target add aarch64-apple-ios

# ── CocoaPods ───────────────────────────────────────────────────

if ! command -v pod >/dev/null 2>&1; then
  brew install cocoapods
fi

# ── Init iOS project ─────────────────────────────────────────────

bun tauri ios init --skip-targets-install

# ── Build unsigned ───────────────────────────────────────────────
# --no-sign = CODE_SIGN_IDENTITY="" CODE_SIGNING_REQUIRED=NO
# Xcode may refuse iphoneos builds without any provisioning profile
# at all. If so, fall back to simulator.

BUILD_OK=false

if bun tauri ios build --no-sign 2>&1; then
  BUILD_OK=true
  echo "Device build OK (unsigned)."
else
  echo "::warning::Device build failed. Falling back to simulator..."
  if bun tauri ios build --no-sign --target aarch64-apple-ios-sim 2>&1; then
    BUILD_OK=true
    echo "Simulator build OK."
  fi
fi

$BUILD_OK || { echo "::error::iOS build failed."; exit 1; }

# ── Find .app ────────────────────────────────────────────────────

APP_PATH="$(find src-tauri/gen/apple/build -name '*.app' -type d 2>/dev/null | head -n1)"
[[ -d "$APP_PATH" ]] || { echo "::error::No .app found."; exit 1; }
echo "App: $APP_PATH"
codesign -dv "$APP_PATH" 2>/dev/null && echo "Signed" || echo "Unsigned (expected)"

# ── Package IPA ──────────────────────────────────────────────────

IPA_DIR="$RUNNER_TEMP/ipa"
rm -rf "$IPA_DIR"
mkdir -p "$IPA_DIR/Payload"
cp -R "$APP_PATH" "$IPA_DIR/Payload/"

IPA_NAME="QxChat_${APP_VERSION:-${GITHUB_REF_NAME:-ios}}_unsigned.ipa"
IPA_PATH="$IPA_DIR/$IPA_NAME"

cd "$IPA_DIR" && zip -qr "$IPA_NAME" Payload && cd - >/dev/null

APP_ZIP="QxChat_${APP_VERSION:-${GITHUB_REF_NAME:-ios}}_app.zip"
(cd "$(dirname "$APP_PATH")" && zip -qr "$RUNNER_TEMP/$APP_ZIP" "$(basename "$APP_PATH")")

echo "IPA_PATH=$IPA_PATH" >> "$GITHUB_ENV"
echo "APP_ZIP=$RUNNER_TEMP/$APP_ZIP" >> "$GITHUB_ENV"

echo
echo "── unsigned IPA ───────────────────────────────────────"
echo "IPA: $IPA_PATH"
echo
echo "Users must sign with their own Apple ID:"
echo "  AltStore  : https://altstore.io"
echo "  SideStore : https://sidestore.io"
echo "  Sideloadly: https://sideloadly.io"
