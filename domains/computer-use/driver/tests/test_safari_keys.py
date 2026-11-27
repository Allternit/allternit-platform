"""Safari key queue gating (arc_cua.backends.macos_safari): keys are sent only
while the agent's window is Safari's key window and the person is not using
Safari, and the previous key window is handed back. Fakes only: nothing here
touches Safari or posts an event."""

import os
import sys
import threading
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "arc"))

from arc_cua.backends import macos_safari as ms  # noqa: E402

AGENT, USER, OTHER_AGENT, PID = 101, 202, 303, 42


class Clock:
    def __init__(self):
        self.t = 1000.0

    def __call__(self):
        return self.t

    def sleep(self, s):
        self.t += s


class FakeSafari:
    """Safari's key window and front state, changed by make_key like AXMain would."""

    def __init__(self, key=USER, front=True, refuse_key=False):
        self.key = key
        self.front = front
        self.refuse_key = refuse_key
        self.made_key = []

    def front_pid(self):
        return PID if self.front else 7

    def key_window(self, pid):
        return self.key

    def make_key(self, pid, window):
        self.made_key.append(window)
        if not self.refuse_key:
            self.key = window
        return not self.refuse_key


def queue(safari, clock, idle=10.0, hold_s=2.0):
    presence = ms.Presence(probe=lambda: idle, clock=clock)
    return ms.SafariKeys(front_pid=safari.front_pid, key_window=safari.key_window, make_key=safari.make_key,
                         presence=presence, sleep=clock.sleep, clock=clock, hold_s=hold_s)


class MaySend(unittest.TestCase):
    def test_sends_when_agent_window_is_key_and_safari_is_not_front(self):
        self.assertEqual(ms.may_send(agent_window=AGENT, key_window=AGENT, safari_front=False, person_idle_s=0.0),
                         (True, ""))

    def test_sends_when_safari_is_front_but_person_idle_over_3s(self):
        ok, _ = ms.may_send(agent_window=AGENT, key_window=AGENT, safari_front=True, person_idle_s=3.5)
        self.assertTrue(ok)

    def test_holds_while_person_uses_safari(self):
        ok, why = ms.may_send(agent_window=AGENT, key_window=AGENT, safari_front=True, person_idle_s=1.0)
        self.assertFalse(ok)
        self.assertIn("person", why)

    def test_idle_exactly_3s_still_holds(self):
        ok, _ = ms.may_send(agent_window=AGENT, key_window=AGENT, safari_front=True, person_idle_s=3.0)
        self.assertFalse(ok)

    def test_never_while_another_window_is_key(self):
        for front, idle in ((False, 99.0), (True, 99.0)):
            ok, why = ms.may_send(agent_window=AGENT, key_window=USER, safari_front=front, person_idle_s=idle)
            self.assertFalse(ok)
            self.assertIn("another Safari window is key", why)

    def test_no_key_window_holds(self):
        ok, _ = ms.may_send(agent_window=AGENT, key_window=None, safari_front=False, person_idle_s=99.0)
        self.assertFalse(ok)


class PresenceClock(unittest.TestCase):
    def test_own_input_does_not_count_as_the_person(self):
        clock = Clock()
        hid = {"idle": 10.0}
        p = ms.Presence(probe=lambda: hid["idle"], clock=clock)
        self.assertAlmostEqual(p.person_idle_s(), 10.0)
        clock.sleep(1.0)
        p.own_input()  # Our key resets the HID clock...
        hid["idle"] = 0.0
        self.assertAlmostEqual(p.person_idle_s(), 11.0)  # ...but the person is still 11 s idle.

    def test_person_input_after_ours_counts(self):
        clock = Clock()
        hid = {"idle": 10.0}
        p = ms.Presence(probe=lambda: hid["idle"], clock=clock)
        p.person_idle_s()
        p.own_input()
        clock.sleep(2.0)
        hid["idle"] = 0.5  # An event 1.5 s after ours: the person.
        self.assertAlmostEqual(p.person_idle_s(), 0.5)

    def test_unknown_person_before_our_input_is_treated_as_present(self):
        clock = Clock()
        p = ms.Presence(probe=lambda: 0.0, clock=clock)
        p.own_input()
        self.assertEqual(p.person_idle_s(), 0.0)


class Queue(unittest.TestCase):
    def test_sends_makes_key_then_hands_back(self):
        safari, clock = FakeSafari(key=USER, front=False), Clock()
        sent = []
        queue(safari, clock).send(PID, AGENT, [lambda: sent.append(safari.key)])
        self.assertEqual(sent, [AGENT])  # Keys went out only while the agent window was key.
        self.assertEqual(safari.made_key, [AGENT, USER])
        self.assertEqual(safari.key, USER)

    def test_holds_while_person_types_in_safari_then_gives_up(self):
        safari, clock = FakeSafari(key=USER, front=True), Clock()
        sent = []
        with self.assertRaises(ms.SafariKeysHeld):
            queue(safari, clock, idle=0.2).send(PID, AGENT, [lambda: sent.append(1)])
        self.assertEqual(sent, [])
        self.assertEqual(safari.made_key, [])  # The person's window was never taken.
        self.assertEqual(safari.key, USER)

    def test_never_sends_when_window_cannot_become_key(self):
        safari, clock = FakeSafari(key=USER, front=False, refuse_key=True), Clock()
        sent = []
        with self.assertRaises(ms.SafariKeysHeld):
            queue(safari, clock).send(PID, AGENT, [lambda: sent.append(1)])
        self.assertEqual(sent, [])

    def test_hands_focus_back_when_person_returns_mid_run(self):
        safari, clock = FakeSafari(key=USER, front=False), Clock()
        sent = []

        def first():
            sent.append(safari.key)
            safari.front = True  # The person clicks back into Safari...
            q.presence = ms.Presence(probe=lambda: 0.0, clock=clock)  # ...and types.

        q = queue(safari, clock)
        with self.assertRaises(ms.SafariKeysHeld):
            q.send(PID, AGENT, [first, lambda: sent.append(safari.key)])
        self.assertEqual(sent, [AGENT])  # The second batch never went out.
        self.assertEqual(safari.key, USER)  # And the person's window was handed back.

    def test_one_sender_per_safari_process(self):
        safari, clock = FakeSafari(key=USER, front=False), Clock()
        q = queue(safari, clock, hold_s=0.2)
        inside, release = threading.Event(), threading.Event()

        def slow():
            inside.set()
            release.wait(2)

        t = threading.Thread(target=lambda: q.send(PID, AGENT, [slow]))
        t.start()
        inside.wait(2)
        sent = []
        with self.assertRaises(ms.SafariKeysHeld):  # A second agent waits its turn, then gives up.
            q.send(PID, OTHER_AGENT, [lambda: sent.append(1)])
        release.set()
        t.join(2)
        self.assertEqual(sent, [])

    def test_agent_window_already_key_is_not_reassigned(self):
        safari, clock = FakeSafari(key=AGENT, front=False), Clock()
        sent = []
        queue(safari, clock).send(PID, AGENT, [lambda: sent.append(1)])
        self.assertEqual(sent, [1])
        self.assertEqual(safari.made_key, [])


if __name__ == "__main__":
    unittest.main()
