"""
Allternit Computer Use — End-to-End Tests
Validates the full pipeline: routing → policy → session → adapter → receipt.
No external dependencies required (uses mock adapters).
"""

import asyncio
import json
import sys
import os
import tempfile
import shutil
from pathlib import Path

# Add packages to path — directory is "computer-use" (hyphen) which Python
# can't import directly, so we add its parent and alias the module.
PACKAGES_ROOT = Path(__file__).resolve().parents[2]
COMPUTER_USE_ROOT = PACKAGES_ROOT / "computer-use"
sys.path.insert(0, str(PACKAGES_ROOT))
sys.path.insert(0, str(COMPUTER_USE_ROOT))

# Import from the package modules directly (each is a top-level package
# because computer-use/ is on sys.path)
from core import BaseAdapter, ActionRequest, ResultEnvelope, Receipt, Artifact, PolicyDecision
from routing import Router, RouteConstraints, RouteDecision, RoutingError
from routing.registry import AdapterRegistry
from policy import PolicyEngine, PolicyEvaluation, PolicyRule
from receipts import ReceiptWriter, ActionReceipt, Evidence
from sessions import SessionManager, Session
from telemetry import TelemetryCollector, TelemetryEvent
from conformance import ConformanceSuite, ConformanceTest, ConformanceRunner, SuiteResult
from conformance.suites import build_all_suites


# ============================================================================
# Test Helpers
# ============================================================================

class TestResults:
    def __init__(self):
        self.passed = 0
        self.failed = 0
        self.errors = []

    def ok(self, name: str):
        self.passed += 1
        print(f"  ✓ {name}")

    def fail(self, name: str, reason: str):
        self.failed += 1
        self.errors.append(f"{name}: {reason}")
        print(f"  ✗ {name} — {reason}")

    def summary(self):
        total = self.passed + self.failed
        print(f"\n{'='*60}")
        print(f"Results: {self.passed}/{total} passed, {self.failed} failed")
        if self.errors:
            print("\nFailures:")
            for e in self.errors:
                print(f"  - {e}")
        print(f"{'='*60}")
        return self.failed == 0


results = TestResults()


# ============================================================================
# Mock Adapter for Testing
# ============================================================================

class MockBrowserAdapter(BaseAdapter):
    """Mock adapter that simulates successful browser actions."""

    def __init__(self):
        self._initialized = False
        self.actions_executed = []

    @property
    def adapter_id(self) -> str:
        return "browser.mock"

    @property
    def family(self) -> str:
        return "browser"

    async def initialize(self) -> None:
        self._initialized = True

    async def execute(self, action: ActionRequest, session_id: str, run_id: str) -> ResultEnvelope:
        envelope = self._make_envelope(action, session_id, run_id)
        self.actions_executed.append(action.action_type)

        result_data = {
            "url": action.target,
            "title": f"Mock page for {action.target}",
            "content": "Mock extracted content",
        }

        envelope.status = "completed"
        envelope.extracted_content = result_data
        from datetime import datetime
        envelope.completed_at = datetime.utcnow().isoformat()

        receipt = self._emit_receipt(envelope, action, result_data)

        return envelope

    async def close(self) -> None:
        self._initialized = False


# ============================================================================
# Test 1: Core Types
# ============================================================================

