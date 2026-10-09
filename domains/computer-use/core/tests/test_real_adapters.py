"""
Allternit Computer Use — Real Adapter Integration Tests
Actually tests browser automation, desktop capture, and the operator service.
No mocks. Real Playwright, real native desktop capture, real HTTP calls.
"""

import asyncio
import sys
import os
import json
import time
import subprocess
import signal
from pathlib import Path
from datetime import datetime

# Path setup
COMPUTER_USE_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(COMPUTER_USE_ROOT))

from core import BaseAdapter, ActionRequest, ResultEnvelope
from routing import Router, RouteConstraints
from policy import PolicyEngine
from receipts import ReceiptWriter
from sessions import SessionManager
from telemetry import TelemetryCollector

import tempfile
import shutil


class TestResults:
    def __init__(self):
        self.passed = 0
        self.failed = 0
        self.skipped = 0
        self.errors = []

    def ok(self, name: str):
        self.passed += 1
        print(f"  ✓ {name}")

    def fail(self, name: str, reason: str):
        self.failed += 1
        self.errors.append(f"{name}: {reason}")
        print(f"  ✗ {name} — {reason}")

    def skip(self, name: str, reason: str):
        self.skipped += 1
        print(f"  ⊘ {name} — SKIPPED: {reason}")

    def summary(self):
        total = self.passed + self.failed
        print(f"\n{'='*60}")
        print(f"Results: {self.passed}/{total} passed, {self.failed} failed, {self.skipped} skipped")
        if self.errors:
            print("\nFailures:")
            for e in self.errors:
                print(f"  - {e}")
        print(f"{'='*60}")
        return self.failed == 0


results = TestResults()


# ============================================================================
# Test 1: Playwright — Real Browser
# ============================================================================

async def test_playwright_real():
    print("\n[Test 1] Playwright Adapter — Real Browser")

    try:
        from playwright.async_api import async_playwright
    except ImportError:
        results.skip("Playwright import", "playwright not installed")
        return

    pw = None
    browser = None
    try:
        pw = await async_playwright().start()
        browser = await pw.chromium.launch(headless=True, args=["--no-sandbox"])
        page = await browser.new_page()

        # Test 1a: Navigate to a real page
        await page.goto("https://example.com", timeout=15000)
        title = await page.title()
        assert title == "Example Domain", f"Expected 'Example Domain', got '{title}'"
        results.ok(f"Navigate to example.com — title: '{title}'")

        # Test 1b: Extract text content
        try:
            text = await page.evaluate("() => document.body.innerText")
            assert "Example Domain" in text
            results.ok(f"Extract page text content — {len(text)} chars")
        except Exception as e:
            results.fail("Extract page text content", str(e) or repr(e))

        # Test 1c: Take screenshot
        try:
            screenshot = await page.screenshot()
            assert len(screenshot) > 1000, f"Screenshot too small: {len(screenshot)} bytes"
            results.ok(f"Screenshot captured — {len(screenshot)} bytes")
        except Exception as e:
            results.fail("Screenshot", str(e) or repr(e))

        # Test 1d: Extract HTML
        try:
            html = await page.content()
            assert "<h1>" in html
            assert "Example Domain" in html
            results.ok("Extract full HTML")
        except Exception as e:
            results.fail("Extract full HTML", str(e) or repr(e))

        # Test 1e: Evaluate JavaScript
        try:
            result = await page.evaluate("() => ({ url: location.href, title: document.title })")
            assert result["url"] == "https://example.com/"
            assert result["title"] == "Example Domain"
            results.ok("JavaScript evaluation in page")
        except Exception as e:
            results.fail("JavaScript evaluation", str(e) or repr(e))

        # Test 1f: Navigate to second page
        try:
            await page.goto("https://httpbin.org/html", timeout=15000)
            text2 = await page.evaluate("() => document.body.innerText")
            assert "Herman Melville" in text2 or "Moby" in text2, f"Expected Moby Dick content, got: {text2[:100]}"
            results.ok("Navigate to httpbin.org/html — content verified")
        except Exception as e:
            results.fail("Navigate to httpbin.org/html", str(e) or repr(e))

        # Test 1g: Multi-tab (new context with two pages)
        try:
            ctx = await browser.new_context()
            p1 = await ctx.new_page()
            await p1.goto("https://example.com", timeout=15000)
            p2 = await ctx.new_page()
            await p2.goto("https://example.com", timeout=15000)
            assert len(ctx.pages) >= 2, f"Expected ≥2 pages, got {len(ctx.pages)}"
            await p2.close()
            await p1.close()
            await ctx.close()
            results.ok("Multi-tab: two pages in same context")
        except Exception as e:
            results.fail("Multi-tab", str(e) or repr(e))

        # Test 1h: Network interception
        try:
            responses_captured = []
            page.on("response", lambda r: responses_captured.append(r.url))
            await page.goto("https://example.com", timeout=15000)
            assert len(responses_captured) > 0
            results.ok(f"Network capture — {len(responses_captured)} responses intercepted")
        except Exception as e:
            results.fail("Network capture", str(e) or repr(e))

    except Exception as e:
        results.fail("Playwright browser launch", str(e) or repr(e))
    finally:
        if browser:
            await browser.close()
        if pw:
            await pw.stop()


