"""Vision fallback (D6): the AX/vision switch, mark -> element mapping, the
grounder's answer parsing and zoom-reground math. No model, no screen."""

import os
import struct
import sys
import tempfile
import unittest
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "arc"))

from allternit_driver import element_map as em  # noqa: E402
from allternit_driver.router import Router, assess_tree  # noqa: E402
from allternit_driver.vision import Shot, build, marks_to_nodes  # noqa: E402
from allternit_driver.vision import geometry as g  # noqa: E402
from allternit_driver.vision import grounder as gr  # noqa: E402
from allternit_driver.vision import marks as mk  # noqa: E402


def png(w: int, h: int) -> bytes:
    """A real (blank) PNG of w x h: enough for png_size and Shot."""
    raw = b"".join(b"\x00" + b"\x00\x00\x00" * w for _ in range(h))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)  # noqa: E731
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


def elements(*specs):
    """specs: (key, role, bounds, parent, actions) -> element-map elements."""
    nodes = [em.RawNode(key=k, role=r, bounds=b, parent=p, actions=a) for k, r, b, p, a in specs]
    return em.build(nodes, nodes[0].bounds[:2] if nodes and nodes[0].bounds else (0, 0))


class GeometryTest(unittest.TestCase):
    def test_zoom_box_centres_clamps_and_keeps_a_minimum(self):
        self.assertEqual(g.zoom_box(2000, 1200, (1000, 600)), (500, 300, 1500, 900))
        self.assertEqual(g.zoom_box(2000, 1200, (1990, 10)), (1000, 0, 2000, 600))  # Shifted inside.
        self.assertEqual(g.zoom_box(600, 400, (300, 200)), (76, 0, 524, 400))  # min_side 448, capped at the image.

    def test_norm_point_maps_through_the_crop(self):
        crop = (500, 300, 1500, 900)
        self.assertEqual(g.norm_to_px(500, 500, crop), (1000, 600))
        self.assertEqual(g.norm_to_px(0, 1000, crop), (500, 900))
        self.assertEqual(g.norm_to_px(250, 250, (0, 0, 1280, 800)), (320, 200))

    def test_fit_scale_and_agreement(self):
        self.assertEqual(g.fit_scale(1000, 1000, 2_000_000), 1.0)
        self.assertAlmostEqual(g.fit_scale(2000, 2000, 1_000_000), 0.5)
        self.assertTrue(g.agree((100, 100), (110, 100), 1280, 800))
        self.assertFalse(g.agree((100, 100), (200, 100), 1280, 800))

    def test_snap_picks_the_smallest_containing_box(self):
        boxes = [(0, 0, 500, 500), (100, 100, 200, 140), (90, 90, 300, 300)]
        self.assertEqual(g.snap((150, 120), boxes), 1)
        self.assertEqual(g.snap((250, 250), boxes), 2)
        self.assertIsNone(g.snap((900, 900), boxes))

    def test_desktop_point_and_screen_box(self):
        # Window at (100, 50) pt on a 2x display: image px (40, 20) -> desktop px (240, 120).
        self.assertEqual(g.px_to_desktop((40, 20), (100, 50), 2.0), (240, 120))
        self.assertEqual(g.px_to_screen((40, 20, 140, 60), (100, 50), 2.0), (120, 60, 50, 20))

    def test_hamming(self):
        self.assertEqual(g.hamming("ff", "fe"), 1)
        self.assertGreater(g.hamming("ff", "f"), 1000)


class ParseTest(unittest.TestCase):
    def test_venus_formats(self):
        self.assertEqual(gr.parse("venus15", "[903, 944]"), ("positive", (903.0, 944.0)))
        self.assertEqual(gr.parse("venus15", "[100,200,300,400]"), ("positive", (200.0, 300.0)))
        self.assertEqual(gr.parse("venus15", "[-1, -1]"), ("infeasible", None))
        self.assertEqual(gr.parse("venus15", "I can't see it")[0], "wrong_format")

    def test_holo2_json_after_thinking(self):
        self.assertEqual(gr.parse("holo2", '<think>the button is low right</think>{"x": 812, "y": 77}'), ("positive", (812.0, 77.0)))

    def test_prompts_follow_each_model(self):
        self.assertIn("in the format [x,y]", gr.prompt("venus15", "Export button."))
        # The authors strip a trailing period; their template adds one back.
        self.assertIn("instruction: \nExport button. \n\n", gr.prompt("venus15", "Export button."))
        self.assertIn("ClickCoordinates", gr.prompt("holo2", "Export"))
        self.assertTrue(gr.prompt("holo2", "Export").endswith("Export"))