def test_core_types():
    print("\n[Test 1] Core Types")

    # ActionRequest
    action = ActionRequest(action_type="goto", target="https://example.com")
    assert action.action_id, "action_id should be auto-generated"
    assert action.action_type == "goto"
    assert action.target == "https://example.com"
    assert action.timeout_ms == 30000
    results.ok("ActionRequest creation")

    # ResultEnvelope
    envelope = ResultEnvelope(
        run_id="run-1",
        session_id="ses-1",
        adapter_id="browser.playwright",
        family="browser",
        mode="execute",
        action="goto",
        target="https://example.com",
    )
    d = envelope.to_dict()
    assert d["run_id"] == "run-1"
    assert d["family"] == "browser"
    assert d["status"] == "pending"
    assert isinstance(d["artifacts"], list)
    assert isinstance(d["receipts"], list)
    assert isinstance(d["fallbacks_used"], list)
    results.ok("ResultEnvelope creation and serialization")

    # Receipt
    receipt = Receipt(
        run_id="run-1",
        session_id="ses-1",
        action_type="goto",
        adapter_id="browser.playwright",
        target="https://example.com",
    )
    h = receipt.compute_integrity_hash(
        {"action": "goto", "target": "https://example.com"},
        {"url": "https://example.com", "title": "Example"}
    )
    assert len(h) == 64, "SHA-256 hash should be 64 hex chars"
    assert receipt.integrity_hash == h
    results.ok("Receipt integrity hash computation")

    # Artifact
    artifact = Artifact(type="screenshot", path="screenshots/test.png", size_bytes=1024, media_type="image/png")
    assert artifact.type == "screenshot"
    results.ok("Artifact creation")

    # PolicyDecision
    pd = PolicyDecision(decision="allow", reason="No policy triggered")
    assert pd.decision == "allow"
    results.ok("PolicyDecision creation")


# ============================================================================
# Test 2: Router
# ============================================================================

