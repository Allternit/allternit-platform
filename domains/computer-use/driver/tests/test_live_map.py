"""Live element map: freshness rules, lifecycle and observer error handling,
with a fake observer (no OS accessibility needed)."""

import os
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from allternit_driver import live as lv  # noqa: E402
from allternit_driver.live.atspi import kind_of  # noqa: E402


class FakeObserver(lv.Observer):
    name = "fake"

    def __init__(self, degraded=None):
        self.degraded = degraded
        self.notify = {}
        self.sig = (1, 1)
        self.gone = None
        self.unsubscribed = []

    def subscribe(self, pid, window_id, notify):
        self.notify[(pid, window_id)] = notify
        return self.degraded

    def unsubscribe(self, pid, window_id):
        self.unsubscribed.append((pid, window_id))

    def probe(self, pid, window_id):
        if self.gone:
            raise self.gone
        return self.sig


class LiveMapTest(unittest.TestCase):
    def setUp(self):
        self.obs = FakeObserver()
        self.refreshed, self.forgot = [], []
        self.live = lv.LiveMaps(self.obs, lambda key, full: self._refresh(key, full), self.forgot.append)

    def tearDown(self):
        self.live.close()

    def _refresh(self, key, full):
        full = self.live.begin(key) or full
        self.refreshed.append((key, full))
        self.live.done(key, full)

    def _watch(self, key="1:10", pid=1, wid=10):
        self.live.ensure(key, pid, wid)
        self._refresh(key, True)  # The driver's first walk.

    def test_clean_window_is_served_from_the_map(self):
        self._watch()
        self.assertIsNone(self.live.serve("1:10"))

    def test_notifications_choose_patch_or_full_walk(self):
        self._watch()
        self.live._closed = True  # Keep the worker out: the reader decides here.
        self.obs.notify[(1, 10)](lv.ELEMENT)
        self.assertEqual(self.live.serve("1:10"), "patch")
        self.assertFalse(self.live.begin("1:10"))
        self.live.done("1:10", False)
        self.assertIsNone(self.live.serve("1:10"))
        self.obs.notify[(1, 10)](lv.WINDOW)
        self.assertEqual(self.live.serve("1:10"), "full")
        self.live.begin("1:10")
        self.live.done("1:10", True)
        for _ in range(lv.LARGE_CHANGE + 1):
            self.obs.notify[(1, 10)](lv.STRUCTURE)
        self.assertEqual(self.live.serve("1:10"), "full")

    def test_worker_patches_a_window_a_caller_reads(self):
        self._watch()
        self.obs.notify[(1, 10)](lv.ELEMENT)
        deadline = time.monotonic() + 2
        while ("1:10", False) not in self.refreshed and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertIn(("1:10", False), self.refreshed)
        self.assertIsNone(self.live.serve("1:10"))

    def test_prewarmed_window_is_patched_only_on_read(self):
        self.live.ensure("1:10", 1, 10, asked=False)
        self._refresh("1:10", True)
        self.obs.notify[(1, 10)](lv.ELEMENT)
        time.sleep(0.2)
        self.assertEqual(self.refreshed, [("1:10", True)])
        self.assertEqual(self.live.serve("1:10"), "patch")

    def test_degraded_window_reads_on_a_timer(self):
        self.obs.degraded = "no notifications"
        self._watch()
        self.assertEqual(self.live.state("1:10"), {"state": "degraded", "observer": "fake", "reason": "no notifications"})
        self.assertIsNone(self.live.serve("1:10"))
        self.live._watches["1:10"].refreshed -= lv.DEGRADED_TTL_S + 0.1
        self.assertEqual(self.live.serve("1:10"), "full")

    def test_unannounced_changes_degrade_the_window(self):
        self._watch()
        w = self.live.get("1:10")
        for _ in range(lv.MISSES_TO_DEGRADE):
            self.obs.sig = (self.obs.sig[0] + 1, 1)
            w.probed = 0
            self.live._tend(w, time.monotonic())
        self.assertTrue(w.degraded)
        self.assertEqual(self.refreshed[-1], ("1:10", True))

    def test_cap_evicts_least_recently_used(self):
        for i in range(lv.MAX_WINDOWS + 1):
            self.live.ensure(f"1:{i}", 1, i)
            time.sleep(0.001)
        self.assertIsNone(self.live.get("1:0"))
        self.assertEqual(self.forgot, ["1:0"])
        self.assertIn((1, 0), self.obs.unsubscribed)

    def test_window_gone_drops_one_window_app_gone_drops_all(self):
        self._watch("1:10", 1, 10)
        self._watch("1:11", 1, 11)
        self.obs.gone = lv.WindowGone("closed")
        self.live.get("1:10").probed = 0
        try:
            self.live._tend(self.live.get("1:10"), time.monotonic())
        except lv.WindowGone:
            self.live.drop("1:10")
        self.assertIsNone(self.live.get("1:10"))
        self.assertIsNotNone(self.live.get("1:11"))
        self.live.drop_pid(1, "quit")
        self.assertIsNone(self.live.get("1:11"))

    def test_observer_failure_degrades_instead_of_raising(self):
        class Broken(FakeObserver):
            def subscribe(self, pid, window_id, notify):
                raise OSError("boom")

        live = lv.LiveMaps(Broken(), lambda k, f: None, lambda k: None)
        try:
            self.assertIn("boom", live.ensure("2:1", 2, 1).degraded)
        finally:
            live.close()


class ATSPIKindTest(unittest.TestCase):
    def test_signal_names_map_to_kinds(self):
        self.assertEqual(kind_of("org.a11y.atspi.Event.Object", "ChildrenChanged"), lv.STRUCTURE)
        self.assertEqual(kind_of("org.a11y.atspi.Event.Object", "TextChanged"), lv.ELEMENT)
        self.assertEqual(kind_of("org.a11y.atspi.Event.Object", "PropertyChange"), lv.ELEMENT)
        self.assertEqual(kind_of("org.a11y.atspi.Event.Window", "Destroy"), lv.GONE)
        self.assertEqual(kind_of("org.a11y.atspi.Event.Focus", "Focus"), lv.ELEMENT)


if __name__ == "__main__":
    unittest.main()
