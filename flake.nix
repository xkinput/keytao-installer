{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/bfc1b8a4574108ceef22f02bafcf6611380c100d";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { nixpkgs, flake-utils, ... }:
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

        ndkVersion = "26.1.10909125"; # NDK r26b — Tauri 2 requires r25+
        cmdLineToolsVer = "13.0";

        androidComposition = pkgs.androidenv.composeAndroidPackages {
          cmdLineToolsVersion = cmdLineToolsVer;
          platformToolsVersion = "35.0.2";
          buildToolsVersions = [ "35.0.0" ];
          platformVersions = [ "35" ];
          includeNDK = true;
          ndkVersions = [ ndkVersion ];
          abiVersions = [
            "arm64-v8a"
            "x86_64"
          ];
          includeEmulator = false;
          includeSources = false;
          includeSystemImages = false;
          systemImageTypes = [ ];
          useGoogleAPIs = false;
          useGoogleTVAddOns = false;
        };

        androidSdk = androidComposition.androidsdk;
      in
      {
        devShells.default = pkgs.mkShell {
          # Tools (go into PATH / nativeBuildInputs, no effect on RPATH)
          packages = [
            androidSdk
            pkgs.jdk17
            pkgs.pkg-config
          ];

          # Runtime libraries: placed in buildInputs so the Nix cc-wrapper
          # automatically injects -L/-rpath into NIX_LDFLAGS, which cargo uses
          # when linking. This gives every compiled binary the correct RPATH
          # without needing patchelf.
          buildInputs = with pkgs; [
            librime
            xorg.libxcb
            libxkbcommon
            wayland
            dbus
            gtk3
            webkitgtk_4_1
            glib
            gdk-pixbuf
            pango
            atk
            cairo
            harfbuzz
            bzip2
            openssl
            libsoup_3
            xdotool
            libayatana-appindicator
          ];

          ANDROID_HOME = "${androidSdk}/libexec/android-sdk";
          NDK_HOME = "${androidSdk}/libexec/android-sdk/ndk/${ndkVersion}";
          RIME_INCLUDE_DIR = "${pkgs.librime}/include";
          RIME_LIB_DIR = "${pkgs.librime}/lib";

          # Ensure runtime libs are findable even when cargo doesn't embed RPATH.
          # As a flake attribute (not shellHook), direnv's `use flake` exports this.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
            with pkgs;
            [
              librime
              xorg.libxcb
              libxkbcommon
              wayland
              dbus
              gtk3
              webkitgtk_4_1
              glib
              gdk-pixbuf
              pango
              atk
              cairo
              harfbuzz
              bzip2
              openssl
              libsoup_3
              xdotool
              libayatana-appindicator
            ]
          );

          shellHook = ''
            export JAVA_HOME="${pkgs.jdk17}"

            # Add Android platform-tools (adb) and cmdline-tools to PATH
            export PATH="${androidSdk}/libexec/android-sdk/platform-tools:$PATH"
            export PATH="${androidSdk}/libexec/android-sdk/cmdline-tools/${cmdLineToolsVer}/bin:$PATH"

            # Embed RPATH for all runtime libs so binaries work without LD_LIBRARY_PATH.
            # The -L flags are injected via NIX_LDFLAGS by mkShell; here we add -rpath
            # so the dynamic linker finds the libs at runtime even outside the shell.
            export RUSTFLAGS="-C link-arg=-Wl,-rpath,${
              pkgs.lib.makeLibraryPath (
                with pkgs;
                [
                  librime
                  xorg.libxcb
                  libxkbcommon
                  wayland
                  dbus
                  gtk3
                  webkitgtk_4_1
                  glib
                  gdk-pixbuf
                  pango
                  atk
                  cairo
                  harfbuzz
                  bzip2
                  openssl
                  libsoup_3
                  libayatana-appindicator
                ]
              )
            }"
            # Install Android Rust cross-compilation targets (idempotent)
            for _t in aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android; do
              rustup target add "$_t" 2>/dev/null || true
            done

            # Nix's mkShell sets DEVELOPER_DIR / SDKROOT to the Nix apple-sdk, which
            # lacks libiconv.tbd stubs. Cargo is configured to use /usr/bin/clang as
            # linker for host builds (src-tauri/.cargo/config.toml), so we point
            # SDKROOT at the Xcode CLT SDK so /usr/bin/clang can find system libs.
            unset DEVELOPER_DIR
            # Try CLT SDK paths in order of preference; fall back to leaving SDKROOT as-is.
            for _sdk_candidate in \
              "/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk" \
              "/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk" \
              "/Library/Developer/CommandLineTools/SDKs/MacOSX15.sdk"; do
              if [ -d "$_sdk_candidate" ]; then
                export SDKROOT="$_sdk_candidate"
                break
              fi
            done
          '';
        };
      }
    );
}