def test_router():
    print("\n[Test 2] Router")

    router = Router()

    # ── Browser family ──

    # Deterministic browser execute
    decision = router.route("Fill a form", family="browser", mode="execute", constraints=RouteConstraints(deterministic=True))
    assert decision.primary_adapter == "browser.playwright", f"Expected browser.playwright, got {decision.primary_adapter}"
    assert decision.family == "browser"
    assert decision.mode == "execute"
    results.ok("browser × execute (deterministic) → playwright")

    # Adaptive browser execute
    decision = router.route("Extract data", family="browser", mode="execute", constraints=RouteConstraints(deterministic=False))
    assert decision.primary_adapter == "browser.playwright", f"Expected browser.playwright, got {decision.primary_adapter}"
    results.ok("browser × execute (adaptive) → playwright")

    # Inspect mode
    decision = router.route("Debug this page", family="browser", mode="inspect")
    assert decision.primary_adapter == "browser.cdp", f"Expected browser.cdp, got {decision.primary_adapter}"
    assert "browser.playwright" in decision.fallback_chain
    results.ok("browser × inspect → cdp")

    # Parallel mode
    decision = router.route("Run 5 browsers", family="browser", mode="parallel")
    assert decision.primary_adapter == "browser.playwright", f"Expected browser.playwright, got {decision.primary_adapter}"
    results.ok("browser × parallel → playwright")

    # Browser × desktop mode (cross-family route)
    decision = router.route("Capture browser screen", family="browser", mode="desktop")
    assert decision.primary_adapter == "browser.playwright"
    results.ok("browser × desktop → playwright (screenshot/capture)")

    # ── Desktop family ──

    # Desktop × desktop
    decision = router.route("Click button", family="desktop", mode="desktop")
    assert decision.primary_adapter == "desktop.accessibility", f"Expected desktop.accessibility, got {decision.primary_adapter}"
    results.ok("desktop × desktop → accessibility")

    # Desktop × execute
    decision = router.route("Type text", family="desktop", mode="execute")
    assert decision.primary_adapter == "desktop.accessibility"
    results.ok("desktop × execute → accessibility")

    # Desktop × inspect
    decision = router.route("Screenshot debug", family="desktop", mode="inspect")
    assert decision.primary_adapter == "desktop.accessibility"
    results.ok("desktop × inspect → accessibility")

    # Desktop × parallel
    decision = router.route("Multi screenshot", family="desktop", mode="parallel")
    assert decision.primary_adapter == "desktop.accessibility"
    results.ok("desktop × parallel → accessibility")

    # ── Error cases ──

    # Unknown family raises RoutingError
    try:
        router.route("test", family="unknown", mode="execute")
        results.fail("Unknown family", "Should have raised RoutingError")
    except RoutingError:
        results.ok("Unknown family → RoutingError")

    # Unknown mode raises RoutingError
    try:
        router.route("test", family="browser", mode="nonexistent")
        results.fail("Unknown mode", "Should have raised RoutingError")
    except RoutingError:
        results.ok("Unknown mode → RoutingError")

    # ── Routing guarantees ──

    # Routing determinism (G5)
    d1 = router.route("test", family="browser", mode="execute", constraints=RouteConstraints(deterministic=True))
    d2 = router.route("test", family="browser", mode="execute", constraints=RouteConstraints(deterministic=True))
    assert d1.primary_adapter == d2.primary_adapter, "Same inputs must produce same adapter"
    assert d1.fallback_chain == d2.fallback_chain, "Same inputs must produce same fallbacks"
    results.ok("Routing determinism (G5)")

    # ── v0.2 Families and Modes ──

    # Retrieval × crawl
    decision = router.route("Crawl docs site", family="retrieval", mode="crawl")
    assert decision.primary_adapter == "retrieval.playwright-crawler"
    results.ok("retrieval × crawl → playwright-crawler")

    # Retrieval × execute (single-page extract)
    decision = router.route("Extract page data", family="retrieval", mode="execute")
    assert decision.primary_adapter == "retrieval.playwright-crawler"
    results.ok("retrieval × execute → playwright-crawler")

    # Retrieval × inspect
    decision = router.route("Preview crawl", family="retrieval", mode="inspect")
    assert decision.primary_adapter == "retrieval.playwright-crawler"
    results.ok("retrieval × inspect → playwright-crawler")

    # Hybrid × hybrid
    decision = router.route("Download then process", family="hybrid", mode="hybrid")
    assert decision.primary_adapter == "hybrid.orchestrator"
    results.ok("hybrid × hybrid → orchestrator")

    # Hybrid × execute
    decision = router.route("Cross-family task", family="hybrid", mode="execute")
    assert decision.primary_adapter == "hybrid.orchestrator"
    results.ok("hybrid × execute → orchestrator")

    # Browser × assist
    decision = router.route("Help user fill form", family="browser", mode="assist")
    assert decision.primary_adapter == "browser.extension"
    assert "browser.playwright" in decision.fallback_chain
    results.ok("browser × assist → extension (with playwright fallback)")

    # Desktop × assist
    decision = router.route("Guide user through app", family="desktop", mode="assist")
    assert decision.primary_adapter == "desktop.accessibility"
    results.ok("desktop × assist → accessibility")

    # All supported routes list
    routes = router.list_supported_routes()
    families_covered = {r["family"] for r in routes}
    assert "browser" in families_covered
    assert "desktop" in families_covered
    assert "retrieval" in families_covered
    assert "hybrid" in families_covered
    results.ok(f"list_supported_routes → {len(routes)} routes, families: {sorted(families_covered)}")

    # Decision audit trail
    decisions = router.get_decisions()
    assert len(decisions) >= 18, f"Expected ≥18 logged decisions, got {len(decisions)}"
    results.ok("Route decisions logged for audit")


# ============================================================================
# Test 3: Policy Engine
# ============================================================================