class FakeImage:
    def __init__(self, w, h, box=None):
        self.size = (w, h)
        self.box = box

    def crop(self, box):
        return FakeImage(box[2] - box[0], box[3] - box[1], box)

    def resize(self, wh):
        return FakeImage(wh[0], wh[1], self.box)


class GroundPolicyTest(unittest.TestCase):
    def runner(self, answers):
        calls = []

        def run(model, img, text):
            calls.append((model, img.size, img.box))
            return answers.pop(0)

        return run, calls

    def test_sharp_window_on_a_known_element_needs_no_zoom(self):
        run, calls = self.runner(["[500, 500]"])
        res = gr.ground(run, FakeImage(1000, 800), "OK", boxes=[(480, 380, 520, 420)])
        self.assertEqual((res.status, res.confidence, res.zoomed, res.snapped), ("positive", "high", False, 0))
        self.assertEqual(res.point, (500, 400))
        self.assertEqual(len(calls), 1)

    def test_downscaled_window_zooms_and_maps_the_crop_point_back(self):
        # 3000x2000 > MAX_PIXELS: pass 1 is downscaled, so the crop is regrounded.
        run, calls = self.runner(["[500, 500]", "[520, 500]"])
        res = gr.ground(run, FakeImage(3000, 2000), "tiny icon")
        self.assertTrue(res.zoomed)
        crop = g.zoom_box(3000, 2000, (1500, 1000))
        self.assertEqual(calls[1][2], tuple(int(round(v)) for v in crop))
        self.assertEqual(res.point, g.norm_to_px(520, 500, crop))
        self.assertEqual(res.confidence, "high")  # 30 px apart on a 3600 px diagonal: agree.
        self.assertLess(calls[0][1][0] * calls[0][1][1], gr.MAX_PIXELS + 3000)

    def test_point_off_every_element_zooms_and_disagreement_is_medium(self):
        run, _ = self.runner(["[100, 100]", "[900, 900]"])
        res = gr.ground(run, FakeImage(1000, 1000), "x", boxes=[(800, 800, 820, 820)])
        self.assertTrue(res.zoomed)
        self.assertEqual(res.confidence, "medium")

    def test_infeasible_primary_hands_over_to_the_fallback_model(self):
        run, calls = self.runner(["[-1, -1]", '{"x": 250, "y": 250}'])
        res = gr.ground(run, FakeImage(800, 800), "x", zoom="never")
        self.assertEqual([c[0] for c in calls], ["ui-venus-1.5-8b", "holo2-8b"])
        self.assertEqual((res.status, res.model, res.point), ("positive", "holo2-8b", (200, 200)))

    def test_crop_that_loses_the_target_keeps_pass_one_as_low(self):
        run, _ = self.runner(["[500, 500]", "[-1, -1]"])
        res = gr.ground(run, FakeImage(1000, 1000), "x", zoom="always")
        self.assertEqual((res.point, res.confidence, res.zoomed), ((500, 500), "low", True))

    def test_unavailable_models_are_skipped(self):
        run, _ = self.runner([])
        res = gr.ground(run, FakeImage(10, 10), "x", available=lambda m: False)
        self.assertEqual(res.status, "unavailable")


