Name:           gizzi-code
Version:        2.2.0
Release:        1%{?dist}
Summary:        AI-powered terminal interface for the Allternit ecosystem

License:        Apache-2.0
URL:            https://docs.gizziio.com
# Glob/Grep use rg on PATH (gizzi-code does not statically link ripgrep)
Requires:       ripgrep
# Binary release asset (version-named tarball, tag gizzi-code/v%{version})
Source0:        https://github.com/Allternit/allternit-platform/releases/download/gizzi-code/v%{version}/gizzi-code-v%{version}-linux-x64.tar.gz

%description
Gizzi Code is an AI-powered terminal interface and runtime
for the Allternit ecosystem, providing intelligent code assistance
and terminal automation.

%prep
%autosetup

%build
# Binary is pre-built

%install
mkdir -p %{buildroot}/usr/local/bin
install -m 755 gizzi-code %{buildroot}/usr/local/bin/gizzi-code

%files
/usr/local/bin/gizzi-code

%changelog
* Tue Oct 06 2026 Allternit Technologies <team@allternit.io> - 2.2.0-1
- Update to 2.2.0

* Tue Sep 29 2026 Allternit Technologies <team@allternit.io> - 2.1.8-1
- Update to 2.1.8

* Tue Sep 29 2026 Allternit Technologies <team@allternit.io> - 2.1.7-1
- Update to 2.1.7

* Mon Sep 28 2026 Allternit Technologies <team@allternit.io> - 2.1.6-1
- Update to 2.1.6

* Mon Sep 28 2026 Allternit Technologies <team@allternit.io> - 2.1.5-1
- Update to 2.1.5

* Mon Sep 28 2026 Allternit Technologies <team@allternit.io> - 2.1.4-1
- Update to 2.1.4

* Sat Sep 26 2026 Allternit Technologies <team@allternit.io> - 2.1.3-1
- Update to 2.1.3

* Sat Sep 06 2026 Allternit Technologies <team@allternit.io> - 2.0.7-1
- Update to 2.0.7; onboarding auto-picks default brain

* Sat Sep 06 2026 Allternit Technologies <team@allternit.io> - 2.0.6-1
- Update to 2.0.6; fix /theme TUI crash; real TS syntax highlighting

* Fri Sep 05 2026 Allternit Technologies <team@allternit.io> - 2.0.5-1
- Update to 2.0.5; Cloud-first /model list; npm verify via npm pack

* Fri Sep 05 2026 Allternit Technologies <team@allternit.io> - 2.0.4-1
- Update to 2.0.4; gizzi onboarding --defaults

* Fri Sep 05 2026 Allternit Technologies <team@allternit.io> - 2.0.3-1
- Update to 2.0.3; reap session children; CLI brains and Cloud default

* Fri Sep 05 2026 Allternit Technologies <team@allternit.io> - 2.0.2-1
- Update to 2.0.2; GitHub Release assets at gizzi-code/v%{version}

* Wed Sep 03 2026 Allternit Technologies <team@allternit.io> - 1.0.2-1
- Update to 1.0.2; point Source0 at the version-named binary release asset

* Mon Apr 07 2025 Allternit Technologies <team@allternit.io> - 1.0.0-1
- Initial release