def test_policy():
    print("\n[Test 3] Policy Engine")

    engine = PolicyEngine()

    # Allow normal request
    result = engine.evaluate(target="https://example.com", action_type="goto", adapter_id="browser.playwright")
    assert result.decision == "allow", f"Expected allow, got {result.decision}"
    results.ok("Normal request → allow")

    # Destructive action gate
    result = engine.evaluate(
        target="https://shop.com/checkout",
        action_type="act",
        action_description="confirm purchase and submit payment",
        adapter_id="browser.playwright",
    )
    assert result.decision == "require_approval", f"Expected require_approval for destructive action, got {result.decision}"
    assert "P-002" in result.rules_applied
    results.ok("Destructive action → require_approval (G2)")

    # High-risk desktop → force headed
    result = engine.evaluate(
        target="TextEdit",
        action_type="act",
        adapter_id="desktop.accessibility",
        adapter_risk_level="high",
        family="desktop",
    )
    assert result.decision == "require_headed" or result.overrides, f"Expected headed override for high-risk desktop, got {result.decision}"
    results.ok("High-risk desktop → force headed (G2)")

    # Session isolation
    result = engine.evaluate(
        target="https://example.com",
        action_type="goto",
        adapter_id="browser.playwright",
        cross_session_access=True,
    )
    assert result.decision == "deny", f"Expected deny for cross-session access, got {result.decision}"
    assert "P-007" in result.rules_applied
    results.ok("Cross-session access → deny (G2)")

    # Credential isolation
    result = engine.evaluate(
        target="https://example.com",
        action_type="goto",
        adapter_id="browser.playwright",
        cross_session_auth=True,
    )
    assert result.decision == "deny", f"Expected deny for cross-session auth, got {result.decision}"
    assert "P-004" in result.rules_applied
    results.ok("Cross-session auth → deny (G2)")

    # Experimental adapter blocked
    result = engine.evaluate(
        target="https://example.com",
        action_type="goto",
        adapter_id="browser.cdp",
        adapter_conformance_grade="experimental",
        explicit_opt_in=False,
    )
    assert result.decision == "deny", f"Expected deny for experimental adapter, got {result.decision}"
    results.ok("Experimental adapter without opt-in → deny (G2)")

    # Experimental adapter allowed with opt-in
    result = engine.evaluate(
        target="https://example.com",
        action_type="goto",
        adapter_id="browser.cdp",
        adapter_conformance_grade="experimental",
        explicit_opt_in=True,
    )
    assert result.decision == "allow", f"Expected allow with explicit opt-in, got {result.decision}"
    results.ok("Experimental adapter with opt-in → allow")

    # Audit trail
    evaluations = engine.get_evaluations()
    assert len(evaluations) >= 7
    results.ok("Policy evaluations logged for audit")


# ============================================================================
# Test 4: Session Manager
# ============================================================================

def test_sessions():
    print("\n[Test 4] Session Manager")

    tmp_dir = tempfile.mkdtemp()
    try:
        mgr = SessionManager(sessions_dir=tmp_dir)

        # Create session
        session = mgr.create(run_id="run-1", family="browser", mode="execute", adapter_id="browser.playwright")
        assert session.session_id.startswith("ses_")
        assert session.status == "created"
        assert session.family == "browser"
        assert session.artifact_root, "artifact_root should be set"
        results.ok("Session creation")

        # Activate
        session = mgr.activate(session.session_id)
        assert session.status == "active"
        results.ok("Session activation")

        # Record action
        session = mgr.record_action(session.session_id, {"action": "goto", "target": "https://example.com"})
        assert len(session.previous_actions) == 1
        results.ok("Action recording")

        # Session isolation — get by ID
        other = mgr.get("nonexistent")
        assert other is None
        results.ok("Session isolation — unknown ID returns None")

        # List sessions
        sessions = mgr.list_sessions(family="browser")
        assert len(sessions) == 1
        results.ok("Session listing with filter")

        # Destroy
        session = mgr.destroy(session.session_id)
        assert session.status == "destroyed"
        assert session.destroyed_at is not None
        results.ok("Session destruction")

        # Persistence check
        session_file = Path(tmp_dir) / session.session_id / "session.json"
        assert session_file.exists(), "Session should be persisted to disk"
        with open(session_file) as f:
            data = json.load(f)
        assert data["status"] == "destroyed"
        results.ok("Session persistence to disk")

        # Artifact root isolation
        s1 = mgr.create(run_id="run-2", family="browser", mode="execute", adapter_id="browser.playwright")
        s2 = mgr.create(run_id="run-3", family="desktop", mode="desktop", adapter_id="desktop.accessibility")
        assert s1.artifact_root != s2.artifact_root, "Different sessions must have different artifact roots"
        results.ok("Artifact root isolation between sessions")

    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ============================================================================