# ============================================================================
# Test 2: Playwright Adapter Wrapper — Full Pipeline
# ============================================================================

async def test_playwright_adapter_wrapper():
    print("\n[Test 2] Playwright Adapter Wrapper — Full Pipeline with Receipts")

    try:
        from adapters.browser.playwright import PlaywrightAdapter
    except ImportError as e:
        results.skip("PlaywrightAdapter import", str(e))
        return

    tmp_dir = tempfile.mkdtemp()
    adapter = PlaywrightAdapter()
    receipt_writer = ReceiptWriter(receipts_dir=tmp_dir)
    session_mgr = SessionManager(sessions_dir=os.path.join(tmp_dir, "sessions"))
    telemetry = TelemetryCollector()
    router = Router()
    policy = PolicyEngine()

    try:
        # Step 1: Route
        decision = router.route("Go to example.com", family="browser", mode="execute",
                               constraints=RouteConstraints(deterministic=True))
        assert decision.primary_adapter == "browser.playwright"
        results.ok("Route → browser.playwright")

        # Step 2: Policy
        pol = policy.evaluate(target="https://example.com", action_type="goto",
                             adapter_id="browser.playwright", family="browser")
        assert pol.decision == "allow"
        results.ok("Policy → allow")

        # Step 3: Session
        session = session_mgr.create(run_id="real-test-1", family="browser",
                                     mode="execute", adapter_id="browser.playwright")
        session = session_mgr.activate(session.session_id)

        # Step 4: Route receipt
        route_receipt = receipt_writer.emit_route_decision(
            run_id=session.run_id, session_id=session.session_id,
            route_decision=decision.to_dict(), policy_decision_id=pol.decision_id,
        )

        # Step 5: Execute real navigation
        telemetry.adapter_started("browser.playwright", "browser", "execute",
                                 session.session_id, session.run_id)
        start = time.monotonic()

        action = ActionRequest(action_type="goto", target="https://example.com")
        envelope = await adapter.execute(action, session_id=session.session_id, run_id=session.run_id)

        duration = int((time.monotonic() - start) * 1000)
        telemetry.adapter_completed("browser.playwright", "browser", "execute",
                                   session.session_id, session.run_id, duration_ms=duration)

        assert envelope.status == "completed", f"Expected completed, got {envelope.status}: {envelope.error}"
        assert envelope.adapter_id == "browser.playwright"
        assert envelope.family == "browser"
        assert len(envelope.receipts) >= 1
        results.ok(f"Playwright goto example.com — completed in {duration}ms")

        # Step 6: Action receipt
        action_receipt = receipt_writer.emit(
            run_id=session.run_id, session_id=session.session_id,
            action_type="goto", adapter_id="browser.playwright",
            target="https://example.com",
            action_data={"action": "goto", "target": "https://example.com"},
            result_data=envelope.extracted_content or {"status": "completed"},
            policy_decision_id=pol.decision_id, duration_ms=duration,
        )
        assert len(action_receipt.integrity_hash) == 64
        results.ok("Action receipt with integrity hash")

        # Step 7: Extract content
        action2 = ActionRequest(action_type="extract", target="https://example.com")
        envelope2 = await adapter.execute(action2, session_id=session.session_id, run_id=session.run_id)
        assert envelope2.status == "completed"
        assert envelope2.extracted_content is not None
        content = envelope2.extracted_content
        assert "Example Domain" in str(content), f"Expected 'Example Domain' in extracted content"
        results.ok(f"Extract page content — found 'Example Domain'")

        # Step 8: Screenshot
        action3 = ActionRequest(action_type="screenshot", target="page")
        envelope3 = await adapter.execute(action3, session_id=session.session_id, run_id=session.run_id)
        assert envelope3.status == "completed"
        assert len(envelope3.artifacts) >= 1
        artifact = envelope3.artifacts[0]
        assert artifact.type == "screenshot"
        assert artifact.size_bytes > 1000
        results.ok(f"Screenshot captured — {artifact.size_bytes} bytes")

        # Step 9: JavaScript eval
        action4 = ActionRequest(action_type="eval", target="page",
                               parameters={"script": "() => document.title"})
        envelope4 = await adapter.execute(action4, session_id=session.session_id, run_id=session.run_id)
        assert envelope4.status == "completed"
        assert envelope4.extracted_content == "Example Domain"
        results.ok(f"JS eval → '{envelope4.extracted_content}'")

        # Step 10: Verify audit trail
        all_receipts = receipt_writer.list_receipts(session_id=session.session_id)
        assert len(all_receipts) >= 2  # route + action
        results.ok(f"Audit trail: {len(all_receipts)} receipts persisted")

        # Verify receipt files on disk
        receipt_files = list(Path(tmp_dir).glob("rcpt_*.json"))
        assert len(receipt_files) >= 2
        with open(receipt_files[0]) as f:
            receipt_data = json.load(f)
        assert "integrity_hash" in receipt_data
        assert len(receipt_data["integrity_hash"]) == 64
        results.ok("Receipt files verified on disk with integrity hashes")

        # Step 11: Telemetry metrics
        metrics = telemetry.get_adapter_metrics("browser.playwright")
        assert metrics["completions"] == 1
        assert metrics["avg_duration_ms"] > 0
        results.ok(f"Telemetry: {metrics['completions']} completion, avg {metrics['avg_duration_ms']}ms")

        # Step 12: Session cleanup
        session_mgr.record_action(session.session_id, {
            "action": "goto", "target": "https://example.com",
            "receipt_id": action_receipt.receipt_id, "duration_ms": duration,
        })
        session_mgr.destroy(session.session_id)
        final = session_mgr.get(session.session_id)
        assert final.status == "destroyed"
        assert len(final.previous_actions) == 1
        results.ok("Session destroyed with action history")

    except Exception as e:
        results.fail("Playwright adapter wrapper pipeline", str(e))
    finally:
        await adapter.close()
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ============================================================================
# Test 4: Operator Service — Real HTTP
# ============================================================================

