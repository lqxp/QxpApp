#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

PACKAGE="com.qxp.client"
APK_DIR="src-tauri/gen/android/app/build/outputs/apk"
DEV_PORT="${QXP_DEV_PORT:-4560}"

DO_LOG=false
for arg in "$@"; do
  case "$arg" in
    --log|-l) DO_LOG=true ;;
  esac
done

# ── 1. Find the APK ──────────────────────────────────────────────

APK=""
if [[ -d "$APK_DIR" ]]; then
  # Prefer universal release, then release, then any
  APK="$(find "$APK_DIR" -path "*/universal/release/*.apk" -type f 2>/dev/null | sort | head -n1)"
  if [[ -z "$APK" ]]; then
    APK="$(find "$APK_DIR" -path "*/release/*.apk" -type f 2>/dev/null | sort | head -n1)"
  fi
  if [[ -z "$APK" ]]; then
    APK="$(find "$APK_DIR" -name "*.apk" -type f 2>/dev/null | sort | head -n1)"
  fi
fi

if [[ -z "$APK" || ! -f "$APK" ]]; then
  echo -e "${RED}No APK found in $APK_DIR${NC}"
  echo "Build first: bun run build:android"
  exit 1
fi

echo -e "${GREEN}APK: $APK${NC}"

# ── 2. Verify ADB is available ───────────────────────────────────

if ! command -v adb >/dev/null 2>&1; then
  echo -e "${RED}adb not found. Run: nix develop${NC}"
  exit 1
fi

# ── 3. Kill & start ADB server ───────────────────────────────────

echo "Starting ADB server..."
adb kill-server 2>/dev/null || true
adb start-server 2>/dev/null || true
sleep 1

# ── 4. Wait for device ───────────────────────────────────────────

echo "Waiting for device..."
DEVICE=""
for i in $(seq 1 30); do
  DEVICE="$(adb devices 2>/dev/null | grep -v 'List of devices' | grep -v '^$' | grep 'device$' | head -n1 | awk '{print $1}')"
  if [[ -n "$DEVICE" ]]; then
    break
  fi
  printf "."
  sleep 1
done
echo

if [[ -z "$DEVICE" ]]; then
  echo -e "${RED}No device found after 30s.${NC}"
  echo "  - USB connected + debugging enabled?"
  echo "  - Accepted the RSA fingerprint popup on the phone?"
  exit 1
fi

echo -e "${GREEN}Device: $DEVICE${NC}"

# ── 5. Uninstall old version ─────────────────────────────────────

INSTALLED="$(adb -s "$DEVICE" shell pm list packages "$PACKAGE" 2>/dev/null || true)"
if [[ -n "$INSTALLED" ]]; then
  echo "Uninstalling existing $PACKAGE..."
  adb -s "$DEVICE" uninstall "$PACKAGE" 2>/dev/null || {
    echo -e "${YELLOW}Uninstall failed — trying with --user 0...${NC}"
    adb -s "$DEVICE" shell pm uninstall --user 0 "$PACKAGE" 2>/dev/null || true
  }
  sleep 1
fi

# ── 6. Install new APK ───────────────────────────────────────────

echo "Installing..."
if adb -s "$DEVICE" install -r "$APK" 2>&1; then
  echo -e "${GREEN}Install OK${NC}"
else
  # Retry once
  echo -e "${YELLOW}Retrying install...${NC}"
  sleep 2
  if adb -s "$DEVICE" install -r "$APK" 2>&1; then
    echo -e "${GREEN}Install OK (retry)${NC}"
  else
    echo -e "${RED}Install failed.${NC}"
    exit 1
  fi
fi

# ── 7. Reverse port forwarding (dev server) ────────────────────

echo "Setting up reverse proxy :${DEV_PORT} -> device:${DEV_PORT}..."
adb -s "$DEVICE" reverse tcp:"$DEV_PORT" tcp:"$DEV_PORT" 2>/dev/null && \
  echo -e "${GREEN}Reverse proxy active:${NC}" && \
  adb -s "$DEVICE" reverse --list 2>/dev/null || \
  echo -e "${YELLOW}Reverse proxy failed (non-fatal)${NC}"

# ── 8. Launch the app ────────────────────────────────────────────

echo "Launching..."
adb -s "$DEVICE" shell monkey -p "$PACKAGE" -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1

echo -e "${GREEN}Done — QxChat launched on $DEVICE${NC}"

# ── 9. Show logcat (optional) ────────────────────────────────────

if $DO_LOG; then
  echo "── logcat ────────────────────────────────────────"
  adb -s "$DEVICE" logcat -c
  adb -s "$DEVICE" logcat -v time chromium:V *:S 2>/dev/null | grep -iE "qxp|tauri|console" || true
fi