# Test 5: Receipt Writer
# ============================================================================

def test_receipts():
    print("\n[Test 5] Receipt Writer")

    tmp_dir = tempfile.mkdtemp()
    try:
        writer = ReceiptWriter(receipts_dir=tmp_dir)

        # Emit receipt
        receipt = writer.emit(
            run_id="run-1",
            session_id="ses-1",
            action_type="goto",
            adapter_id="browser.playwright",
            target="https://example.com",
            action_data={"action": "goto", "target": "https://example.com"},
            result_data={"url": "https://example.com", "title": "Example"},
        )
        assert receipt.receipt_id.startswith("rcpt_")
        assert len(receipt.integrity_hash) == 64
        assert receipt.status == "success"
        results.ok("Receipt emission with integrity hash (G3)")

        # Persistence
        path = Path(tmp_dir) / f"{receipt.receipt_id}.json"
        assert path.exists(), "Receipt should be persisted"
        with open(path) as f:
            data = json.load(f)
        assert data["integrity_hash"] == receipt.integrity_hash
        results.ok("Receipt persistence to disk")

        # Read back
        read_back = writer.get_receipt(receipt.receipt_id)
        assert read_back is not None
        assert read_back["receipt_id"] == receipt.receipt_id
        results.ok("Receipt retrieval by ID")

        # Route decision receipt
        route_receipt = writer.emit_route_decision(
            run_id="run-1",
            session_id="ses-1",
            route_decision={"primary_adapter": "browser.playwright", "family": "browser", "mode": "execute"},
        )
        assert route_receipt.action_type == "route_decision"
        assert route_receipt.adapter_id == "router"
        results.ok("Route decision receipt (G5)")

        # List by session
        receipts = writer.list_receipts(session_id="ses-1")
        assert len(receipts) == 2
        results.ok("Receipt listing by session")

        # Evidence attachment
        receipt_with_evidence = writer.emit(
            run_id="run-1",
            session_id="ses-1",
            action_type="act",
            adapter_id="browser.playwright",
            target="button#submit",
            action_data={"action": "click", "selector": "button#submit"},
            result_data={"clicked": True},
            before_evidence=Evidence(url="https://example.com/form", dom_hash="abc123"),
            after_evidence=Evidence(url="https://example.com/success", dom_hash="def456"),
        )
        d = receipt_with_evidence.to_dict()
        assert d["before_evidence"]["dom_hash"] == "abc123"
        assert d["after_evidence"]["dom_hash"] == "def456"
        results.ok("Receipt with before/after evidence")

    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ============================================================================
# Test 6: Telemetry
# ============================================================================

def test_telemetry():
    print("\n[Test 6] Telemetry")

    collector = TelemetryCollector()

    # Emit events
    collector.adapter_started("browser.playwright", "browser", "execute", "ses-1", "run-1")
    collector.adapter_completed("browser.playwright", "browser", "execute", "ses-1", "run-1", duration_ms=1500)
    collector.adapter_error("browser.playwright", "browser", "ses-2", "run-2", "Connection refused")
    collector.fallback_used("browser.playwright", "browser.cdp", "Primary failed", "ses-2", "run-2")

    events = collector.get_events()
    assert len(events) == 4
    results.ok("Telemetry event emission")

    # Filter by session
    ses1_events = collector.get_events(session_id="ses-1")
    assert len(ses1_events) == 2
    results.ok("Telemetry filtering by session")

    # Filter by type
    errors = collector.get_events(event_type="adapter.error")
    assert len(errors) == 1
    results.ok("Telemetry filtering by event type")

    # Adapter metrics
    metrics = collector.get_adapter_metrics("browser.playwright")
    assert metrics["completions"] == 1
    assert metrics["errors"] == 1
    assert metrics["avg_duration_ms"] == 1500
    results.ok("Adapter metrics aggregation")

    # Listener
    received = []
    collector.add_listener(lambda e: received.append(e))
    collector.adapter_started("browser.cdp", "browser", "inspect", "ses-3", "run-3")
    assert len(received) == 1
    results.ok("Telemetry listener")