class SwitchTest(unittest.TestCase):
    WIN = ("w", "AXWindow", (0, 0, 1000, 800), None, ())

    def test_empty_and_sparse_trees_go_to_vision(self):
        self.assertEqual(assess_tree(elements(self.WIN)).reason, "empty_tree")
        self.assertEqual(assess_tree([]).source, "vision")
        sparse = elements(self.WIN, ("t", "AXStaticText", (0, 0, 100, 20), "w", ()))
        self.assertEqual((assess_tree(sparse).source, assess_tree(sparse).reason), ("vision", "sparse_tree"))
        self.assertEqual(assess_tree(elements(self.WIN, ("b", "AXButton", (0, 0, 9, 9), "w", ("press",))), "empty_tree").source, "vision")

    def test_a_canvas_the_tree_cant_see_into_is_hybrid(self):
        tree = elements(self.WIN, ("b1", "AXButton", (0, 0, 50, 20), "w", ("press",)),
                        ("b2", "AXButton", (60, 0, 50, 20), "w", ("press",)),
                        ("c", "AXGroup", (0, 40, 1000, 700), "w", ()))
        d = assess_tree(tree)
        self.assertEqual((d.source, d.reason, d.regions), ("hybrid", "opaque_region", ((0, 40, 1000, 700),)))

    def test_a_real_tree_stays_ax(self):
        tree = elements(self.WIN, ("b1", "AXButton", (0, 0, 50, 20), "w", ("press",)),
                        ("g", "AXGroup", (0, 40, 1000, 700), "w", ()), ("b2", "AXButton", (10, 50, 50, 20), "g", ("press",)))
        self.assertEqual(assess_tree(tree).source, "ax")

    def test_modes_errors_and_audit_record_per_app(self):
        with tempfile.TemporaryDirectory() as d:
            from allternit_driver.router import Audit

            r = Router("darwin", d, Audit(d))
            self.assertEqual(r.choose_source("app", [], "off").source, "ax")
            self.assertEqual(r.choose_source("app", [], "only").reason, "asked")
            self.assertEqual(r.choose_source("app", [], "auto", ax_error="no window").reason, "ax_error")
            self.assertEqual(r.table()["sources"]["app"]["reason"], "ax_error")
            with open(os.path.join(d, "audit.jsonl")) as f:
                rows = [line for line in f if '"op":"source"' in line]
            self.assertEqual(len(rows), 3)


class MarkMappingTest(unittest.TestCase):
    MARKS = [
        {"box": [20, 20, 220, 80], "kind": "control", "name": "Export", "hash": "f" * 64},
        {"box": [400, 400, 440, 440], "kind": "icon", "name": "", "hash": "f" * 64},
        {"box": [1000, 20, 1100, 60], "kind": "text", "name": "Title", "hash": "f" * 64},
    ]

    def test_marks_become_vision_elements_in_screen_points(self):
        shot = Shot(png(1200, 800), (100.0, 50.0), 2.0)
        nodes = marks_to_nodes(self.MARKS, shot)
        els = build(nodes, shot.origin)
        self.assertEqual([e.mark for e in els], [1, 2, 3])
        self.assertTrue(all(e.id.startswith("v") and e.source == "vision" for e in els))
        self.assertEqual(els[0].bounds, (110.0, 60.0, 100.0, 30.0))
        pub = els[0].public()
        self.assertEqual((pub["source"], pub["mark"], pub["name"], pub["role"]), ("vision", 1, "Export", "control"))
        # Stable: the same marks give the same ids.
        self.assertEqual([e.id for e in els], [e.id for e in build(marks_to_nodes(self.MARKS, shot), shot.origin)])
        # Tree ids keep their prefix and source stays out of their public form.
        tree = elements(("w", "AXWindow", (0, 0, 10, 10), None, ()))
        self.assertTrue(tree[0].id.startswith("e"))
        self.assertNotIn("source", tree[0].public())

    def test_hybrid_keeps_only_marks_in_the_opaque_region_and_drops_tree_twins(self):
        shot = Shot(png(1200, 800), (0.0, 0.0), 1.0)
        nodes = marks_to_nodes(self.MARKS, shot, regions=((300, 300, 500, 300),))
        self.assertEqual([n.name or n.role for n in nodes], ["icon"])
        nodes = marks_to_nodes(self.MARKS, shot, ax_boxes=[(20, 20, 220, 80)])
        self.assertEqual([n.name or n.role for n in nodes], ["icon", "Title"])

    def test_merge_names_controls_by_their_text_and_numbers_in_reading_order(self):
        texts = [mk.Region((40, 30, 100, 50), "text", "Export", 0.9, "ocr"), mk.Region((10, 300, 200, 320), "text", "Hello there", 0.9, "ocr")]
        shapes = [mk.Region((20, 20, 120, 60), "icon", "", 0.6), mk.Region((500, 25, 540, 55), "icon", "", 0.6),
                  mk.Region((0, 0, 900, 700), "icon", "", 0.4)]  # A panel around all text: not a control.
        out = mk.merge(texts, shapes)
        self.assertEqual([(r.kind, r.name) for r in out], [("control", "Export"), ("icon", ""), ("text", "Hello there")])


