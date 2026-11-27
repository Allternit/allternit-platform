"""The per-agent display allocator: next free display and the memory cap.
Run from domains/computer-use/driver: python -m unittest discover -s tests"""

import os
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from allternit_driver import agent_display as ad  # noqa: E402


class NextFreeDisplayTest(unittest.TestCase):
    def test_starts_at_ten_and_skips_agents_and_live_x_servers(self):
        self.assertEqual(ad.next_free_display(set(), lambda n: False), 10)
        # :10 held by an agent, :11 by some other X server (socket or lock).
        self.assertEqual(ad.next_free_display({10}, lambda n: n == 11), 12)

    def test_reuses_a_gap_left_by_a_stopped_agent(self):
        self.assertEqual(ad.next_free_display({11, 12}, lambda n: False), 10)

    def test_refuses_when_every_display_is_taken(self):
        with self.assertRaises(ad.DisplayError) as e:
            ad.next_free_display(set(range(10, 100)), lambda n: False)
        self.assertEqual(e.exception.code, "display_limit")


class DisplayCapTest(unittest.TestCase):
    def test_four_gb_computer_holds_six_agent_browsers(self):
        # (4096 - 1536 reserve) // 400 = 6, with plenty free right now.
        self.assertEqual(ad.display_cap(4096, 3500, running=0), 6)

    def test_free_memory_limits_new_displays(self):
        # Room for 6 by total, but only 900 MB free: 2 running + 2 more.
        self.assertEqual(ad.display_cap(4096, 900, running=2), 4)

    def test_small_computer_gets_none(self):
        self.assertEqual(ad.display_cap(1800, 1500, running=0), 0)

    def test_hard_max_wins(self):
        self.assertEqual(ad.display_cap(16384, 16000, running=0, hard_max=3), 3)

    def test_meminfo_parses_kb_to_mb(self):
        mem = ad.meminfo_mb("MemTotal:        4194304 kB\nMemAvailable:    2097152 kB\nHugePages_Total:       0\n")
        self.assertEqual(mem["MemTotal"], 4096)
        self.assertEqual(mem["MemAvailable"], 2048)


class StartRefusalTest(unittest.TestCase):
    def test_start_past_the_cap_refuses_with_a_clear_error(self):
        with tempfile.TemporaryDirectory() as state, tempfile.TemporaryDirectory() as profiles, \
                mock.patch.dict(os.environ, {"ALLTERNIT_AGENT_DISPLAY_MAX": "0"}):
            with self.assertRaises(ad.DisplayError) as e:
                ad.Displays(state, profiles).start("agent-1")
            self.assertEqual(e.exception.code, "display_limit")
            self.assertIn("agent browser", str(e.exception))
            self.assertEqual(e.exception.as_json()["running"], 0)
            self.assertEqual(os.listdir(profiles), [])  # Nothing started, no profile made.

    def test_agent_id_cannot_escape_the_profiles_dir(self):
        for bad in ("../root", "a/b", "", ".hidden", "-x"):
            with self.assertRaises(ad.DisplayError):
                ad.check_agent_id(bad)
        self.assertEqual(ad.check_agent_id("bot_7.run-2"), "bot_7.run-2")


if __name__ == "__main__":
    unittest.main()
