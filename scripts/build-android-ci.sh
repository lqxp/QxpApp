#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ -z "${QXP_RUNTIME_CONFIG_URL:-}" && -z "${QXP_SERVER_ORIGIN:-}" && -z "${QXP_API_BASE_URL:-}" ]]; then
  echo "::error::No runtime configuration provided for Android build."
  exit 1
fi

echo "QXP_RUNTIME_CONFIG_URL=${QXP_RUNTIME_CONFIG_URL:-}"
echo "QXP_SERVER_ORIGIN=${QXP_SERVER_ORIGIN:-}"
echo "QXP_API_BASE_URL=${QXP_API_BASE_URL:-}"
echo "QXP_WS_URL=${QXP_WS_URL:-}"
echo "QXP_TURN_URLS=${QXP_TURN_URLS:-}"
echo "QXP_TURN_USERNAME=${QXP_TURN_USERNAME:+***set***}"
echo "QXP_TURN_CREDENTIAL=${QXP_TURN_CREDENTIAL:+***set***}"
echo "QXP_RELAY_ONLY=${QXP_RELAY_ONLY:-}"
echo "QXP_CALLS_ENABLED=${QXP_CALLS_ENABLED:-}"
echo "QXP_CALLS_UNAVAILABLE_REASON=${QXP_CALLS_UNAVAILABLE_REASON:-}"
echo "EXPECTED_API_BASE_URL=${EXPECTED_API_BASE_URL:-}"

bun install --no-save
(cd client && bun install --no-save)
(cd client && node ./scripts/sync-runtime-config.mjs --out dist/runtime-config.js)

echo "----- client/dist/runtime-config.js -----"
cat client/dist/runtime-config.js
echo "----------------------------------------"

cat > client/dist/validate-runtime-config.cjs <<'EOF'
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

console.log("Resolved runtime config:");
console.log(JSON.stringify({ serverOrigin, apiBaseUrl, wsUrl }, null, 2));

if (!serverOrigin) throw new Error("runtime-config.js missing serverOrigin");
if (!apiBaseUrl) throw new Error("runtime-config.js missing apiBaseUrl");
if (!wsUrl) throw new Error("runtime-config.js missing wsUrl");
if (expectedApiBaseUrl && apiBaseUrl !== expectedApiBaseUrl) {
  throw new Error("Unexpected apiBaseUrl: " + apiBaseUrl + " (expected " + expectedApiBaseUrl + ")");
}
EOF

node client/dist/validate-runtime-config.cjs
rm -f client/dist/validate-runtime-config.cjs

mkdir -p src-tauri/gen/android/app/src/main/res
cp -R src-tauri/icons/android/. src-tauri/gen/android/app/src/main/res/

# ── Gradle Maven mirrors (workaround for CI 403 from Maven Central) ─
mkdir -p "$HOME/.gradle/init.d"
cat > "$HOME/.gradle/init.d/ci-mirrors.gradle" <<'GRADLE'
beforeSettings { settings ->
  settings.pluginManagement.repositories {
    maven { url "https://dl.google.com/dl/android/maven2/" }
    maven { url "https://repo1.maven.org/maven2/" }
    google()
    mavenCentral()
    gradlePluginPortal()
  }
}
allprojects {
  buildscript {
    repositories {
      maven { url "https://dl.google.com/dl/android/maven2/" }
      maven { url "https://repo1.maven.org/maven2/" }
      google()
      mavenCentral()
    }
  }
  repositories {
    maven { url "https://dl.google.com/dl/android/maven2/" }
    maven { url "https://repo1.maven.org/maven2/" }
    google()
    mavenCentral()
  }
}
GRADLE

bun tauri android build --apk --target aarch64

apk="$(find src-tauri/gen/android/app/build/outputs/apk -path '*/release/*.apk' -type f | sort | head -n1)"
if [[ -z "$apk" ]]; then
  echo "::error::No release APK produced."
  exit 1
fi

apksigner="$(find -L "$ANDROID_HOME/build-tools" -type f -name apksigner | sort -V | tail -n1)"
zipalign="$(find -L "$ANDROID_HOME/build-tools" -type f -name zipalign | sort -V | tail -n1)"
"$apksigner" verify --verbose --print-certs "$apk"
"$zipalign" -c -P 16 -v 4 "$apk"

release_output_dir="src-tauri/gen/android/app/build/outputs/apk/release"
release_apk="$release_output_dir/QxChat_${APP_VERSION:-${GITHUB_REF_NAME:-android}}_aarch64.apk"
mkdir -p "$release_output_dir"
cp "$apk" "$release_apk"
echo "Android APK release asset: $release_apk"
