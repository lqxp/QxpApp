#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if [ "${QXP_WINDOWS_BUILD_SHELL:-}" != "1" ]; then
  exec nix develop .#windows -c env QXP_WINDOWS_BUILD_SHELL=1 bash "$0" "$@"
fi

# Interactive: local dev or production?
HAS_LOCAL_FLAG=false
for arg in "$@"; do [[ "$arg" == "--local" ]] && HAS_LOCAL_FLAG=true; done

if [[ -t 0 ]] && ! $HAS_LOCAL_FLAG; then
  echo -e "\033[1;36mBuild target:\033[0m"
  echo "  [1] Production  (from files/config.custom.toml)"
  echo "  [2] Local dev   (http://127.0.0.1:4560)"
  read -rp "Choose [1/2] (default: 1): " choice
  if [[ "$choice" == "2" ]]; then
    export QXP_SERVER_ORIGIN="http://127.0.0.1:4560"
    export QXP_API_BASE_URL="http://127.0.0.1:4560"
    export QXP_WS_URL="ws://127.0.0.1:4560/ws"
    echo -e "\033[1;33mLocal dev mode\033[0m"
  fi
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

bundle_args=(--bundles nsis)
for arg in "$@"; do
  case "$arg" in
    --bundles|--bundles=*)
      bundle_args=()
      break
      ;;
  esac
done

bun install --no-save
bun tauri build --target x86_64-pc-windows-gnu "${bundle_args[@]}" "$@"
