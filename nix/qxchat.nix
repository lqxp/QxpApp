{
  lib,
  rustPlatform,
  stdenvNoCC,
  fetchPnpmDeps,
  pnpmConfigHook,
  pkg-config,
  makeWrapper,
  wrapGAppsHook4,
  copyDesktopItems,
  makeDesktopItem,
  gobject-introspection,
  glib-networking,
  gtk3,
  webkitgtk_4_1,
  libsoup_3,
  openssl,
  glib,
  gdk-pixbuf,
  pango,
  cairo,
  atkmm,
  at-spi2-atk,
  harfbuzz,
  librsvg,
  dbus,
  gst_all_1,
  pipewire,
  libdrm,
  libgbm ? mesa,
  libglvnd,
  mesa,
  libepoxy,
  wayland,
  nodejs,
  pnpm,
  libayatana-appindicator,
}:

let
  pname = "qxchat";
  version = "1.14.9";

  webkitgtk = webkitgtk_4_1.override {
    enableExperimental = true;
  };

  frontendSrc = ../client;

  frontend = stdenvNoCC.mkDerivation {
    pname = "${pname}-frontend";
    inherit version;

    src = frontendSrc;

    nativeBuildInputs = [
      nodejs
      pnpm
      pnpmConfigHook
    ];

    pnpmDeps = fetchPnpmDeps {
      inherit pname version;
      src = frontendSrc;
      fetcherVersion = 4;

      pnpmInstallFlags = [
        "--config.minimum-release-age=0"
        "--force"
      ];

      hash = "sha256-oc9H/TyXRx/X8xdiGXjs9Wa6CodI2kaDi8XT84mDpns=";
    };

    buildPhase = ''
      runHook preBuild
      pnpm install --offline --frozen-lockfile --force

      mkdir -p ../files
      cat > ../files/config.custom.toml << 'TOML_EOF'
      [rtc]
      relayOnly = true
      defaultTurnServer = "google-stun"

      [[rtc.servers]]
      id = "qxp-turn"
      label = "QXP Server"
      hint = "Self-hosted relay on our infrastructure — may be unstable under load. No IP exposed, everything goes through our relay."
      turnUrls = [
          "turn:relay-01.qxch.at:3478?transport=udp",
          "turn:relay-01.qxch.at:3478?transport=tcp",
          "turns:relay-01.qxch.at:5349?transport=tcp",
      ]
      turnUsername = "qxp-turn"
      turnCredential = "ee74bf0f9bf21e74d98f2d85176c9b5cb85ffde977ec80e8"

      [[rtc.servers]]
      id = "google-stun"
      label = "Google STUN"
      hint = "Google public STUN — very stable and fast, but ⚠️ your IPs are exposed (no anonymous relay). Only useful on local networks or for testing."
      turnUrls = [
          "stun:stun.l.google.com:19302",
          "stun:stun1.l.google.com:19302",
      ]
      TOML_EOF

      QXP_SERVER_ORIGIN=https://qxch.at \
      QXP_API_BASE_URL=https://qxch.at \
      QXP_WS_URL=wss://qxch.at/ws \
      QXP_CALLS_ENABLED=true \
      QXP_RELAY_ONLY=true \
      pnpm run build:tauri
      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall
      mkdir -p $out
      cp -r dist $out/
      runHook postInstall
    '';
  };

  gstPlugins = [
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    gst_all_1.gst-plugins-good
    gst_all_1.gst-plugins-bad
    gst_all_1.gst-plugins-ugly
    gst_all_1.gst-libav
    gst_all_1.gst-plugins-rs
    pipewire
  ];

  gstPluginPath = lib.concatStringsSep ":" (map (pkg: "${pkg}/lib/gstreamer-1.0") gstPlugins);
  pipewireSpaPath = "${pipewire}/lib/spa-0.2";
  runtimeLibPath = lib.makeLibraryPath (
    [
      gtk3
      webkitgtk
      libsoup_3
      openssl
      glib
      gdk-pixbuf
      pango
      cairo
      atkmm
      at-spi2-atk
      glib-networking
      harfbuzz
      librsvg
      dbus
      libdrm
      libgbm
      libglvnd
      mesa
      libepoxy
      wayland
      pipewire
      libayatana-appindicator
    ]
    ++ gstPlugins
  );

  desktopItem = makeDesktopItem {
    name = "com.qxp.client";
    desktopName = "QxChat";
    exec = "qxchat";
    terminal = false;
    categories = [
      "Network"
      "Chat"
    ];
    icon = "qxchat";
    extraConfig = {
      StartupWMClass = "com.qxp.client";
    };
  };
in
rustPlatform.buildRustPackage {
  inherit pname version;

  # Keep client/dist in the source tree (it is gitignored but required by Tauri at build/runtime).
  src = ../.;
  cargoRoot = "src-tauri";
  buildAndTestSubdir = "src-tauri";

  cargoLock = {
    lockFile = ../src-tauri/Cargo.lock;
  };

  nativeBuildInputs = [
    pkg-config
    makeWrapper
    wrapGAppsHook4
    copyDesktopItems
    gobject-introspection
  ];

  postPatch = ''
        # Prevent Tauri from trying to run bun build steps inside the Rust build hook.
        substituteInPlace src-tauri/tauri.conf.json \
          --replace-fail '"beforeBuildCommand": "cd client && bun run build:tauri",' '"beforeBuildCommand": "",'

        rm -rf client/dist
        mkdir -p client
        cp -r ${frontend}/dist client/dist
        chmod -R u+w client/dist

  '';

  buildInputs = [
    gtk3
    webkitgtk
    libsoup_3
    openssl
    glib
    gdk-pixbuf
    pango
    cairo
    atkmm
    at-spi2-atk
    glib-networking
    harfbuzz
    librsvg
    dbus
    libdrm
    libgbm
    libglvnd
    mesa
    libepoxy
    wayland
  ]
  ++ gstPlugins;

  desktopItems = [ desktopItem ];

  postInstall = ''
    install -Dm644 src-tauri/icons/icon.png "$out/share/icons/hicolor/512x512/apps/qxchat.png"

    wrapProgram "$out/bin/qxchat" \
      --set G_APPLICATION_ID "com.qxp.client" \
      --set WEBKIT_DISABLE_DMABUF_RENDERER "1" \
      --set WEBKIT_DISABLE_COMPOSITING_MODE "1" \
      --set LD_LIBRARY_PATH "${runtimeLibPath}" \
      --set GIO_MODULE_DIR "${glib-networking}/lib/gio/modules" \
      --set GIO_EXTRA_MODULES "${glib-networking}/lib/gio/modules" \
      --set GST_PLUGIN_SYSTEM_PATH_1_0 "${gstPluginPath}" \
      --set GST_PLUGIN_PATH_1_0 "${gstPluginPath}" \
      --set GST_PLUGIN_SYSTEM_PATH "${gstPluginPath}" \
      --set GST_PLUGIN_PATH "${gstPluginPath}" \
      --set PIPEWIRE_MODULE_DIR "${pipewire}/lib/pipewire-0.3" \
      --set SPA_PLUGIN_DIR "${pipewireSpaPath}"
  '';

  meta = {
    description = "QxChat desktop client (Tauri)";
    homepage = "https://github.com/lqxp/client-tauri";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "qxchat";
  };
}
