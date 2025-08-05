{ }:
let
  pkgs = import <nixpkgs> { };
in
pkgs.mkShell {
  name = "default-dev-environment";
  packages = with pkgs; [
    pkg-config
    openssl
    rustc
  ];
  PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
}
