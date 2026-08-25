{ pkgs ? import <nixpkgs> {} }:

let
  gstPlugins = [
    pkgs.gst_all_1.gstreamer.out
    pkgs.gst_all_1.gst-plugins-base
    pkgs.gst_all_1.gst-plugins-good
    pkgs.gst_all_1.gst-plugins-bad
    pkgs.gst_all_1.gst-plugins-ugly
    pkgs.gst_all_1.gst-libav
  ];
  gstTools = with pkgs.gst_all_1; [
    gstreamer
    gst-plugins-base
    gst-plugins-good
    gst-plugins-bad
    gst-plugins-ugly
    gst-libav
  ];
  gstPluginPath = pkgs.lib.concatStringsSep ":" (map (pkg: "${pkg}/lib/gstreamer-1.0") gstPlugins);
  basePackages = with pkgs; [
    at-spi2-atk
    atkmm
    alsa-lib
    bun
    cairo
    cargo
    cargo-tauri
    coreutils
    expat
    file
    findutils
    fontconfig
    fribidi
    freetype
    gdk-pixbuf
    gnugrep
    gnused
    glib
    glib.bin
    glib.dev
    glib-networking
    gobject-introspection
    gtk3
    harfbuzz
    libdrm
    libgbm
    libglvnd
    libgpg-error
    librsvg
    libsoup_3
    mesa
    openssl
    pango
    patchelf
    pkg-config
    rustc
    webkitgtk_4_1
    xdg-utils
    xdotool
    libx11
    libxcb
    libayatana-appindicator
    zlib
  ];
  allPackages = basePackages ++ gstTools ++ gstPlugins;
  libraryPath = pkgs.lib.makeLibraryPath allPackages;
  pkgConfigPath = pkgs.lib.makeSearchPathOutput "dev" "lib/pkgconfig" allPackages;
in
(pkgs.buildFHSEnv {
  name = "qxchat-appimage-build-env";

  targetPkgs = _: allPackages;

  profile = ''
    export GIO_MODULE_DIR="${pkgs.glib-networking}/lib/gio/modules"
    export GST_PLUGIN_SYSTEM_PATH_1_0="${gstPluginPath}"
    export LD_LIBRARY_PATH="/usr/lib:/lib:${libraryPath}:''${LD_LIBRARY_PATH:-}"
    export LIBRARY_PATH="${libraryPath}:''${LIBRARY_PATH:-}"
    export TRAY_LIBRARY_PATH="${pkgs.libayatana-appindicator}/lib/libayatana-appindicator3.so.1"
    export PATH="/usr/local/bin:''${PATH:-}"
    export PKG_CONFIG_PATH="/usr/local/lib/pkgconfig:${pkgConfigPath}:''${PKG_CONFIG_PATH:-}"
    export WEBKIT_DISABLE_DMABUF_RENDERER=1
    export LQXP_APPIMAGE_FHS=1
    unset SOURCE_DATE_EPOCH
  '';

  extraBuildCommands = ''
    mkdir -p "$out/usr/bin"
    ln -sf "${pkgs.xdg-utils}/bin/xdg-open" "$out/usr/bin/xdg-open"
    ln -sf "${pkgs.xdg-utils}/bin/xdg-mime" "$out/usr/bin/xdg-mime"
    mkdir -p "$out/usr/local/lib/pkgconfig" "$out/usr/local/share/lqxp-empty-glib-schemas"
    touch "$out/usr/local/share/lqxp-empty-glib-schemas/.keep"
    sed 's|^schemasdir=.*|schemasdir=/usr/local/share/lqxp-empty-glib-schemas|' \
      "${pkgs.glib.dev}/lib/pkgconfig/gio-2.0.pc" \
      > "$out/usr/local/lib/pkgconfig/gio-2.0.pc"

    mkdir -p "$out/usr/local/bin"
    cat > "$out/usr/local/bin/pkg-config" <<'EOF'
#!/usr/bin/env bash
set -e

package="''${@: -1}"
case "$package" in
  ayatana-appindicator3-0.1|appindicator3-0.1)
    case " $* " in
      *" --libs-only-L "*)
        echo "-L${pkgs.libayatana-appindicator}/lib"
        ;;
      *" --libs-only-l "*)
        echo "-layatana-appindicator3"
        ;;
      *" --libs "*)
        echo "-L${pkgs.libayatana-appindicator}/lib -layatana-appindicator3"
        ;;
      *)
        exec /usr/bin/pkg-config "$@"
        ;;
    esac
    ;;
  *)
    exec /usr/bin/pkg-config "$@"
    ;;
esac
EOF
    chmod +x "$out/usr/local/bin/pkg-config"

    cat > "$out/usr/local/bin/sed" <<'EOF'
#!/usr/bin/env bash
set -e

for arg in "$@"; do
  case "$arg" in
    */target/release/bundle/appimage/*)
      chmod u+w "$arg" 2>/dev/null || true
      chmod -R u+w "$(dirname "$arg")" 2>/dev/null || true
      ;;
  esac
done

exec /usr/bin/sed "$@"
EOF
    chmod +x "$out/usr/local/bin/sed"

    cat > "$out/usr/local/bin/cp" <<'EOF'
#!/usr/bin/env bash
set +e

target_dir=
sources=()
skip_next=0

for arg in "$@"; do
  if [ "$skip_next" -eq 1 ]; then
    target_dir="$arg"
    skip_next=0
    continue
  fi

  case "$arg" in
    --target-directory=*)
      target_dir="''${arg#--target-directory=}"
      ;;
    --target-directory)
      skip_next=1
      ;;
    -*)
      ;;
    *)
      sources+=("$arg")
      ;;
  esac
done

/usr/bin/cp "$@"
status=$?

if [ "$status" -eq 0 ]; then
  case "$target_dir" in
    */target/release/bundle/appimage/*)
      for src in "''${sources[@]}"; do
        case "$src" in
          /*)
            chmod -R u+w "$target_dir/$src" 2>/dev/null || true
            ;;
        esac
      done
      ;;
  esac
fi

exit "$status"
EOF
    chmod +x "$out/usr/local/bin/cp"

    cat > "$out/usr/local/bin/find" <<'EOF'
#!/usr/bin/env bash
set +e

for arg in "$@"; do
  if [ "$arg" = "-print0" ]; then
    /usr/bin/find "$@" | /usr/bin/grep -z -v -E '(-gdb\.py|\.py)$'
    exit "''${PIPESTATUS[0]}"
  fi
done

exec /usr/bin/find "$@"
EOF
    chmod +x "$out/usr/local/bin/find"
  '';

  runScript = "bash";
})
