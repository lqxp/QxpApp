#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

load_dotenv() {
  if [[ -f .env ]]; then
    set -a
    # shellcheck disable=SC1091
    source .env
    set +a
    export LQXP_DOTENV_LOADED=1
  fi
}

load_dotenv

if [[ "${LQXP_ANDROID_BUILD_RUNNING:-}" != "1" ]]; then
  if command -v nix >/dev/null 2>&1 && [[ -f flake.nix ]]; then
    echo "Entering nix develop for Android build..."
    exec env TMPDIR=/tmp nix develop \
      --command env TMPDIR=/tmp LQXP_ANDROID_BUILD_RUNNING=1 \
      scripts/build-android.sh "$@"
  fi

  echo "warning: not running inside nix develop, continuing with the current environment." >&2
fi

command -v bun >/dev/null 2>&1 || {
  echo "error: bun is required." >&2
  exit 1
}

command -v rustup >/dev/null 2>&1 || {
  echo "error: rustup is required." >&2
  exit 1
}

export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"
export TMPDIR="/tmp"
export LQXP_RUSTUP_BIN_DIR="/tmp/qxchat-rustup-bin-${UID:-$(id -u)}"
mkdir -p "$LQXP_RUSTUP_BIN_DIR"

cat > "$LQXP_RUSTUP_BIN_DIR/cargo" <<'EOF'
#!/usr/bin/env bash
exec rustup run "${RUSTUP_TOOLCHAIN:-stable}" cargo "$@"
EOF

cat > "$LQXP_RUSTUP_BIN_DIR/rustc" <<'EOF'
#!/usr/bin/env bash
exec rustup run "${RUSTUP_TOOLCHAIN:-stable}" rustc "$@"
EOF

chmod +x "$LQXP_RUSTUP_BIN_DIR/cargo" "$LQXP_RUSTUP_BIN_DIR/rustc"
export PATH="$LQXP_RUSTUP_BIN_DIR:$PATH"
export CARGO="$LQXP_RUSTUP_BIN_DIR/cargo"
export RUSTC="$LQXP_RUSTUP_BIN_DIR/rustc"
hash -r 2>/dev/null || true

command -v cargo >/dev/null 2>&1 || {
  echo "error: cargo is required." >&2
  exit 1
}

is_release_build() {
  local arg
  for arg in "$@"; do
    if [[ "$arg" == "--debug" ]]; then
      return 1
    fi
  done

  return 0
}

