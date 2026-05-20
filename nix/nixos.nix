{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.keytao-app;
  system = pkgs.stdenv.hostPlatform.system;
in
{
  options.services.keytao-app = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = pkgs.stdenv.isLinux;
      description = "Install the KeyTao app and expose keytao-ime environment defaults.";
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${system}.default;
      defaultText = "inputs.keytao-app.packages.${system}.default";
      description = "Package providing keytao-app and keytao-ime.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    environment.variables.XMODIFIERS = lib.mkDefault "@im=keytao";
  };
}
