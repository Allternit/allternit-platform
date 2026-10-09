#!/usr/bin/env bash
# Allternit Computer Use — Conformance Runner
# Runs the full conformance harness or a specific suite.
#
# Usage:
#   ./scripts/run-conformance.sh            # run all suites via pytest
#   ./scripts/run-conformance.sh e2e        # run unit e2e tests (no real adapters)
#   ./scripts/run-conformance.sh browser    # Suite A (requires Playwright + network)
#   ./scripts/run-conformance.sh desktop    # Suite D (requires display)
#   ./scripts/run-conformance.sh routing    # Suite F (no external deps)
#   ./scripts/run-conformance.sh retrieval  # Suite R (requires Playwright + network)
#   ./scripts/run-conformance.sh hybrid     # Suite H (no external deps)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPUTER_USE="$REPO_ROOT/packages/computer-use"
SUITE="${1:-all}"

echo "═══════════════════════════════════════════════════════════"
echo "  Allternit Computer Use — Conformance Runner"
echo "  Suite: $SUITE"
echo "  Working dir: $COMPUTER_USE"
echo "═══════════════════════════════════════════════════════════"

# Check Python available
if ! command -v python3 &>/dev/null; then
  echo "ERROR: python3 not found"
  exit 1
fi

cd "$COMPUTER_USE"

# Install test dependencies if not present
if ! python3 -c "import pytest" &>/dev/null 2>&1; then
  echo "Installing pytest + pytest-asyncio..."
  pip3 install --quiet pytest pytest-asyncio
fi

# Suite routing
case "$SUITE" in
  all)
    echo ""
    echo "Running: Full e2e test suite (no real adapters needed)"
    python3 tests/test_e2e.py
    echo ""
    echo "Running: Suite F (routing/policy) — no external deps"
    python3 -m pytest tests/ -k "routing or policy" -v 2>/dev/null || true
    ;;
  e2e)
    echo ""
    echo "Running: Unit e2e tests (no real adapters needed)"
    python3 tests/test_e2e.py
    ;;
  browser)
    echo ""
    echo "Running: Suite A — Browser Deterministic (requires Playwright + network)"
    echo "         Adapter: browser.playwright"
    python3 -m pytest tests/ -k "browser_deterministic or suite_a" -v 2>/dev/null || \
    python3 tests/test_real_adapters.py
    ;;
  desktop)
    echo ""
    echo "Running: Suite D — Desktop (requires display + Accessibility permission)"
    echo "         Adapter: desktop.accessibility"
    python3 tests/test_real_adapters.py
    ;;
  routing)
    echo ""
    echo "Running: Suite F — Routing & Policy (no external deps)"
    python3 -c "
import asyncio, sys
sys.path.insert(0, '.')
from conformance.suites import build_suite_f
from conformance import ConformanceRunner

async def run():
    suite = build_suite_f()
    runner = ConformanceRunner()
    runner.register_suite(suite)

    class MockAdapter:
        adapter_id = 'mock'
    result = await runner.run_suite('routing-policy-v1', MockAdapter())
    print(f'Suite F: {result.passed}/{result.total} passed — grade: {result.grade}')
    return result.grade == 'production'

ok = asyncio.run(run())
sys.exit(0 if ok else 1)
"
    ;;
  retrieval)
    echo ""
    echo "Running: Suite R — Retrieval (requires Playwright + network)"
    echo "         Adapter: retrieval.playwright-crawler"
    python3 -c "
import asyncio, sys
sys.path.insert(0, '.')
from conformance.suites import build_suite_r
from conformance import ConformanceRunner

async def run():
    from adapters.retrieval.playwright-crawler import PlaywrightCrawlerAdapter
    adapter = PlaywrightCrawlerAdapter()
    await adapter.initialize()

    suite = build_suite_r()
    runner = ConformanceRunner()
    runner.register_suite(suite)
    result = await runner.run_suite('retrieval-v1', adapter)
    print(f'Suite R: {result.passed}/{result.total} passed — grade: {result.grade}')
    await adapter.close()
    return result.passed == result.total

ok = asyncio.run(run())
sys.exit(0 if ok else 1)
"
    ;;
  hybrid)
    echo ""
    echo "Running: Suite H — Hybrid Orchestrator"
    echo "         Adapter: hybrid.orchestrator"
    python3 -c "
import asyncio, sys
sys.path.insert(0, '.')
from conformance.suites import build_suite_h
from conformance import ConformanceRunner
from adapters.hybrid.orchestrator import HybridOrchestrator

async def run():
    adapter = HybridOrchestrator()
    suite = build_suite_h()
    runner = ConformanceRunner()
    runner.register_suite(suite)
    result = await runner.run_suite('hybrid-v1', adapter)
    print(f'Suite H: {result.passed}/{result.total} passed — grade: {result.grade}')
    await adapter.close()
    return result.passed == result.total

ok = asyncio.run(run())
sys.exit(0 if ok else 1)
"
    ;;
  *)
    echo "Unknown suite: $SUITE"
    echo "Valid options: all, e2e, browser, desktop, routing, retrieval, hybrid"
    exit 1
    ;;
esac

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Done."
echo "═══════════════════════════════════════════════════════════"
