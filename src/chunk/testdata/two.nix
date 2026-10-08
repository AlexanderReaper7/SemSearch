{ pkgs, lib, ... }:
{
  # First.
  environment.systemPackages = with pkgs; [
    git
    ripgrep
  ];

  # Second.
  services.demo = {
    enable = true;
    settings = {
      port = 8080;
      host = "127.0.0.1";
    };
  };
}