class FakeCua:
    available = True
    tools = set()
    error = None

    def __init__(self):
        self.calls = []

    def read(self, pid, window_id, max_elements=None):
        return [em.RawNode(key="0", role="AXWindow", bounds=(100, 50, 600, 400))], {"app": "Paint", "degraded": None}

    def windows(self, pid=None):
        return [{"pid": 7, "window_id": 9, "app_name": "Paint", "bounds": {"x": 100, "y": 50, "width": 600, "height": 400}}]

    def screenshot(self, pid, window_id):
        return png(1200, 800)

    def pixel(self, tool, args):
        self.calls.append((tool, args))
        return {}

    def start(self):
        pass

    def close(self):
        pass


class FakeWorker:
    def __init__(self, hash_now="f" * 64):
        self.hash_now = hash_now

    def status(self):
        return {"state": "ready"}

    def call(self, op, timeout=0, **kw):
        if op == "marks":
            return {"size": [1200, 800], "marks": MarkMappingTest.MARKS}
        if op == "hash":
            return {"hashes": [self.hash_now]}
        if op == "ground":
            return {"status": "positive", "point": [820, 520], "confidence": "medium", "zoomed": True, "model": "ui-venus-1.5-8b"}
        raise AssertionError(op)

    def close(self):
        pass


class ReadActTest(unittest.TestCase):
    """read_ui -> vision marks -> act on a vision id, through the real Driver
    with fake engines (no screen, no model)."""

    def driver(self, worker):
        from allternit_driver.core import Driver

        cua = FakeCua()
        d = Driver(cua, arc=type("NoArc", (), {"available": False, "error": None, "hub": None, "close": lambda s: None})(), os_name="linux")
        d.vision.worker = worker
        return d, cua

    def test_empty_tree_reads_through_vision_and_act_clicks_the_mark(self):
        d, cua = self.driver(FakeWorker())
        out = d.read_ui({"pid": 7, "target": "the red swatch"})
        self.assertEqual((out["source"], out["source_reason"], out["engine"], out["marks"]), ("vision", "empty_tree", "vision", 3))
        ids = [e["id"] for e in out["elements"]]
        self.assertTrue(all(i.startswith("v") for i in ids))
        # The grounded point hit no mark: it became a point element.
        g_ = out["grounded"]
        self.assertEqual((g_["source"], g_["point"]), ("vision", [510.0, 310.0]))
        res = d.act({"id": ids[0], "op": "click"})
        self.assertEqual(res["status"], "done")
        # Mark 1 centre (120, 50) px, window at (100, 50) pt, 2 px/pt -> desktop px (320, 150).
        self.assertEqual(cua.calls[-1], ("click", {"scope": "desktop", "x": 320.0, "y": 150.0}))
        res = d.act({"id": ids[1], "op": "set_value", "value": "hi"})
        self.assertEqual([c[0] for c in cua.calls[-3:]], ["click", "hotkey", "type_text"])
        self.assertEqual(d.router.table()["sources"]["Paint"]["source"], "vision")

    def test_changed_pixels_refuse_the_action_with_a_fresh_map(self):
        d, cua = self.driver(FakeWorker(hash_now="0" * 64))
        out = d.read_ui({"pid": 7})
        res = d.act({"id": out["elements"][0]["id"], "op": "click"})
        self.assertEqual(res["status"], "stale_version")
        self.assertIn("elements", res["map"])
        self.assertEqual(cua.calls, [])

    def test_vision_off_keeps_the_tree_answer(self):
        d, _ = self.driver(FakeWorker())
        out = d.read_ui({"pid": 7, "vision": "off"})
        self.assertEqual((out["source"], out["source_reason"]), ("ax", "off"))
        self.assertNotIn("marks", out)


if __name__ == "__main__":
    unittest.main()
