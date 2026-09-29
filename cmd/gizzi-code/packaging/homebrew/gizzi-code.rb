class GizziCode < Formula
  desc "AI-powered terminal interface for the Allternit ecosystem"
  homepage "https://docs.gizziio.com"
  version "2.1.5"
  license "Apache-2.0"

  # gizzi-code doesn\'t statically link ripgrep; Glob/Grep use `rg` on PATH.
  depends_on "ripgrep"

  # Release tags look like "gizzi-code/v2.0.5"; assets are version-named:
  # gizzi-code-v2.0.5-<target>.tar.gz
  base_url = "https://github.com/Gizziio/allternit-platform/releases/download/gizzi-code/v#{version}"

  # macOS ARM64 (Apple Silicon)
  if OS.mac? && Hardware::CPU.arm?
    url "#{base_url}/gizzi-code-v#{version}-darwin-arm64.tar.gz"
    sha256 "b6f23aa24ff798e3ac1289ea0e3b8645f883a5e6e61cecc076e695fd0771c636"
  end

  # macOS Intel
  if OS.mac? && Hardware::CPU.intel?
    url "#{base_url}/gizzi-code-v#{version}-darwin-x64.tar.gz"
    sha256 "8e9ea36a2f5b23b34fd3538b9939b652b96afa6446abdc0ce6860f42e7b98eed"
  end

  # Linux ARM64
  if OS.linux? && Hardware::CPU.arm?
    url "#{base_url}/gizzi-code-v#{version}-linux-arm64.tar.gz"
    sha256 "480b3b66c218fd9512de2a669a0f3e7543c80adf52a8681fa037a949b82d94ac"
  end

  # Linux x64
  if OS.linux? && Hardware::CPU.intel?
    url "#{base_url}/gizzi-code-v#{version}-linux-x64.tar.gz"
    sha256 "7fbd11bb38a7415c58ed27fe45e0df2acbf14953628d35efd5b3be0aafadfaf1"
  end

  def install
    bin.install "gizzi-code"
    bin.install_symlink "gizzi-code" => "gizzi"

    # Install shell completions
    bash_completion.install "completions/gizzi-code.bash" if File.exist?("completions/gizzi-code.bash")
    zsh_completion.install "completions/_gizzi-code" if File.exist?("completions/_gizzi-code")
    fish_completion.install "completions/gizzi-code.fish" if File.exist?("completions/gizzi-code.fish")
  end

  test do
    system "#{bin}/gizzi-code", "--version"
  end
end
