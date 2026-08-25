{
  description = "LQXP Client Tauri development shell (Android + Fenix Rust)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix.url = "github:nix-community/fenix";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      fenix,
    }:

    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          config = {
            allowUnfree = true;
            android_sdk.accept_license = true;
          };
        };

        fenixPkgs = fenix.packages.${system};

        rustToolchain = fenixPkgs.combine [
          # Keep the shell lean for CI: cargo/rustc/std are enough here and
          # avoid pulling extras like rustfmt/clippy/docs into the Nix closure.
          fenixPkgs.stable.cargo
          fenixPkgs.stable.rustc
          fenixPkgs.stable.rust-std

          fenixPkgs.targets.aarch64-linux-android.stable.rust-std
          fenixPkgs.targets.armv7-linux-androideabi.stable.rust-std
          fenixPkgs.targets.i686-linux-android.stable.rust-std
          fenixPkgs.targets.x86_64-linux-android.stable.rust-std
        ];

        windowsRustToolchain = fenixPkgs.combine [
          fenixPkgs.stable.cargo
          fenixPkgs.stable.rustc
          fenixPkgs.stable.rust-std
          fenixPkgs.targets.x86_64-pc-windows-gnu.stable.rust-std
        ];

        androidComposition = pkgs.androidenv.composeAndroidPackages {
          platformVersions = [
            "35"
            "36"
            "latest"
          ];
          buildToolsVersions = [
            "35.0.0"
            "latest"
          ];
          abiVersions = [
            "armeabi-v7a"
            "arm64-v8a"
            "x86"
            "x86_64"
          ];

          includeCmake = "if-supported";
          includeEmulator = "if-supported";
          includeNDK = "if-supported";
          includeSystemImages = false;
          ndkVersions = [ "27.0.12077973" ];
        };

        androidSdk = androidComposition.androidsdk;
        androidSdkRoot = "${androidSdk}/libexec/android-sdk";

        jdk = pkgs.jdk17;

        gstPlugins = [
          pkgs.gst_all_1.gstreamer.out
          pkgs.gst_all_1.gst-plugins-base
          pkgs.gst_all_1.gst-plugins-good
          pkgs.gst_all_1.gst-plugins-bad
          pkgs.gst_all_1.gst-plugins-ugly
          pkgs.gst_all_1.gst-libav
          pkgs.gst_all_1.gst-plugins-rs
          pkgs.pipewire
        ];

        gstPluginPath = pkgs.lib.concatStringsSep ":" (map (pkg: "${pkg}/lib/gstreamer-1.0") gstPlugins);

        webkitgtk = pkgs.webkitgtk_4_1.override {
          enableExperimental = true;
        };

      in
      {
        devShells.windows = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            windowsRustToolchain
            cargo-tauri
            bun
            pkg-config
            pkgsCross.mingwW64.stdenv.cc
            libayatana-appindicator
            nsis
          ];

          depsTargetTarget = [
            pkgs.pkgsCross.mingwW64.windows.pthreads
          ];

          shellHook = ''
            export CARGO_BUILD_TARGET=x86_64-pc-windows-gnu
            export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
            export TAURI_LINUX_AYATANA_APPINDICATOR=true
            export TRAY_LIBRARY_PATH="${pkgs.libayatana-appindicator}/lib/libayatana-appindicator3.so.1"
            export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
            export CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++
            export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
            export RANLIB_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ranlib
            export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS="-C target-feature=+crt-static"
            mingw_lib="${pkgs.pkgsCross.mingwW64.stdenv.cc.cc}/x86_64-w64-mingw32/lib"
            pthread_lib="${pkgs.pkgsCross.mingwW64.windows.pthreads}/lib"
            export LIBRARY_PATH="$pthread_lib:$mingw_lib:''${LIBRARY_PATH:-}"
            export RUSTFLAGS="-L native=$pthread_lib -L native=$mingw_lib ''${RUSTFLAGS:-}"
            echo "✔ Tauri Windows build environment ready"
          '';
        };

        devShells.default = pkgs.mkShell {

          nativeBuildInputs = with pkgs; [
            rustToolchain
            cargo-tauri
            cargo-ndk

            bun

            androidComposition.platform-tools
            androidSdk
            gradle
            jdk

            pkg-config
            gobject-introspection

            wrapGAppsHook4
            gst_all_1.gstreamer.dev
          ];

          buildInputs =
            (with pkgs; [
              at-spi2-atk
              atkmm
              cairo
              gdk-pixbuf
              glib
              glib-networking
              gtk3
              harfbuzz
              librsvg
              libsoup_3
              openssl
              pango
              webkitgtk
              xdotool
              dbus
              libayatana-appindicator
              alsa-lib
            ])
            ++ gstPlugins;

          shellHook = ''
            export JAVA_HOME="${jdk.home}"
            export ANDROID_HOME="${androidSdkRoot}"
            export ANDROID_SDK_ROOT="${androidSdkRoot}"

            # -----------------------
            # NDK setup
            # -----------------------
            android_ndk_dir="$ANDROID_SDK_ROOT/ndk-bundle"

            if [ -d "$ANDROID_SDK_ROOT/ndk" ]; then
              android_ndk_candidate="$(
                find "$ANDROID_SDK_ROOT/ndk" \
                  -mindepth 1 -maxdepth 1 -type d \
                  | sort -V | tail -n 1
              )"

              if [ -n "$android_ndk_candidate" ]; then
                android_ndk_dir="$android_ndk_candidate"
              fi
            fi

            export ANDROID_NDK_HOME="$android_ndk_dir"
            export ANDROID_NDK_ROOT="$android_ndk_dir"
            export NDK_HOME="$android_ndk_dir"

            export ANDROID_API_LEVEL="24"
            export ANDROID_PLATFORM="android-$ANDROID_API_LEVEL"

            # -----------------------
            # Tauri targets
            # -----------------------
            : "''${TAURI_ANDROID_RUST_TARGETS:=aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android}"
            export TAURI_ANDROID_RUST_TARGETS

            # -----------------------
            # PATH setup
            # -----------------------
            export PATH="$ANDROID_SDK_ROOT/cmdline-tools/latest/bin:$ANDROID_SDK_ROOT/platform-tools:$ANDROID_SDK_ROOT/emulator:$PATH"

            # CMake
            if [ -d "$ANDROID_SDK_ROOT/cmake" ]; then
              CMAKE_ROOT="$(
                find "$ANDROID_SDK_ROOT/cmake" \
                  -mindepth 1 -maxdepth 1 -type d \
                  | sort -V | tail -n 1
              )"
              export PATH="$CMAKE_ROOT/bin:$PATH"
            fi

            # aapt2 fix
            aapt2="$(
              find "$ANDROID_SDK_ROOT/build-tools" \
                -name aapt2 -type f 2>/dev/null \
                | sort -V | tail -n 1
            )"

            if [ -n "$aapt2" ]; then
              export GRADLE_OPTS="-Dorg.gradle.project.android.aapt2FromMavenOverride=$aapt2"
            fi

            # -----------------------
            # GTK / WebKit / GStreamer
            # -----------------------
            export GST_PLUGIN_SYSTEM_PATH_1_0="${gstPluginPath}"
            export GST_PLUGIN_PATH_1_0="${gstPluginPath}"
            export GST_PLUGIN_SYSTEM_PATH="${gstPluginPath}"
            export GST_PLUGIN_PATH="${gstPluginPath}"
            export GIO_MODULE_DIR="${pkgs.glib-networking}/lib/gio/modules"
            export WEBKIT_DISABLE_DMABUF_RENDERER=1
            export WEBKIT_DISABLE_COMPOSITING_MODE=1
            export WEBKIT_DISABLE_DMABUF_RENDERER=1

            export LD_LIBRARY_PATH="$LD_LIBRARY_PATH:${
              pkgs.lib.makeLibraryPath [
                webkitgtk

                pkgs.gtk3
                pkgs.glib
                pkgs.gdk-pixbuf
                pkgs.pango
                pkgs.cairo
                pkgs.atkmm
                pkgs.at-spi2-atk
                pkgs.glib-networking
                pkgs.harfbuzz
                pkgs.librsvg
                pkgs.libsoup_3
                pkgs.openssl
                pkgs.dbus

                pkgs.gst_all_1.gstreamer
                pkgs.gst_all_1.gst-plugins-base
                pkgs.gst_all_1.gst-plugins-good
                pkgs.gst_all_1.gst-plugins-bad
                pkgs.gst_all_1.gst-plugins-ugly
                pkgs.gst_all_1.gst-libav
                pkgs.libayatana-appindicator
              ]
            }"

            export GIO_EXTRA_MODULES="${pkgs.glib-networking}/lib/gio/modules"
            export GTK_PATH="${pkgs.gtk3}/lib/gtk-3.0"

            # -----------------------
            # NDK linker setup
            # -----------------------
            case "$(uname -s)-$(uname -m)" in
              Linux-x86_64)
                host="linux-x86_64"
                ;;
              *)
                host=""
                ;;
            esac

            if [ -n "$host" ]; then
              bin="$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/$host/bin"

              export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$bin/aarch64-linux-android$ANDROID_API_LEVEL-clang"
              export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="$bin/armv7a-linux-androideabi$ANDROID_API_LEVEL-clang"
              export CARGO_TARGET_I686_LINUX_ANDROID_LINKER="$bin/i686-linux-android$ANDROID_API_LEVEL-clang"
              export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$bin/x86_64-linux-android$ANDROID_API_LEVEL-clang"
            fi

            # -----------------------
            # env file
            # -----------------------
            if [ -f .env ]; then
              set -a
              . ./.env
              set +a
            fi

            echo "✔ Tauri Android + Fenix environment ready"
          '';
        };
      }
    );
}
