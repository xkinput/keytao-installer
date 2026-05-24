{ self }:
{
  config,
  lib,
  pkgs,
  options,
  ...
}:

let
  cfg = config.programs.keytao-app;
  system = pkgs.stdenv.hostPlatform.system;
  package = self.packages.${system}.default;
  hasNiri = options ? programs && options.programs ? niri && options.programs.niri ? settings;
  kdeVirtualKeyboardDesktop = "keytao-wayland-launcher.desktop";
in
{
  options.programs.keytao-app = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = pkgs.stdenv.isLinux;
      description = "Install the KeyTao app and the keytao-ime Linux daemon.";
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = package;
      defaultText = "inputs.keytao-app.packages.${system}.default";
      description = "Package providing keytao-app and keytao-ime.";
    };

    kde = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable KDE Plasma Wayland virtual-keyboard integration for keytao-ime.";
    };

    kdeAutoConfigureVirtualKeyboard = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Configure Plasma KWin to use KeyTao as the Wayland Virtual Keyboard.";
    };

    setInputMethodEnvironment = lib.mkOption {
      type = lib.types.bool;
      default = pkgs.stdenv.isLinux;
      description = "Export toolkit environment variables for keytao-ime compatibility.";
    };

    forceXimToolkitEnvironment = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Force GTK_IM_MODULE and QT_IM_MODULE to xim for legacy X11 applications.";
    };

    autostart = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Start keytao-app automatically for desktop sessions.";
    };

    autostartDaemon = lib.mkOption {
      type = lib.types.bool;
      default = pkgs.stdenv.isLinux;
      description = "Start keytao-ime daemon automatically via XDG autostart.";
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        home.packages = [ cfg.package ];
      }

      (lib.mkIf cfg.setInputMethodEnvironment {
        home.sessionVariables = {
          XMODIFIERS = "@im=keytao";
          GTK_IM_MODULE = lib.mkDefault "ibus";
          QT_IM_MODULE = lib.mkDefault "ibus";
        };

        systemd.user.sessionVariables = {
          XMODIFIERS = "@im=keytao";
          GTK_IM_MODULE = lib.mkDefault "ibus";
          QT_IM_MODULE = lib.mkDefault "ibus";
        };
      })

      (lib.mkIf cfg.kde {
        home.packages = [ pkgs.kdePackages.kconfig ];
      })

      (lib.mkIf (cfg.kde && cfg.kdeAutoConfigureVirtualKeyboard) {
        home.activation.configureKeytaoKdeVirtualKeyboard = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
          if [ -x "${pkgs.kdePackages.kconfig}/bin/kreadconfig6" ]; then
            CURRENT_IM=$("${pkgs.kdePackages.kconfig}/bin/kreadconfig6" --file "$HOME/.config/kwinrc" --group Wayland --key InputMethod || true)
            if [ "$CURRENT_IM" != "${kdeVirtualKeyboardDesktop}" ]; then
              "${pkgs.kdePackages.kconfig}/bin/kwriteconfig6" --file "$HOME/.config/kwinrc" --group Wayland --key InputMethod "${kdeVirtualKeyboardDesktop}"
            fi
          fi
        '';
      })

      (lib.mkIf (cfg.autostart && hasNiri) {
        programs.niri.settings.spawn-at-startup = [
          { command = [ "${cfg.package}/bin/keytao-app" ]; }
        ];
      })

      (lib.mkIf (cfg.autostart && !hasNiri) {
        xdg.configFile."autostart/keytao-app.desktop".source =
          "${cfg.package}/share/applications/keytao-app.desktop";
      })

      # Keep a compatibility daemon for IBus/XIM clients; KWin starts the native
      # Wayland input-method instance separately from keytao-wayland-launcher.desktop.
      (lib.mkIf (cfg.autostartDaemon || cfg.kde) {
        xdg.configFile."autostart/keytao-ime.desktop".text = ''
          [Desktop Entry]
          Name=KeyTao IME Daemon
          Exec=${cfg.package}/bin/keytao-ime
          Icon=keytao-app
          Type=Application
          NoDisplay=true
          X-KDE-autostart-phase=1
        '';
      })

      (lib.mkIf (cfg.setInputMethodEnvironment && hasNiri) {
        programs.niri.settings.environment = {
          "XMODIFIERS" = "@im=keytao";
        }
        // lib.optionalAttrs cfg.forceXimToolkitEnvironment {
          "GTK_IM_MODULE" = lib.mkDefault "xim";
          "QT_IM_MODULE" = lib.mkDefault "xim";
        };
      })
    ]
  );
}
