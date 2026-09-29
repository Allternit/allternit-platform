# Homebrew formula for Gizzi Code (binary distribution)
# Usage: brew tap <you>/gizzi && brew install gizzi
#
# NOTE: canonical formula lives in packaging/homebrew/gizzi-code.rb in the
# Gizziio/allternit-platform repo; this copy is for tap distribution.

class GizziCode < Formula
  desc "AI-powered terminal interface and runtime for the Allternit ecosystem"
  homepage "https://docs.gizziio.com"
  version "2.1.6"
  license "Apache-2.0"

  # gizzi-code doesn\'t statically link ripgrep; Glob/Grep use `rg` on PATH.
  depends_on "ripgrep"

  # Release tags look like "gizzi-code/v1.0.2"; assets are version-named:
  # gizzi-code-v1.0.2-<target>.tar.gz
  base_url = "https://github.com/Gizziio/allternit-platform/releases/download/gizzi-code/v#{version}"

  if OS.mac? && Hardware::CPU.arm?
    url "#{base_url}/gizzi-code-v#{version}-darwin-arm64.tar.gz"
    sha256 "ada7244a3e6f40a499a9c7256747f87ad3cca80f4fb111a8dc789f41cc59f980"
  elsif OS.mac? && Hardware::CPU.intel?
    url "#{base_url}/gizzi-code-v#{version}-darwin-x64.tar.gz"
    sha256 "dd158bb86741d8218497f27e54ccf0e8b1117bce3e61ef21d407e3c16f9dd7b3"
  elsif OS.linux? && Hardware::CPU.arm?
    url "#{base_url}/gizzi-code-v#{version}-linux-arm64.tar.gz"
    sha256 "fe936bc78311b36a127ccf8d55461081f1c04bc6d967311bac9e38b536ea7d78"
  elsif OS.linux? && Hardware::CPU.intel?
    url "#{base_url}/gizzi-code-v#{version}-linux-x64.tar.gz"
    sha256 "92f963edf9dad3488aa1b5957aa285ae4d33894bf1a3114a05454bae5cc86341"
  end

  def install
    bin.install "gizzi-code"
    bin.install_symlink "gizzi-code" => "gizzi"
  end

  service do
    run [opt_bin/"gizzi-code", "daemon", "start"]
    keep_alive true
    error_log_path var/"log/gizzi-code/error.log"
    log_path var/"log/gizzi-code/output.log"
    working_dir var/"run/gizzi-code"
  end

  test do
    system "#{bin}/gizzi-code", "--version"
  end
end