# ============================================================================
# Test 7: Adapter Registry
# ============================================================================

def test_registry():
    print("\n[Test 7] Adapter Registry")

    adapters_root = str(COMPUTER_USE_ROOT / "adapters")
    registry = AdapterRegistry(adapters_root=adapters_root)
    registry.load_manifests()

    all_adapters = registry.list_adapters()
    assert len(all_adapters) >= 7, f"Expected ≥7 adapter manifests, got {len(all_adapters)}"
    results.ok(f"Loaded {len(all_adapters)} adapter manifests")

    # Filter by family
    browser_adapters = registry.list_adapters(family="browser")
    assert len(browser_adapters) >= 4, f"Expected ≥4 browser adapters, got {len(browser_adapters)}"
    results.ok("Filter by family=browser")

    retrieval_adapters = registry.list_adapters(family="retrieval")
    assert len(retrieval_adapters) >= 1, f"Expected ≥1 retrieval adapters, got {len(retrieval_adapters)}"
    results.ok("Filter by family=retrieval")

    hybrid_adapters = registry.list_adapters(family="hybrid")
    assert len(hybrid_adapters) >= 1, f"Expected ≥1 hybrid adapters, got {len(hybrid_adapters)}"
    results.ok("Filter by family=hybrid")

    # Manifest lookup
    pw = registry.get_manifest("browser.playwright")
    assert pw is not None
    assert pw["family"] == "browser"
    assert "deterministic-web" in pw["capability_classes"]
    assert pw["traits"]["headless"] == "yes"
    assert pw["pattern"] == "deterministic"
    assert pw["production_status"] == "production"
    results.ok("Playwright manifest lookup (new schema)")

    # Capability check via traits
    assert registry.supports("browser.playwright", "parallel") == True
    assert registry.supports("browser.playwright", "headless") == True
    results.ok("Trait support check")

    # Guarantee grades
    assert registry.get_guarantee_grade("browser.playwright", "semantic") == "A"
    assert registry.get_guarantee_grade("browser.playwright", "routing_confidence") == "A"
    assert registry.get_guarantee_grade("browser.extension", "conformance") == "C"
    results.ok("Guarantee grade lookup")

    # Production status
    assert registry.is_production_grade("browser.playwright") == True
    assert registry.is_production_grade("browser.cdp") == False
    assert registry.is_routable("browser.cdp") == True   # beta is routable
    assert registry.is_routable("browser.extension") == False    # experimental is not
    results.ok("Production status and routability checks")

    # Mode filtering
    crawl_adapters = registry.list_adapters(mode="crawl")
    assert any(a["adapter_id"] == "retrieval.playwright-crawler" for a in crawl_adapters)
    results.ok("Filter by mode=crawl")


# ============================================================================
# Test 8: Mock Adapter E2E
# ============================================================================

async def test_adapter_e2e():
    print("\n[Test 8] Mock Adapter E2E Pipeline")

    adapter = MockBrowserAdapter()
    await adapter.initialize()
    assert adapter._initialized
    results.ok("Adapter initialization")

    action = ActionRequest(action_type="goto", target="https://example.com")
    envelope = await adapter.execute(action, session_id="ses-test", run_id="run-test")

    assert envelope.status == "completed"
    assert envelope.adapter_id == "browser.mock"
    assert envelope.family == "browser"
    assert envelope.action == "goto"
    assert envelope.target == "https://example.com"
    assert len(envelope.receipts) == 1
    assert envelope.extracted_content is not None
    results.ok("Adapter execution → ResultEnvelope (G1)")

    # Verify envelope serialization
    d = envelope.to_dict()
    required_fields = ["run_id", "session_id", "adapter_id", "family", "mode", "action", "target", "status", "artifacts", "receipts", "policy_decisions", "trace_id", "fallbacks_used"]
    for field in required_fields:
        assert field in d, f"Missing G1 field: {field}"
    results.ok("ResultEnvelope contains all G1 fields")

    await adapter.close()
    results.ok("Adapter cleanup")


