#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if [ "${QXP_WINDOWS_BUILD_SHELL:-}" != "1" ]; then
  exec nix develop .#windows -c env QXP_WINDOWS_BUILD_SHELL=1 bash "$0" "$@"
fi

# ── Argument parsing ────────────────────────────────────────────
MODE=""
UPLOAD=false
tauri_args=()
for arg in "$@"; do
  case "$arg" in
    --prod) MODE="prod" ;;
    --local) MODE="local" ;;
    --upload) UPLOAD=true ;;
    *) tauri_args+=("$arg") ;;
  esac
done

# Interactive prompt if no mode flag
if [[ -z "$MODE" && -t 0 ]]; then
  echo -e "\033[1;36mBuild target:\033[0m"
  echo "  [1] Production  (from files/config.custom.toml)"
  echo "  [2] Local dev   (http://127.0.0.1:4560)"
  read -rp "Choose [1/2] (default: 1): " choice
  MODE="${choice:-1}"
fi

if [[ "$MODE" == "local" || "$MODE" == "2" ]]; then
  export QXP_SERVER_ORIGIN="http://127.0.0.1:4560"
  export QXP_API_BASE_URL="http://127.0.0.1:4560"
  export QXP_WS_URL="ws://127.0.0.1:4560/ws"
  echo -e "\033[1;33mLocal dev mode\033[0m"
else
  unset QXP_SERVER_ORIGIN QXP_API_BASE_URL QXP_WS_URL
  echo -e "\033[1;36mProduction mode\033[0m"
fi

# Production values come from files/config.custom.toml (via sync-runtime-config.mjs).
# No hardcoded env vars — the sync script handles the full chain: TOML → fallback.
# Only set defaults if running outside tauri build (e.g. direct xcodebuild).

export CARGO_BUILD_TARGET=x86_64-pc-windows-gnu
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
export TAURI_LINUX_AYATANA_APPINDICATOR=true

appindicator_lib="${TRAY_LIBRARY_PATH:-}"
if [ -z "$appindicator_lib" ]; then
  shopt -s nullglob
  appindicator_candidates=(/nix/store/*-libayatana-appindicator-*/lib/libayatana-appindicator3.so.1)
  shopt -u nullglob
  appindicator_lib="${appindicator_candidates[0]:-}"
fi
if [ -n "$appindicator_lib" ]; then
  export TRAY_LIBRARY_PATH="$appindicator_lib"
  pkg_config_wrapper_dir="$(mktemp -d)"
  real_pkg_config="$(command -v pkg-config)"
  cat > "$pkg_config_wrapper_dir/pkg-config" <<EOF
#!/usr/bin/env bash
package="\${@: -1}"
if [ "\$package" = "ayatana-appindicator3-0.1" ] || [ "\$package" = "ayatana-appindicator3" ]; then
  case " \$* " in
    *" --libs-only-L "*) echo "-L$(dirname "$appindicator_lib")" ;;
    *" --libs-only-l "*) echo "-layatana-appindicator3" ;;
    *" --libs "*) echo "-L$(dirname "$appindicator_lib") -layatana-appindicator3" ;;
    *" --variable=libdir "*) echo "$(dirname "$appindicator_lib")" ;;
    *" --exists "*) exit 0 ;;
    *) exit 0 ;;
  esac
else
  exec "$real_pkg_config" "\$@"
fi
EOF
  chmod +x "$pkg_config_wrapper_dir/pkg-config"
  export PATH="$pkg_config_wrapper_dir:$PATH"
fi

export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
export CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++
export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
export RANLIB_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ranlib

# Statically link mingw runtime so the .exe runs without extra DLLs on Windows
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS="-C target-feature=+crt-static ${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS:-}"

bun install --no-save
bun tauri build --target x86_64-pc-windows-gnu "${tauri_args[@]}"
BUILD_EXIT=$?

# ── Find built artifacts ─────────────────────────────────────────

find_exe() {
  echo "src-tauri/target/x86_64-pc-windows-gnu/release/qxchat.exe"
}

if [[ $BUILD_EXIT -eq 0 ]]; then
  ARTIFACT="$(find_exe)"
  if [[ -n "$ARTIFACT" && -f "$ARTIFACT" ]]; then
    do_upload=false
    if $UPLOAD; then
      do_upload=true
    elif [[ -t 0 ]]; then
      echo
      echo -e "\033[1;33mUpload to download.wf? (15s timeout) [y/N] \033[0m"
      read -rt 15 answer || answer=""
      [[ "$answer" =~ ^[Yy] ]] && do_upload=true
    fi

    if $do_upload; then
      SIZE=$(stat -c%s "$ARTIFACT" 2>/dev/null || echo 0)
      echo "Uploading $(basename "$ARTIFACT") (${SIZE} bytes)..."

      UPLOADER_ARGS=(--file "$ARTIFACT")
      [[ -n "${DOWNLOADWF_BASE:-}" ]] && UPLOADER_ARGS+=(--base "$DOWNLOADWF_BASE")
      [[ -n "${DOWNLOADWF_PASSWORD:-}" ]] && UPLOADER_ARGS+=(--password "$DOWNLOADWF_PASSWORD")

      UPLOAD_OUT="$(bun run scripts/downloadwf-uploader.mts "${UPLOADER_ARGS[@]}" 2>&1 || true)"
      URL="$(echo "$UPLOAD_OUT" | grep -oE 'https?://[A-Za-z0-9._:-]+/[a-z0-9]{8}' | head -1 || true)"
      if [[ -n "$URL" ]]; then
        echo -e "\033[0;32mDone: $URL\033[0m"
      else
        echo -e "\033[0;31mUpload failed:\033[0m"
        echo "$UPLOAD_OUT"
      fi
    fi
  fi
fi

exit $BUILD_EXIT
