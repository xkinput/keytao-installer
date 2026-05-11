{ self }:
{
  config,
  lib,
  pkgs,
  options,
  ...
}:

let
  cfg = config.programs.keytao-installer;
  system = pkgs.stdenv.hostPlatform.system;
  package = self.packages.${system}.default;
  hasNiri = options ? programs && options.programs ? niri && options.programs.niri ? settings;
in
{
  options.programs.keytao-installer = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = pkgs.stdenv.isLinux;
      description = "Install KeyTao installer and the keytao-ime Linux daemon.";
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = package;
      defaultText = "inputs.keytao-installer.packages.${system}.default";
      description = "Package providing keytao-installer and keytao-ime.";
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
      default = pkgs.stdenv.isLinux;
      description = "Start keytao-installer automatically for desktop sessions.";
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
        }
        // lib.optionalAttrs cfg.forceXimToolkitEnvironment {
          GTK_IM_MODULE = lib.mkDefault "xim";
          QT_IM_MODULE = lib.mkDefault "xim";
        };

        systemd.user.sessionVariables = {
          XMODIFIERS = "@im=keytao";
        }
        // lib.optionalAttrs cfg.forceXimToolkitEnvironment {
          GTK_IM_MODULE = lib.mkDefault "xim";
          QT_IM_MODULE = lib.mkDefault "xim";
        };
      })

      (lib.mkIf (cfg.autostart && hasNiri) {
        programs.niri.settings.spawn-at-startup = [
          { command = [ "${cfg.package}/bin/keytao-installer" ]; }
        ];
      })

      (lib.mkIf (cfg.autostart && !hasNiri) {
        xdg.configFile."autostart/keytao-installer.desktop".source =
          "${cfg.package}/share/applications/keytao-installer.desktop";
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
