; ─────────────────────────────────────────────────────────────────────────────
; Allternit Desktop — Custom NSIS Installer Script
;
; Injected via electron-builder nsis.include.
; Customises Welcome, Directory, Installing, and Finish pages.
; ─────────────────────────────────────────────────────────────────────────────

; ── Colours ───────────────────────────────────────────────────────────────────
; Background and text colours for the inner (non-sidebar) panel area.
; NSIS uses BBGGRR hex format.
!define MUI_BGCOLOR          "1A140A"   ; deep dark (matches sidebar gradient end)
!define MUI_TEXTCOLOR        "8CB0D4"   ; muted gold (D4B08C → reversed)

; Header and sidebar images are defined by electron-builder via package.json installerHeader, installerSidebar properties

; ── Abort confirmation ────────────────────────────────────────────────────────
!define MUI_ABORTWARNING
!define MUI_ABORTWARNING_TEXT          "Are you sure you want to quit the Allternit installer?"
!define MUI_ABORTWARNING_CANCEL_DEFAULT

; ── Welcome page ──────────────────────────────────────────────────────────────
!define MUI_WELCOMEPAGE_TITLE          "Welcome to Allternit  |  A://"
!define MUI_WELCOMEPAGE_TEXT           "This wizard will guide you through the installation of Allternit Desktop.$\r$\n$\r$\nAllternit is a fully offline AI platform that runs entirely on your machine — no subscription, no cloud.$\r$\n$\r$\nClick Next to continue."

; ── Directory page ────────────────────────────────────────────────────────────
!define MUI_DIRECTORYPAGE_TEXT_TOP     "Choose where to install Allternit Desktop."
!define MUI_DIRECTORYPAGE_TEXT_DESTINATION "Installation Folder"

; ── Finish page ───────────────────────────────────────────────────────────────
!define MUI_FINISHPAGE_TITLE           "Allternit Installed  |  A://"
!define MUI_FINISHPAGE_TEXT            "Allternit Desktop has been installed on your computer.$\r$\n$\r$\nType A:// in any supported browser or launcher to open your platform.$\r$\n$\r$\nClick Finish to close this wizard."
; MUI_FINISHPAGE_RUN defined by electron-builder
!define MUI_FINISHPAGE_RUN_TEXT        "Launch Allternit"
!define MUI_FINISHPAGE_LINK            "Visit allternit.com"
!define MUI_FINISHPAGE_LINK_LOCATION   "https://allternit.com"
; MUI_FINISHPAGE_SHOWREADME defined by electron-builder
; MUI_FINISHPAGE_NOREBOOTSUPPORT defined by electron-builder

; ── Uninstaller finish page ───────────────────────────────────────────────────
!define MUI_UNFINISHPAGE_NOAUTOCLOSE

; ── Progress bar ──────────────────────────────────────────────────────────────
; Use smooth progress bar so it feels like a modern installer
!define MUI_INSTFILESPAGE_PROGRESSBAR  "smooth"