sync_android_launcher_icons() {
  local icon_source="src-tauri/icons/android"
  local res_target="src-tauri/gen/android/app/src/main/res"

  [[ -d "$icon_source" && -d "$res_target" ]] || return 0

  for dir in "$icon_source"/*; do
    [[ -d "$dir" ]] || continue
    local name
    name="$(basename "$dir")"
    mkdir -p "$res_target/$name"
    cp -f "$dir"/* "$res_target/$name/"
  done

  echo "Synced Android launcher icons from $icon_source to $res_target"
}

configure_android_release_signing() {
  is_release_build "$@" || return 0

  local keystore_properties="src-tauri/gen/android/keystore.properties"

  # Auto-detect the release keystore: env var → home config → project root
  local project_root
  project_root="$(cd "$(dirname "$0")/.." && pwd)"
  local keystore_path="${ANDROID_KEYSTORE_PATH:-}"
  if [[ -z "$keystore_path" ]]; then
    local home_keystore="$HOME/.config/qxchat/qxchat-release.jks"
    if [[ -f "$home_keystore" ]]; then
      keystore_path="$home_keystore"
    elif [[ -f "$project_root/lqxp-release.jks" ]]; then
      keystore_path="$project_root/lqxp-release.jks"
    else
      keystore_path="$home_keystore"
    fi
  fi

  local keystore_password="${ANDROID_KEYSTORE_PASSWORD:-}"
  local key_alias="${ANDROID_KEY_ALIAS:-lqxp}"
  local key_password="${ANDROID_KEY_PASSWORD:-$keystore_password}"

  local has_explicit_signing_config=0
  if [[ -n "${ANDROID_KEYSTORE_PATH:-}${ANDROID_KEYSTORE_PASSWORD:-}${ANDROID_KEY_ALIAS:-}${ANDROID_KEY_PASSWORD:-}${LQXP_ANDROID_CREATE_KEYSTORE:-}${LQXP_DOTENV_LOADED:-}" ]]; then
    has_explicit_signing_config=1
  fi

  if [[ -f "$keystore_properties" && "${LQXP_REWRITE_ANDROID_KEYSTORE_PROPERTIES:-}" != "1" && "$has_explicit_signing_config" != "1" ]]; then
    return 0
  fi

  if [[ ! -f "$keystore_path" && "${LQXP_ANDROID_CREATE_KEYSTORE:-}" == "1" ]]; then
    command -v keytool >/dev/null 2>&1 || {
      echo "error: keytool is required to create an Android release keystore." >&2
      exit 1
    }

    if [[ -z "$keystore_password" && -t 0 ]]; then
      read -rsp "Android release keystore password: " keystore_password
      echo
    fi

    if [[ -z "$keystore_password" ]]; then
      echo "error: ANDROID_KEYSTORE_PASSWORD is required when LQXP_ANDROID_CREATE_KEYSTORE=1." >&2
      exit 1
    fi

    key_password="${ANDROID_KEY_PASSWORD:-$keystore_password}"
    mkdir -p "$(dirname "$keystore_path")"
    keytool -genkeypair \
      -v \
      -keystore "$keystore_path" \
      -storepass "$keystore_password" \
      -alias "$key_alias" \
      -keypass "$key_password" \
      -keyalg RSA \
      -keysize 2048 \
      -validity 10000 \
      -dname "${ANDROID_KEY_DNAME:-CN=LQXP Client, OU=LQXP, O=LQXP, L=Unknown, ST=Unknown, C=XX}"
  fi

  if [[ -f "$keystore_path" && -n "$keystore_password" && -n "$key_alias" && -n "$key_password" ]]; then
    cat > "$keystore_properties" <<KSPROP
ANDROID_KEYSTORE_PATH=$keystore_path
ANDROID_KEYSTORE_PASSWORD=$keystore_password
ANDROID_KEY_ALIAS=$key_alias
ANDROID_KEY_PASSWORD=$key_password
KSPROP
    chmod 600 "$keystore_properties"
    echo "Android release signing enabled via $keystore_properties"
    return 0
  fi

  # Keystore file exists but no password in env → prompt interactively
  if [[ -f "$keystore_path" && -t 0 ]]; then
    read -rsp "Keystore password for $keystore_path: " keystore_password
    echo
    key_alias="${ANDROID_KEY_ALIAS:-lqxp}"
    key_password="${ANDROID_KEY_PASSWORD:-$keystore_password}"
    if [[ -n "$keystore_password" ]]; then
      cat > "$keystore_properties" <<KSPROP
ANDROID_KEYSTORE_PATH=$keystore_path
ANDROID_KEYSTORE_PASSWORD=$keystore_password
ANDROID_KEY_ALIAS=$key_alias
ANDROID_KEY_PASSWORD=$key_password
KSPROP
      chmod 600 "$keystore_properties"
      echo "Android release signing enabled via $keystore_properties"
      return 0
    fi
  fi

  cat >&2 <<WARN
warning: release signing is not configured; Gradle will produce an unsigned APK.

The release keystore was auto-detected at: $keystore_path
WARN
  if [[ -f "$keystore_path" ]]; then
    echo "  → Keystore file exists, but password is missing." >&2
  else
    echo "  → Keystore file NOT found." >&2
  fi
  cat >&2 <<WARN

To sign the APK, set these environment variables (e.g. in a .env file):
  ANDROID_KEYSTORE_PASSWORD='<password>'
  ANDROID_KEY_ALIAS='lqxp'         (default: lqxp)
  ANDROID_KEY_PASSWORD='<password>' (defaults to ANDROID_KEYSTORE_PASSWORD)

Or to create a new local debug keystore:
  LQXP_ANDROID_CREATE_KEYSTORE=1 ANDROID_KEYSTORE_PASSWORD='change-me' bun run build:android
WARN
}

if [[ -z "${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}" ]]; then
  echo "error: ANDROID_SDK_ROOT/ANDROID_HOME is not set. Run through nix develop." >&2
  exit 1
fi

if [[ -z "${ANDROID_NDK_ROOT:-${ANDROID_NDK_HOME:-${NDK_HOME:-}}}" ]]; then
  echo "error: ANDROID_NDK_ROOT/ANDROID_NDK_HOME/NDK_HOME is not set. Run through nix develop." >&2
  exit 1
fi

if declare -F lqxp-rustup-android-targets >/dev/null 2>&1; then
  lqxp-rustup-android-targets
else
  rustup toolchain install "$RUSTUP_TOOLCHAIN" --profile minimal
  rustup target add --toolchain "$RUSTUP_TOOLCHAIN" \
    aarch64-linux-android \
    armv7-linux-androideabi \
    i686-linux-android \
    x86_64-linux-android
fi

if [[ ! -d src-tauri/gen/android || "${LQXP_FORCE_ANDROID_INIT:-}" == "1" ]]; then
  bun tauri android init
fi

sync_android_launcher_icons

local_properties="src-tauri/gen/android/local.properties"
if [[ ! -f "$local_properties" || "${LQXP_REWRITE_ANDROID_LOCAL_PROPERTIES:-}" == "1" ]]; then
  {
    echo "# Generated by scripts/build-android.sh"
    echo "sdk.dir=${ANDROID_SDK_ROOT:-$ANDROID_HOME}"
    echo "ndk.dir=${ANDROID_NDK_ROOT:-${ANDROID_NDK_HOME:-$NDK_HOME}}"
    if [[ -n "${CMAKE_ROOT:-}" ]]; then
      echo "cmake.dir=$CMAKE_ROOT"
    fi
  } > "$local_properties"
fi

android_gradle_properties="src-tauri/gen/android/gradle.properties"
if [[ -f "$android_gradle_properties" ]] && ! grep -q '^org\.gradle\.daemon=' "$android_gradle_properties"; then
  {
    echo
    echo "# Generated by scripts/build-android.sh: Gradle daemons keep stale Tauri IPC env vars."
    echo "org.gradle.daemon=false"
  } >> "$android_gradle_properties"
fi

export GRADLE_OPTS="-Dorg.gradle.daemon=false ${GRADLE_OPTS:-}"
if [[ -x src-tauri/gen/android/gradlew ]]; then
  src-tauri/gen/android/gradlew --project-dir src-tauri/gen/android --stop >/dev/null 2>&1 || true
fi

bun install --no-save
(cd client && bun install --no-save)

build_args=("$@")
if [[ ${#build_args[@]} -eq 0 ]]; then
  build_args=(--apk --target aarch64)
fi

# Filter out our custom flags before passing to tauri/cargo
LOCAL_DEV=false
tauri_args=()
for arg in "${build_args[@]}"; do
  case "$arg" in
    --local) LOCAL_DEV=true ;;
    *) tauri_args+=("$arg") ;;
  esac
done
build_args=("${tauri_args[@]}")
if [[ ${#build_args[@]} -eq 0 ]]; then
  build_args=(--apk --target aarch64)
fi

if $LOCAL_DEV; then
  export QXP_SERVER_ORIGIN="${QXP_DEV_SERVER:-http://127.0.0.1:4560}"
  echo -e "\033[1;33mLocal dev mode: targeting $QXP_SERVER_ORIGIN\033[0m"
fi

configure_android_release_signing "${build_args[@]}"

set +e
bun tauri android build "${build_args[@]}"
BUILD_EXIT=$?
set -e

echo
echo "APK output(s):"
find src-tauri/gen/android/app/build/outputs -name "*.apk" -print 2>/dev/null || true

if [[ $BUILD_EXIT -eq 0 && -t 0 ]]; then
  echo
  read -rp "Deploy to Android device? [Y/n] " answer
  if [[ -z "$answer" || "$answer" =~ ^[Yy] ]]; then
    scripts/adb-deploy.sh "$@"
  fi
fi

exit $BUILD_EXIT