# ============================================================================
# Test 9: Full Pipeline E2E
# ============================================================================

async def test_full_pipeline():
    print("\n[Test 9] Full Pipeline — Route → Policy → Session → Execute → Receipt")

    tmp_dir = tempfile.mkdtemp()
    try:
        # 1. Route
        router = Router()
        decision = router.route("Navigate to example.com", family="browser", mode="execute",
                               constraints=RouteConstraints(deterministic=True))
        assert decision.primary_adapter == "browser.playwright"
        results.ok("Step 1: Route decision")

        # 2. Policy check
        engine = PolicyEngine()
        policy = engine.evaluate(
            target="https://example.com",
            action_type="goto",
            adapter_id=decision.primary_adapter,
            family=decision.family,
            mode=decision.mode,
        )
        assert policy.decision == "allow"
        results.ok("Step 2: Policy evaluation → allow")

        # 3. Create session
        session_mgr = SessionManager(sessions_dir=tmp_dir)
        session = session_mgr.create(
            run_id="pipeline-run-1",
            family=decision.family,
            mode=decision.mode,
            adapter_id=decision.primary_adapter,
        )
        session = session_mgr.activate(session.session_id)
        results.ok("Step 3: Session created and activated")

        # 4. Initialize adapter and execute
        adapter = MockBrowserAdapter()
        await adapter.initialize()

        action = ActionRequest(action_type="goto", target="https://example.com")
        envelope = await adapter.execute(action, session_id=session.session_id, run_id=session.run_id)

        assert envelope.status == "completed"
        results.ok("Step 4: Adapter execution → completed")

        # 5. Emit receipts
        receipt_writer = ReceiptWriter(receipts_dir=os.path.join(tmp_dir, "receipts"))

        # Route decision receipt
        route_receipt = receipt_writer.emit_route_decision(
            run_id=session.run_id,
            session_id=session.session_id,
            route_decision=decision.to_dict(),
            policy_decision_id=policy.decision_id,
        )
        results.ok("Step 5a: Route decision receipt emitted")

        # Action receipt
        action_receipt = receipt_writer.emit(
            run_id=session.run_id,
            session_id=session.session_id,
            action_type="goto",
            adapter_id=adapter.adapter_id,
            target="https://example.com",
            action_data={"action": "goto", "target": "https://example.com"},
            result_data=envelope.extracted_content,
            policy_decision_id=policy.decision_id,
        )
        assert len(action_receipt.integrity_hash) == 64
        results.ok("Step 5b: Action receipt with integrity hash")

        # 6. Record action in session
        session_mgr.record_action(session.session_id, {
            "action": "goto",
            "target": "https://example.com",
            "receipt_id": action_receipt.receipt_id,
        })
        results.ok("Step 6: Action recorded in session")

        # 7. Telemetry
        telemetry = TelemetryCollector()
        telemetry.adapter_started(adapter.adapter_id, "browser", "execute", session.session_id, session.run_id)
        telemetry.adapter_completed(adapter.adapter_id, "browser", "execute", session.session_id, session.run_id, duration_ms=250)
        results.ok("Step 7: Telemetry emitted")

        # 8. Cleanup
        await adapter.close()
        session_mgr.destroy(session.session_id)
        results.ok("Step 8: Cleanup — adapter closed, session destroyed")

        # Verify full audit trail
        all_receipts = receipt_writer.list_receipts(session_id=session.session_id)
        assert len(all_receipts) == 2, f"Expected 2 receipts (route + action), got {len(all_receipts)}"
        results.ok("Full pipeline audit trail verified (2 receipts)")

        final_session = session_mgr.get(session.session_id)
        assert final_session.status == "destroyed"
        assert len(final_session.previous_actions) == 1
        results.ok("Final session state verified")

    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ============================================================================
