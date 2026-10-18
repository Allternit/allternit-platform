"""com_init: a COM mode clash must not turn UI Automation off (Windows)."""

import sys
import types

import pytest

from allternit_driver import com_init


def _fake_comtypes(monkeypatch, error):
    mod = types.ModuleType("comtypes")
    mod.COINIT_MULTITHREADED = 0
    calls = []

    def co_init_ex(flags):
        calls.append(("ex", flags))
        if error is not None:
            raise error

    def co_init():
        calls.append(("sta", None))
        if error is not None:
            raise error

    mod.CoInitializeEx = co_init_ex
    mod.CoInitialize = co_init
    monkeypatch.setitem(sys.modules, "comtypes", mod)
    return calls


def _changed_mode():
    err = OSError(com_init.RPC_E_CHANGED_MODE, "Cannot change thread mode after it is set")
    err.winerror = com_init.RPC_E_CHANGED_MODE
    return err


def test_mode_clash_is_tolerated(monkeypatch):
    calls = _fake_comtypes(monkeypatch, _changed_mode())
    com_init.init_com()
    com_init.init_com(multithreaded=False)
    assert calls == [("ex", 0), ("sta", None)]


def test_other_com_errors_still_raise(monkeypatch):
    err = OSError(-2147024891, "Access is denied")
    err.winerror = -2147024891
    _fake_comtypes(monkeypatch, err)
    with pytest.raises(OSError):
        com_init.init_com()


def test_sets_multithreaded_flag_before_first_import(monkeypatch):
    monkeypatch.delitem(sys.modules, "comtypes", raising=False)
    monkeypatch.delattr(sys, "coinit_flags", raising=False)
    seen = {}

    class Finder:
        def find_spec(self, name, path=None, target=None):
            if name == "comtypes":
                seen["flags"] = getattr(sys, "coinit_flags", None)
                raise ImportError("stop")
            return None

    monkeypatch.setattr(sys, "meta_path", [Finder(), *sys.meta_path])
    with pytest.raises(ImportError):
        com_init.init_com()
    assert seen["flags"] == com_init.COINIT_MULTITHREADED
