"""Agent-browser downloads land in the Files drive's Downloads root
(contracts/files-drive SPEC section 3, plan step 17).

A site picks the suggested file name, so it is untrusted: it must not leave
the folder, hide as a dotfile, or replace a file already there.
"""

import asyncio
import sys
from pathlib import Path

DOMAIN_CORE_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(DOMAIN_CORE_ROOT / "gateway"))

import session_manager as sm  # noqa: E402


class FakeDownload:
    """Playwright's Download at the edge: a suggested name and save_as."""

    def __init__(self, suggested_filename: str, body: bytes):
        self.suggested_filename = suggested_filename
        self.body = body

    async def save_as(self, path: str) -> None:
        Path(path).write_bytes(self.body)


def test_downloads_are_off_unless_the_drive_is_on(monkeypatch, tmp_path):
    monkeypatch.delenv("ALLTERNIT_FILES_DRIVE", raising=False)
    monkeypatch.delenv("ALLTERNIT_FILES_DOWNLOADS_DIR", raising=False)
    assert sm.files_downloads_dir() is None
    monkeypatch.setenv("ALLTERNIT_FILES_DRIVE", "1")
    monkeypatch.setenv("HOME", str(tmp_path))
    assert sm.files_downloads_dir() == tmp_path / "Allternit" / "Downloads"


def test_a_hostile_suggested_name_stays_in_downloads_and_replaces_nothing(tmp_path):
    downloads = tmp_path / "Allternit" / "Downloads"
    (tmp_path / ".ssh").mkdir()
    keys = tmp_path / ".ssh" / "authorized_keys"
    keys.write_text("ssh-ed25519 OWNER")

    first = asyncio.run(sm.save_download(FakeDownload("../../.ssh/authorized_keys", b"attacker key"), downloads))
    assert first is not None and first.parent == downloads
    assert first.name == "ssh authorized_keys"
    assert keys.read_text() == "ssh-ed25519 OWNER"

    # Same name again: a second file, the first one untouched.
    second = asyncio.run(sm.save_download(FakeDownload("../../.ssh/authorized_keys", b"second"), downloads))
    assert second is not None and second != first
    assert first.read_bytes() == b"attacker key" and second.read_bytes() == b"second"

    # No dotfiles, and a long name keeps its extension.
    hidden = asyncio.run(sm.save_download(FakeDownload(".bashrc", b"x"), downloads))
    assert hidden is not None and hidden.name == "bashrc"
    long = asyncio.run(sm.save_download(FakeDownload("a" * 300 + ".pdf", b"%PDF"), downloads))
    assert long is not None and long.suffix == ".pdf" and len(long.name) <= 120