# Test 10: Conformance Harness
# ============================================================================

async def test_conformance():
    print("\n[Test 10] Conformance Harness")

    # Build all suites
    suites = build_all_suites()
    assert len(suites) >= 5, f"Expected ≥5 conformance suites, got {len(suites)}"
    results.ok(f"Built {len(suites)} conformance suites")

    # Check test counts — all tests have real test_fn
    total_tests = sum(len(s.list_tests()) for s in suites)
    assert total_tests >= 20, f"Expected ≥20 total tests, got {total_tests}"
    for suite in suites:
        for test in suite.list_tests():
            assert test.test_fn is not None, f"Test {test.test_id} has no test_fn"
    results.ok(f"Total tests: {total_tests}, all have real test functions")

    # Run Suite F (routing/policy) against mock — these don't need a real adapter
    runner = ConformanceRunner()
    for suite in suites:
        runner.register_suite(suite)

    adapter = MockBrowserAdapter()
    result = await runner.run_suite("routing-policy-v1", adapter)
    assert result.total == 6
    assert result.passed == 6, f"Expected 6 passed, got {result.passed} (failures: {[r.to_dict() for r in result.results if r.status != 'pass']})"
    assert result.grade == "production"  # 100% pass rate
    results.ok("Suite F (routing/policy) → production grade (6/6 pass)")

    # Test with actual test functions
    custom_suite = ConformanceSuite("test-v1", "Test Suite", "Test")

    async def passing_test(adapter):
        assert True

    async def failing_test(adapter):
        assert False, "Intentional failure"

    custom_suite.add_test(ConformanceTest(test_id="T-01", suite_id="test-v1", name="Pass", description="", category="basic", test_fn=passing_test))
    custom_suite.add_test(ConformanceTest(test_id="T-02", suite_id="test-v1", name="Fail", description="", category="basic", test_fn=failing_test))

    runner.register_suite(custom_suite)
    result = await runner.run_suite("test-v1", adapter)
    assert result.passed == 1
    assert result.failed == 1
    assert result.pass_rate == 50.0
    assert result.grade == "beta"
    results.ok("Custom suite with pass/fail → beta grade (50%)")

    # Production readiness matrix
    matrix = runner.production_readiness_matrix()
    assert len(matrix) >= 2
    results.ok("Production readiness matrix generated")


# ============================================================================
# Test 11: Policy + Routing Integration
# ============================================================================

async def test_policy_routing_integration():
    print("\n[Test 11] Policy + Routing Integration")

    router = Router()
    engine = PolicyEngine()

    # Scenario: destructive action should block even though routing succeeds
    decision = router.route("Delete all records", family="browser", mode="execute")
    policy = engine.evaluate(
        target="https://admin.example.com",
        action_type="act",
        action_description="delete all records permanently",
        adapter_id=decision.primary_adapter,
        family="browser",
    )
    assert policy.decision == "require_approval", f"Destructive action must require approval, got {policy.decision}"
    results.ok("Destructive action blocked at policy layer")

    # Scenario: experimental adapter blocked by default
    policy = engine.evaluate(
        target="https://example.com",
        action_type="goto",
        adapter_id="browser.cdp",
        adapter_conformance_grade="experimental",
    )
    assert policy.decision == "deny"
    results.ok("Experimental adapter denied without opt-in")


# ============================================================================
# Main
# ============================================================================

def main():
    print("=" * 60)
    print("Allternit Computer Use — End-to-End Test Suite")
    print("=" * 60)

    # Sync tests
    test_core_types()
    test_router()
    test_policy()
    test_sessions()
    test_receipts()
    test_telemetry()
    test_registry()

    # Async tests
    loop = asyncio.new_event_loop()
    loop.run_until_complete(test_adapter_e2e())
    loop.run_until_complete(test_full_pipeline())
    loop.run_until_complete(test_conformance())
    loop.run_until_complete(test_policy_routing_integration())
    loop.close()

    success = results.summary()
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
