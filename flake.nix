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
          packages = [
            androidSdk
            pkgs.jdk17
          ];

          ANDROID_HOME = "${androidSdk}/libexec/android-sdk";
          NDK_HOME = "${androidSdk}/libexec/android-sdk/ndk/${ndkVersion}";

          shellHook = ''
            export JAVA_HOME="${pkgs.jdk17}"

            # Add Android platform-tools (adb) and cmdline-tools to PATH
            export PATH="${androidSdk}/libexec/android-sdk/platform-tools:$PATH"
            export PATH="${androidSdk}/libexec/android-sdk/cmdline-tools/${cmdLineToolsVer}/bin:$PATH"

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
