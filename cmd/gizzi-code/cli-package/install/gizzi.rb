# Homebrew formula for Gizzi Code (binary distribution)
# Usage: brew tap <you>/gizzi && brew install gizzi
#
# NOTE: canonical formula lives in packaging/homebrew/gizzi-code.rb in the
# Allternit/allternit-platform repo; this copy is for tap distribution.

class GizziCode < Formula
  desc "AI-powered terminal interface and runtime for the Allternit ecosystem"
  homepage "https://docs.gizziio.com"
  version "2.2.16"
  license "Apache-2.0"

  # gizzi-code doesn\'t statically link ripgrep; Glob/Grep use `rg` on PATH.
  depends_on "ripgrep"

  # Release tags look like "gizzi-code/v1.0.2"; assets are version-named:
  # gizzi-code-v1.0.2-<target>.tar.gz
  base_url = "https://github.com/Allternit/allternit-platform/releases/download/gizzi-code/v#{version}"

  if OS.mac? && Hardware::CPU.arm?
    url "#{base_url}/gizzi-code-v#{version}-darwin-arm64.tar.gz"
    sha256 "ffe55bae1dff76d6f1fb5bc383dfee7229a5458d6a72e20e9af689fa15d9ea91"
  elsif OS.mac? && Hardware::CPU.intel?
    url "#{base_url}/gizzi-code-v#{version}-darwin-x64.tar.gz"
    sha256 "06fc787f40a83db7efc01982ba628c9f527167fc349ae5b7982fe6f624efbbcb"
  elsif OS.linux? && Hardware::CPU.arm?
    url "#{base_url}/gizzi-code-v#{version}-linux-arm64.tar.gz"
    sha256 "a1e1e623d247612b6efe032dedc3ca78778294942cbb37cddee371c2ab855e73"
  elsif OS.linux? && Hardware::CPU.intel?
    url "#{base_url}/gizzi-code-v#{version}-linux-x64.tar.gz"
    sha256 "9a4006785f75aec3379b73e8e94a08a1529e46f068eac560b9dc8dd4466e8258"
  end

  def install
    # gizzi-code and the Allternit Factory engine live side by side in
    # libexec: gizzi finds allternit-factory next to its own (resolved)
    # binary, and only `gizzi` / `gizzi-code` go on PATH.
    libexec.install "gizzi-code"
    libexec.install "allternit-factory" if File.exist?("allternit-factory")
    bin.install_symlink libexec/"gizzi-code"
    bin.install_symlink libexec/"gizzi-code" => "gizzi"
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
