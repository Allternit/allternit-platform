# Homebrew formula for Gizzi Code (binary distribution)
# Usage: brew tap <you>/gizzi && brew install gizzi
#
# NOTE: canonical formula lives in packaging/homebrew/gizzi-code.rb in the
# Allternit/allternit-platform repo; this copy is for tap distribution.

class GizziCode < Formula
  desc "AI-powered terminal interface and runtime for the Allternit ecosystem"
  homepage "https://docs.gizziio.com"
  version "2.10.0"
  license "Apache-2.0"

  # gizzi-code doesn\'t statically link ripgrep; Glob/Grep use `rg` on PATH.
  depends_on "ripgrep"

  # Release tags look like "gizzi-code/v1.0.2"; assets are version-named:
  # gizzi-code-v1.0.2-<target>.tar.gz
  base_url = "https://github.com/Allternit/allternit-platform/releases/download/gizzi-code/v#{version}"

  if OS.mac? && Hardware::CPU.arm?
    url "#{base_url}/gizzi-code-v#{version}-darwin-arm64.tar.gz"
    sha256 "5078ea0ca5ea57deef976c4b63625e1406768f659cf2c058b559b7077b62435a"
  elsif OS.mac? && Hardware::CPU.intel?
    url "#{base_url}/gizzi-code-v#{version}-darwin-x64.tar.gz"
    sha256 "199c6112eab314747cbf0ecbeeebbcb8ba846162b0f6123a55729ed26055c661"
  elsif OS.linux? && Hardware::CPU.arm?
    url "#{base_url}/gizzi-code-v#{version}-linux-arm64.tar.gz"
    sha256 "d7e3c91ba4beea73c1a11b4b05a0e2febf30356123ac2f94c3335ed5fe48a915"
  elsif OS.linux? && Hardware::CPU.intel?
    url "#{base_url}/gizzi-code-v#{version}-linux-x64.tar.gz"
    sha256 "ed2c7ac3fd1aedc4be0c42327117d24683ae2deaa6ddac86630964d749720b43"
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