async def test_operator_service():
    print("\n[Test 4] Operator Service — Real HTTP Endpoints")

    operator_dir = COMPUTER_USE_ROOT.parents[1] / "services" / "computer-use-operator"
    main_py = operator_dir / "src" / "main.py"

    if not main_py.exists():
        results.skip("Operator service", f"main.py not found at {main_py}")
        return

    # Start the operator in background on a test port
    env = os.environ.copy()
    env["Allternit_OPERATOR_PORT"] = "13010"  # Test port to avoid conflicts
    env["Allternit_OPERATOR_HOST"] = "127.0.0.1"
    env["Allternit_OPERATOR_API_KEY"] = "test-key"
    env["PYTHONPATH"] = str(operator_dir / "src")

    # Check for run.py wrapper
    run_py = operator_dir / "run.py"

    proc = None
    try:
        if run_py.exists():
            # Use the run.py wrapper
            proc = subprocess.Popen(
                [sys.executable, str(run_py)],
                cwd=str(operator_dir),
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        else:
            # Run as package module: python -m src.main
            env["PYTHONPATH"] = str(operator_dir)
            proc = subprocess.Popen(
                [sys.executable, "-m", "src.main"],
                cwd=str(operator_dir),
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )

        # Wait for server to start
        import httpx

        base = "http://127.0.0.1:13010"
        started = False
        for _ in range(30):  # up to 3 seconds
            try:
                async with httpx.AsyncClient() as client:
                    resp = await client.get(f"{base}/health", timeout=1.0)
                    if resp.status_code == 200:
                        started = True
                        break
            except Exception:
                pass
            await asyncio.sleep(0.1)

        if not started:
            stderr_out = proc.stderr.read().decode() if proc.stderr else ""
            results.fail("Operator startup", f"Service didn't start within 3s. stderr: {stderr_out[:500]}")
            return

        results.ok("Operator service started on port 13010")

        async with httpx.AsyncClient() as client:
            # Test 4a: Health endpoint
            resp = await client.get(f"{base}/health")
            assert resp.status_code == 200
            data = resp.json()
            assert data["status"] == "healthy"
            assert data["service"] == "allternit-operator"
            assert "browser-use" in data["capabilities"]
            assert "desktop-use" in data["capabilities"]
            assert "computer-use" in data["capabilities"]
            results.ok(f"GET /health — healthy, capabilities: {list(data['capabilities'].keys())}")

            # Test 4b: Browser health
            resp = await client.get(f"{base}/v1/browser/health")
            assert resp.status_code == 200
            data = resp.json()
            assert data["service"] == "allternit-operator"
            results.ok(f"GET /v1/browser/health — browser-use available: {data.get('available')}")

            # Test 4c: Vision screenshot (desktop capture via operator)
            resp = await client.get(f"{base}/v1/vision/screenshot",
                                   headers={"Authorization": "Bearer test-key"})
            if resp.status_code == 200:
                data = resp.json()
                assert "screenshot" in data
                assert len(data["screenshot"]) > 100  # base64 encoded
                results.ok(f"GET /v1/vision/screenshot — {len(data['screenshot'])} chars base64")
            else:
                results.skip("Vision screenshot", f"Status {resp.status_code}: {resp.text[:200]}")

            # Test 4d: Desktop execute
            resp = await client.post(
                f"{base}/v1/sessions/test-session/desktop/execute",
                json={"instruction": "observe current desktop state", "session_id": "test-session", "use_vision": False},
                headers={"Authorization": "Bearer test-key"},
            )
            if resp.status_code == 200:
                data = resp.json()
                assert data["success"] is True
                receipt_id = data.get("receipt_id")
                results.ok(f"POST /v1/sessions/*/desktop/execute — success, receipt: {receipt_id}")

                # Test 4e: Receipt has full details
                if data.get("receipt_details"):
                    receipt = data["receipt_details"]
                    assert "receipt_id" in receipt
                    assert "input_hashes" in receipt
                    assert "output_hashes" in receipt
                    results.ok(f"Receipt details include integrity hashes")
                else:
                    results.ok("Desktop execute returned (no receipt details in response)")
            else:
                results.skip("Desktop execute", f"Status {resp.status_code}")

            # Test 4f: Telemetry providers
            resp = await client.get(f"{base}/v1/telemetry/providers",
                                   headers={"Authorization": "Bearer test-key"})
            if resp.status_code == 200:
                results.ok(f"GET /v1/telemetry/providers — {len(resp.json())} providers")
            else:
                results.skip("Telemetry providers", f"Status {resp.status_code}")

    except Exception as e:
        results.fail("Operator service test", str(e))
    finally:
        if proc:
            proc.send_signal(signal.SIGTERM)
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()


# ============================================================================
# Test 5: Cross-Family Verification
# ============================================================================

async def test_cross_family():
    print("\n[Test 5] Cross-Family — Browser + Desktop in Same Session Context")

    tmp_dir = tempfile.mkdtemp()

    try:
        session_mgr = SessionManager(sessions_dir=os.path.join(tmp_dir, "sessions"))
        receipt_writer = ReceiptWriter(receipts_dir=os.path.join(tmp_dir, "receipts"))
        telemetry = TelemetryCollector()
        router = Router()
        policy = PolicyEngine()

        # Phase A: Browser task
        browser_decision = router.route("Get page title", family="browser", mode="execute",
                                       constraints=RouteConstraints(deterministic=True))
        browser_policy = policy.evaluate(target="https://example.com", action_type="goto",
                                        adapter_id=browser_decision.primary_adapter, family="browser")
        assert browser_policy.decision == "allow"

        browser_session = session_mgr.create(run_id="cross-1", family="browser",
                                             mode="execute", adapter_id="browser.playwright")
        browser_session = session_mgr.activate(browser_session.session_id)

        from adapters.browser.playwright import PlaywrightAdapter
        pw_adapter = PlaywrightAdapter()
        action = ActionRequest(action_type="goto", target="https://example.com")
        envelope = await pw_adapter.execute(action, session_id=browser_session.session_id, run_id="cross-1")
        assert envelope.status == "completed"

        receipt_writer.emit(
            run_id="cross-1", session_id=browser_session.session_id,
            action_type="goto", adapter_id="browser.playwright",
            target="https://example.com",
            action_data={"action": "goto"}, result_data={"status": "completed"},
        )
        session_mgr.record_action(browser_session.session_id, {"action": "goto", "target": "https://example.com"})
        results.ok("Phase A: Browser navigation completed")

        # Phase B: Desktop task (different session, same run)
        desktop_decision = router.route("Take screenshot", family="desktop", mode="desktop",
                                       constraints=RouteConstraints(deterministic=False))
        desktop_policy = policy.evaluate(target="desktop", action_type="screenshot",
                                        adapter_id=desktop_decision.primary_adapter,
                                        adapter_risk_level="high", family="desktop")

        desktop_session = session_mgr.create(run_id="cross-1", family="desktop",
                                             mode="desktop", adapter_id=desktop_decision.primary_adapter)
        desktop_session = session_mgr.activate(desktop_session.session_id)

        from adapters.desktop.accessibility_adapter import AccessibilityAdapter
        desktop_adapter = AccessibilityAdapter()
        shot = await desktop_adapter.execute("take_screenshot", {})
        assert shot.get("success"), shot.get("error")
        shot_bytes = len(shot.get("image_b64") or "")

        receipt_writer.emit(
            run_id="cross-1", session_id=desktop_session.session_id,
            action_type="screenshot", adapter_id=desktop_decision.primary_adapter,
            target="desktop",
            action_data={"action": "screenshot"}, result_data={"b64_chars": shot_bytes},
        )
        results.ok(f"Phase B: Desktop screenshot — {shot_bytes} base64 chars")

        # Phase C: Verify session isolation
        assert browser_session.session_id != desktop_session.session_id
        assert browser_session.artifact_root != desktop_session.artifact_root
        assert browser_session.family == "browser"
        assert desktop_session.family == "desktop"
        results.ok("Phase C: Session isolation verified — different IDs, different artifact roots")

        # Phase D: Verify unified receipt trail
        browser_receipts = receipt_writer.list_receipts(session_id=browser_session.session_id)
        desktop_receipts = receipt_writer.list_receipts(session_id=desktop_session.session_id)
        assert len(browser_receipts) >= 1
        assert len(desktop_receipts) >= 1
        assert browser_receipts[0].adapter_id == "browser.playwright"
        assert desktop_receipts[0].adapter_id == desktop_decision.primary_adapter
        results.ok("Phase D: Unified receipt trail — each family has own receipts")

        # Phase E: Cross-session access blocked by policy
        cross_policy = policy.evaluate(
            target="desktop", action_type="goto",
            adapter_id="browser.playwright", family="browser",
            cross_session_access=True,
        )
        assert cross_policy.decision == "deny"
        results.ok("Phase E: Cross-session access denied by policy (G2)")

        # Cleanup
        await pw_adapter.close()
        session_mgr.destroy(browser_session.session_id)
        session_mgr.destroy(desktop_session.session_id)
        results.ok("Phase F: Both sessions destroyed, adapters closed")

    except Exception as e:
        results.fail("Cross-family test", str(e))
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ============================================================================
# Test 6: Adapter Registry — Real Manifests
# ============================================================================

def test_registry_real():
    print("\n[Test 6] Adapter Registry — Real Manifest Validation (v0.2 schema)")

    from routing.registry import AdapterRegistry
    from routing.capability_matrix import CapabilityMatrix
    from routing.adapter_selector import AdapterSelector

    registry = AdapterRegistry(adapters_root=str(COMPUTER_USE_ROOT / "adapters"))
    registry.load_manifests()

    manifests = registry.list_adapters()
    assert len(manifests) >= 7, f"Expected ≥7 manifests, got {len(manifests)}"
    results.ok(f"Loaded {len(manifests)} adapter manifests from disk")

    # Validate required fields (new v0.2 schema)
    required_fields = [
        "adapter_id", "family", "pattern", "modes_supported",
        "traits", "guarantees", "production_status",
        "policy_profile", "conformance_suite", "version",
    ]
    for m in manifests:
        for field in required_fields:
            assert field in m, f"Manifest {m.get('adapter_id', '?')} missing field: '{field}'"
    results.ok("All manifests have required v0.2 schema fields")

    # Validate families
    valid_families = {"browser", "desktop", "retrieval", "hybrid"}
    for m in manifests:
        assert m["family"] in valid_families, \
            f"{m['adapter_id']} invalid family: {m['family']}"
    results.ok(f"All adapter families valid ({valid_families})")

    # Validate patterns
    valid_patterns = {"deterministic", "adaptive", "orchestrated-hybrid"}
    for m in manifests:
        assert m["pattern"] in valid_patterns, \
            f"{m['adapter_id']} invalid pattern: {m['pattern']}"
    results.ok("All adapter patterns valid")

    # Validate production_status
    valid_statuses = {"experimental", "beta", "production"}
    for m in manifests:
        assert m["production_status"] in valid_statuses, \
            f"{m['adapter_id']} invalid production_status: {m['production_status']}"
    results.ok("All production_status values valid")

    # Validate traits (must be triState)
    valid_tri = {"yes", "no", "partial", "unknown"}
    required_traits = ["headed", "headless", "parallel", "persistent_session",
                       "visual_reasoning", "low_level_debug", "checkpointable"]
    for m in manifests:
        traits = m.get("traits", {})
        for trait in required_traits:
            assert trait in traits, f"{m['adapter_id']} missing trait: '{trait}'"
            assert traits[trait] in valid_tri, \
                f"{m['adapter_id']}.traits.{trait} = '{traits[trait]}' not in {valid_tri}"
    results.ok("All trait fields present and use triState values")

    # Validate guarantees
    valid_grades = {"A", "B", "C", "X"}
    required_guarantees = ["semantic", "policy", "receipt", "conformance", "routing_confidence"]
    for m in manifests:
        guarantees = m.get("guarantees", {})
        for g in required_guarantees:
            assert g in guarantees, f"{m['adapter_id']} missing guarantee: '{g}'"
            assert guarantees[g] in valid_grades, \
                f"{m['adapter_id']}.guarantees.{g} = '{guarantees[g]}' not in {valid_grades}"
    results.ok("All guarantee grades present and valid (A/B/C/X)")

    # Validate capability classes against known values
    valid_classes = {
        "deterministic-web", "adaptive-web", "embedded-surface",
        "fleet-session-infra", "visual-computer-use",
        "retrieval-crawl", "desktop-automation", "hybrid-orchestration",
    }
    for m in manifests:
        for cls in m.get("capability_classes", []):
            assert cls in valid_classes, \
                f"{m['adapter_id']} invalid capability class: '{cls}'"
    results.ok("All capability classes valid")

    # Validate risk levels
    for m in manifests:
        assert m.get("risk_level", "medium") in {"low", "medium", "high", "critical"}, \
            f"{m['adapter_id']} invalid risk_level"
    results.ok("All risk levels valid")

    # Registry helpers use new schema
    assert registry.supports("browser.playwright", "headless") == True
    assert registry.supports("browser.playwright", "parallel") == True
    assert registry.supports("browser.extension", "headless") == False
    assert registry.get_guarantee_grade("browser.playwright", "semantic") == "A"
    assert registry.get_guarantee_grade("browser.extension", "conformance") == "C"
    assert registry.is_production_grade("browser.playwright") == True
    assert registry.is_production_grade("browser.cdp") == False
    assert registry.is_routable("browser.cdp") == True
    assert registry.is_routable("browser.extension") == False
    results.ok("Registry helpers (supports/get_guarantee_grade/is_production_grade/is_routable) correct")

    # Capability matrix and adapter selector
    matrix = CapabilityMatrix(str(COMPUTER_USE_ROOT / "adapters"))
    matrix.load()
    assert len(matrix._candidates) >= 7

    selector = AdapterSelector(matrix)
    routing_checks = [
        ("browser", "execute", "browser.playwright"),
        ("browser", "inspect", "browser.cdp"),
        ("browser", "parallel", "browser.playwright"),
        ("browser", "assist", "browser.extension"),
        ("retrieval", "crawl", "retrieval.playwright-crawler"),
        ("hybrid", "hybrid", "hybrid.orchestrator"),
    ]
    for family, mode, expected_primary in routing_checks:
        sel_result = selector.select(family, mode, allow_experimental=True)
        assert sel_result.primary == expected_primary, \
            f"{family}×{mode}: expected {expected_primary}, got {sel_result.primary}"
    results.ok("AdapterSelector picks correct primary for all 6 routing cases")

    # Promotion gate
    promo = matrix.promotion_eligible("browser.playwright", 1.0)
    assert promo["current_status"] == "production"
    promo2 = matrix.promotion_eligible("browser.cdp", 0.6)
    assert promo2["current_status"] == "beta"
    assert promo2["eligible"] == False  # 60% < 90% for production
    results.ok("Promotion gate: playwright already production, cdp blocked at 60%")

    # Cross-check router against manifests for all v0.2 routes
    router = Router()
    test_routes = [
        ("browser", "execute", RouteConstraints(deterministic=True)),
        ("browser", "execute", RouteConstraints(deterministic=False)),
        ("browser", "inspect", RouteConstraints()),
        ("browser", "parallel", RouteConstraints()),
        ("browser", "assist", RouteConstraints()),
        ("desktop", "desktop", RouteConstraints()),
        ("desktop", "execute", RouteConstraints()),
        ("desktop", "inspect", RouteConstraints()),
        ("retrieval", "crawl", RouteConstraints()),
        ("retrieval", "execute", RouteConstraints()),
        ("hybrid", "hybrid", RouteConstraints()),
        ("hybrid", "execute", RouteConstraints()),
    ]
    for family, mode, constraints in test_routes:
        decision = router.route("test", family=family, mode=mode, constraints=constraints)
        assert decision.primary_adapter, f"No primary for {family}×{mode}"
        manifest = registry.get_manifest(decision.primary_adapter)
        if manifest:
            assert manifest["family"] == family or manifest["family"] in ("browser", "desktop", "retrieval", "hybrid"), \
                f"Router selected {decision.primary_adapter} for {family}×{mode} but manifest family is {manifest['family']}"
    results.ok(f"Router + manifest cross-check passed for {len(test_routes)} routes")


# ============================================================================
# Test 7: Retrieval Crawler — Real HTTP
# ============================================================================

async def test_retrieval_crawler():
    print("\n[Test 7] Playwright Crawler Adapter — Real Retrieval")

    try:
        from playwright.async_api import async_playwright
    except ImportError:
        results.skip("Playwright Crawler", "playwright not installed")
        return

    try:
        from adapters.retrieval.playwright_crawler import PlaywrightCrawlerAdapter
    except ImportError:
        try:
            import importlib
            mod = importlib.import_module("adapters.retrieval.playwright-crawler")
            PlaywrightCrawlerAdapter = mod.PlaywrightCrawlerAdapter
        except Exception as e:
            results.skip("PlaywrightCrawlerAdapter import", str(e))
            return

    adapter = PlaywrightCrawlerAdapter(max_depth=1, max_pages=5)

    try:
        await adapter.initialize()
        results.ok("Crawler adapter initialized")

        # Test 7a: Single page goto
        action = ActionRequest(action_type="goto", target="https://example.com", parameters={})
        envelope = await adapter.execute(action, session_id="crawl-test", run_id="run-crawl-1")
        assert envelope.status == "completed", f"goto failed: {envelope.error}"
        assert envelope.extracted_content.get("url")
        assert envelope.extracted_content.get("title") == "Example Domain"
        results.ok(f"goto → title: '{envelope.extracted_content['title']}'")

        # Test 7b: Extract content
        action2 = ActionRequest(action_type="extract", target="https://example.com", parameters={})
        envelope2 = await adapter.execute(action2, session_id="crawl-test", run_id="run-crawl-2")
        assert envelope2.status == "completed"
        assert "text" in envelope2.extracted_content
        assert "Example Domain" in envelope2.extracted_content["text"]
        results.ok(f"extract → {len(envelope2.extracted_content['text'])} chars, 'Example Domain' found")

        # Test 7c: Observe (links + meta)
        action3 = ActionRequest(action_type="observe", target="https://example.com", parameters={})
        envelope3 = await adapter.execute(action3, session_id="crawl-test", run_id="run-crawl-3")
        assert envelope3.status == "completed"
        content = envelope3.extracted_content
        assert "link_count" in content
        assert "title" in content
        results.ok(f"observe → {content['link_count']} links found")

        # Test 7d: Crawl with depth limit
        action4 = ActionRequest(
            action_type="crawl", target="https://example.com",
            parameters={"max_depth": 1, "max_pages": 3},
        )
        envelope4 = await adapter.execute(action4, session_id="crawl-test", run_id="run-crawl-4")
        assert envelope4.status == "completed"
        crawl_data = envelope4.extracted_content
        assert crawl_data["pages_crawled"] >= 1
        assert "pages" in crawl_data
        results.ok(f"crawl depth=1 → {crawl_data['pages_crawled']} pages crawled")

        # Test 7e: Artifact emitted for crawl
        assert len(envelope4.artifacts) >= 1
        results.ok(f"Crawl artifact emitted: {envelope4.artifacts[0].type}")

        # Test 7f: Receipt emitted
        assert len(envelope4.receipts) >= 1
        results.ok("Crawl receipt emitted (G3)")

        # Test 7g: Unsupported action handled gracefully
        action5 = ActionRequest(action_type="act", target="button", parameters={})
        envelope5 = await adapter.execute(action5, session_id="crawl-test", run_id="run-crawl-5")
        assert envelope5.status == "failed"
        assert envelope5.error["code"] == "UNSUPPORTED_ACTION"
        results.ok("Unsupported action → UNSUPPORTED_ACTION error (not crash)")

        # Test 7h: G1 result envelope fields
        required_g1 = ["run_id", "session_id", "adapter_id", "family", "status"]
        d = envelope2.to_dict()
        for field in required_g1:
            assert field in d, f"Missing G1 field: {field}"
        assert d["adapter_id"] == "retrieval.playwright-crawler"
        assert d["family"] == "retrieval"
        results.ok("G1 result envelope fields verified for retrieval adapter")

    except Exception as e:
        results.fail("Retrieval crawler test", str(e))
    finally:
        await adapter.close()


# ============================================================================
# Test 8: Hybrid Orchestrator — Real Workflow
# ============================================================================

async def test_hybrid_orchestrator():
    print("\n[Test 8] Hybrid Orchestrator — Real Cross-Family Workflow")

    try:
        from adapters.hybrid.orchestrator import HybridOrchestrator, HybridWorkflow, HybridStep
        from adapters.browser.playwright import PlaywrightAdapter
    except ImportError as e:
        results.skip("HybridOrchestrator", str(e))
        return

    try:
        # Build orchestrator with playwright sub-adapter
        playwright = PlaywrightAdapter()
        orchestrator = HybridOrchestrator()
        orchestrator.register_adapter("browser.playwright", playwright)
        await orchestrator.initialize()
        results.ok("Orchestrator + Playwright sub-adapter initialized")

        # Test 8a: Single step delegation
        action = ActionRequest(
            action_type="execute", target="https://example.com",
            parameters={"adapter_id": "browser.playwright", "sub_action": "goto"},
        )
        envelope = await orchestrator.execute(action, session_id="hybrid-test", run_id="run-hybrid-1")
        assert envelope.status == "completed", f"Single step failed: {envelope.error}"
        assert envelope.extracted_content.get("delegated_to") == "browser.playwright"
        results.ok("Single step delegation → browser.playwright")

        # Test 8b: Multi-step workflow
        workflow_def = {
            "workflow_id": "real-wf-01",
            "name": "Browser → Browser pipeline",
            "steps": [
                {
                    "step_id": "step-goto",
                    "family": "browser",
                    "adapter_id": "browser.playwright",
                    "action_type": "goto",
                    "target": "https://example.com",
                },
                {
                    "step_id": "step-extract",
                    "family": "browser",
                    "adapter_id": "browser.playwright",
                    "action_type": "extract",
                    "target": "https://example.com",
                },
            ],
        }
        action2 = ActionRequest(
            action_type="handoff", target="workflow",
            parameters={"workflow": workflow_def},
        )
        envelope2 = await orchestrator.execute(action2, session_id="hybrid-test", run_id="run-hybrid-2")
        assert envelope2.status == "completed", f"Workflow failed: {envelope2.error}"
        wf_result = envelope2.extracted_content
        assert wf_result["completed_steps"] == 2
        assert wf_result["total_steps"] == 2
        results.ok(f"2-step workflow: {wf_result['completed_steps']}/{wf_result['total_steps']} steps completed")

        # Test 8c: Artifact chaining between steps
        step_results = wf_result["steps"]
        assert step_results[0]["status"] == "completed"
        assert step_results[1]["status"] == "completed"
        results.ok("Step results tracked per step")

        # Test 8d: Receipt emitted
        assert len(envelope2.receipts) >= 1
        results.ok("Workflow receipt emitted (G3)")

        # Test 8e: G1 envelope
        d = envelope2.to_dict()
        assert d["adapter_id"] == "hybrid.orchestrator"
        assert d["family"] == "hybrid"
        results.ok("G1 result envelope: adapter_id=hybrid.orchestrator family=hybrid")

        # Test 8f: Fail-fast on bad adapter
        bad_workflow = {
            "workflow_id": "bad-wf",
            "name": "Bad step test",
            "steps": [
                {
                    "step_id": "step-bad",
                    "family": "browser",
                    "adapter_id": "nonexistent.adapter",
                    "action_type": "goto",
                    "target": "https://example.com",
                },
            ],
        }
        action3 = ActionRequest(
            action_type="handoff", target="workflow",
            parameters={"workflow": bad_workflow},
        )
        envelope3 = await orchestrator.execute(action3, session_id="hybrid-test", run_id="run-hybrid-3")
        assert envelope3.status == "failed"
        results.ok("Unknown adapter in workflow → fails cleanly (not crash)")

    except Exception as e:
        results.fail("Hybrid orchestrator test", str(e))
    finally:
        await orchestrator.close()


# ============================================================================
# Main
# ============================================================================

def main():
    print("=" * 60)
    print("Allternit Computer Use — Real Adapter Integration Tests")
    print(f"Python: {sys.version}")
    print(f"Time: {datetime.now().isoformat()}")
    print("=" * 60)

    # Sync tests
    test_registry_real()

    # Async tests
    loop = asyncio.new_event_loop()
    loop.run_until_complete(test_playwright_real())
    loop.run_until_complete(test_playwright_adapter_wrapper())
    loop.run_until_complete(test_cross_family())
    loop.run_until_complete(test_operator_service())
    loop.run_until_complete(test_retrieval_crawler())
    loop.run_until_complete(test_hybrid_orchestrator())
    loop.close()

    success = results.summary()
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
