"""Element map + router logic (no OS access). Run from domains/computer-use/driver:
python -m unittest discover -s tests"""

import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from allternit_driver import element_map as em  # noqa: E402
from allternit_driver.router import ARC, CUA, EXPLORE_EVERY, Router  # noqa: E402


def calc(display="0", extra_row=False):
    nodes = [
        em.RawNode("w", "AXWindow", "Calculator", bounds=(100, 100, 200, 300)),
        em.RawNode("d", "AXStaticText", "main display", value=display, parent="w", bounds=(110, 110, 180, 40)),
    ]
    if extra_row:
        nodes.append(em.RawNode("x", "AXButton", "AC", parent="w", bounds=(110, 160, 40, 40)))
    nodes += [
        em.RawNode("b7", "AXButton", "7", parent="w", bounds=(110, 200, 40, 40)),
        em.RawNode("t1", "AXButton", "", parent="w", bounds=(160, 200, 40, 40)),
        em.RawNode("t2", "AXButton", "", parent="w", bounds=(210, 200, 40, 40)),
    ]
    return nodes


class ElementMapTest(unittest.TestCase):
    def test_ids_are_stable_across_reads_moves_and_insertions(self):
        a = {e.name or e.path: e.id for e in em.build(calc())}
        moved = [em.RawNode(n.key, n.role, n.name, n.value, (n.bounds[0] + 50, n.bounds[1] + 50, *n.bounds[2:]), n.parent) for n in calc()]
        b = {e.name or e.path: e.id for e in em.build(moved, (150, 150))}
        c = {e.name or e.path: e.id for e in em.build(calc(extra_row=True))}
        self.assertEqual(a["7"], b["7"])
        self.assertEqual(a["7"], c["7"])  # A new sibling with another name shifts nothing.
        ids = [e.id for e in em.build(calc())]
        self.assertEqual(len(ids), len(set(ids)))  # Unnamed twins still get distinct ids.

    def test_versions_bump_only_on_visible_change_and_diff(self):
        maps = em.ElementMaps()
        v1, new1 = maps.record("1:1", em.build(calc("0")), ARC)
        v1b, new1b = maps.record("1:1", em.build(calc("0")), ARC)
        self.assertTrue(new1)
        self.assertFalse(new1b)
        self.assertEqual(v1.number, v1b.number)
        v2, _ = maps.record("1:1", em.build(calc("7", extra_row=True)), ARC)
        self.assertEqual(v2.number, v1.number + 1)
        diff = maps.window("1:1").diff(v1.number)
        self.assertEqual([e["value"] for e in diff["changed"]], ["7"])
        self.assertEqual([e["name"] for e in diff["added"]], ["AC"])
        self.assertEqual(diff["removed"], [])
        self.assertEqual(maps.window_of(diff["added"][0]["id"]), "1:1")

    def test_stale_versions_are_refused(self):
        wm = em.WindowMap("1:1")
        wm.update(em.build(calc("0")), ARC)
        wm.update(em.build(calc("7")), ARC)
        self.assertEqual(wm.check(2).number, 2)
        self.assertEqual(wm.check(None).number, 2)
        with self.assertRaises(em.StaleVersion) as ctx:
            wm.check(1)
        self.assertEqual(ctx.exception.current, 2)

    def test_old_versions_age_out_of_diffs(self):
        wm = em.WindowMap("1:1")
        for i in range(em.HISTORY + 2):
            wm.update(em.build(calc(str(i))), ARC)
        self.assertIsNone(wm.diff(1))
        self.assertIsNotNone(wm.diff(wm.current.number - 1))

    def test_query_keeps_ancestors(self):
        kept, total = em.select(em.build(calc()), "main", None)
        self.assertEqual(total, 5)
        self.assertEqual([e.role for e in kept], ["Window", "StaticText"])


class RouterTest(unittest.TestCase):
    def test_defaults_and_os(self):
        mac, linux = Router("darwin"), Router("linux")
        self.assertEqual(mac.choose("read", "calc", {ARC, CUA}).engine, ARC)
        self.assertEqual(mac.choose("verify", "calc", {ARC, CUA}).engine, CUA)
        self.assertEqual(mac.choose("zoom", "calc", {CUA}).engine, CUA)
        self.assertEqual(mac.choose("batch", "calc", {ARC}).reason, "capability")
        self.assertEqual(linux.choose("read", "gedit", {CUA}).engine, CUA)
        self.assertEqual(mac.choose("read", "calc", set()).reason, "unavailable")

    def test_failures_then_latency_move_an_op(self):
        r = Router("darwin")
        d = r.choose("read", "obsidian", {ARC, CUA})
        for _ in range(3):
            r.record("read", "obsidian", d, 30, False)
        moved = r.choose("read", "obsidian", {ARC, CUA})
        self.assertEqual((moved.engine, moved.reason), (CUA, "failures"))

        r = Router("darwin")
        for _ in range(5):
            r.record("verify", "calc", r.choose("verify", "calc", {ARC, CUA}).__class__(CUA, "default"), 1000, True)
            r.record("verify", "calc", r.choose("verify", "calc", {ARC, CUA}).__class__(ARC, "explore"), 40, True)
        d = r.choose("verify", "calc", {ARC, CUA})
        self.assertEqual((d.engine, d.reason), (ARC, "latency"))

    def test_exploration_is_read_only_and_periodic(self):
        r = Router("darwin")
        picks = [r.choose("screenshot", "calc", {ARC, CUA}) for _ in range(EXPLORE_EVERY)]
        self.assertEqual(picks[-1].reason, "explore")
        self.assertEqual(sum(p.reason == "explore" for p in picks), 1)
        self.assertFalse(any(r.choose("menu", "calc", {ARC, CUA}).reason == "explore" for _ in range(EXPLORE_EVERY * 2)))

    def test_table_persists_per_user_and_degraded_flag(self):
        with tempfile.TemporaryDirectory() as tmp:
            r = Router("darwin", tmp)
            d = r.choose("read", "calc", {ARC, CUA})
            r.record("read", "calc", d, 40, True)
            r.mark_degraded("figma", "empty_tree")
            r.save()
            again = Router("darwin", tmp)
            table = again.table()
            self.assertEqual(table["apps"]["calc"]["read"]["arc"]["calls"], 1)
            self.assertEqual(table["degraded"], {"figma": "empty_tree"})
            self.assertEqual(Router("linux", tmp).table()["apps"], {})  # Another OS starts fresh.


if __name__ == "__main__":
    unittest.main()
