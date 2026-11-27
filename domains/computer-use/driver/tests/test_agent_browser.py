"""agent_browser: profile paths and browser arguments (pure, no Windows needed)."""

import pytest

from allternit_driver import agent_browser as ab


def test_profile_dir_under_programdata(monkeypatch):
    monkeypatch.delenv("ALLTERNIT_DATA_DIR", raising=False)
    assert ab.profile_dir("bot-1") == r"C:\ProgramData\Allternit\browser-profiles\bot-1"
    assert ab.state_file("bot-1") == r"C:\ProgramData\Allternit\browser-state\bot-1.json"


def test_data_dir_override(monkeypatch):
    monkeypatch.setenv("ALLTERNIT_DATA_DIR", r"D:\x")
    assert ab.profile_dir("a.b_c") == r"D:\x\browser-profiles\a.b_c"


@pytest.mark.parametrize("bad", ["", "..", "a/b", "a\\b", "../x", "a..b", "-x", "x" * 65, "a b"])
def test_bad_agent_ids(bad):
    with pytest.raises(ab.LauncherError):
        ab.profile_dir(bad)


def test_build_args():
    args = ab.build_args(r"C:\p\a", 9333)
    assert args[0] == r"--user-data-dir=C:\p\a"
    assert "--remote-debugging-port=9333" in args
    assert "--remote-debugging-address=127.0.0.1" in args
    assert "--force-renderer-accessibility" in args
    assert not any("headless" in a for a in args)
