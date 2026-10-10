# Computer Use Domain - Integration Tests

This directory contains integration tests for the computer-use domain that verify end-to-end functionality with real (or properly mocked) dependencies.

## Test Structure

```
tests/integration/
├── conftest.py                      # Shared fixtures and pytest configuration
├── pytest.ini                       # Pytest configuration
├── README.md                        # This file
├── test_browser_automation.py       # Browser automation tests (Playwright/CDP)
├── test_vision_models.py            # Vision model tests (OpenAI/Anthropic)
├── test_full_workflows.py           # End-to-end workflow tests
└── .github/
    └── workflows/
        └── integration-tests.yml    # GitHub Actions CI configuration
```

## Test Markers

All tests use pytest markers for categorization:

- `@pytest.mark.integration` - Integration tests (may require external deps)
- `@pytest.mark.requires_browser` - Needs Playwright/Chrome
- `@pytest.mark.requires_desktop` - Needs native desktop access + permissions
- `@pytest.mark.requires_vision` - Needs OpenAI/Anthropic API keys
- `@pytest.mark.slow` - Slow tests (can skip with `-m "not slow"`)
- `@pytest.mark.asyncio` - Async tests

## Running Tests

### Run all integration tests

```bash
cd domains/computer-use/core/tests/integration
pytest -m integration
```

### Run specific test categories

```bash
# Browser tests only
pytest -m "requires_browser"

# Desktop tests only
pytest -m "requires_desktop"

# Vision tests only
pytest -m "requires_vision"

# Multiple markers
pytest -m "requires_browser and requires_vision"
```

### Run mock tests (no dependencies needed)

```bash
# Mock tests work in CI without real dependencies
pytest test_browser_automation.py::TestBrowserAutomationMock
pytest test_vision_models.py::TestVisionModelsMock
pytest test_full_workflows.py::TestMockWorkflows
```

### Skip slow tests

```bash
pytest -m "integration and not slow"
```

## Dependencies

### Browser Tests

```bash
pip install playwright pytest-asyncio
playwright install chromium
```

### Vision Tests

```bash
pip install openai anthropic
```

Set environment variables:

```bash
export OPENAI_API_KEY="your-openai-key"
export ANTHROPIC_API_KEY="your-anthropic-key"
```

## CI Configuration

The GitHub Actions workflow (`.github/workflows/integration-tests.yml`) runs:

1. **Browser Tests** on Ubuntu with Playwright browsers pre-installed
2. **Desktop Tests** on both macOS and Ubuntu (with virtual display on Ubuntu)
3. **Vision Tests** using mock responses (no API keys in CI)
4. **Workflow Tests** combining multiple components

### CI Behavior

- Tests skip gracefully when dependencies are unavailable
- Mock tests always run to ensure code coverage
- Desktop tests may skip on GitHub Actions due to permission limitations
- Vision tests use mocks in CI; real API tests only run locally with keys

## Test Categories

### Browser Automation (`test_browser_automation.py`)

Tests browser automation with:
- **Playwright**: Full browser automation (navigation, clicking, screenshots)
- **CDP**: Chrome DevTools Protocol (when Chrome available)
- **Mock**: Simulated browser for CI

### Vision Models (`test_vision_models.py`)

Tests vision model integration with:
- **OpenAI**: GPT-4 Vision for image analysis and coordinate extraction
- **Anthropic**: Claude Vision for UI element detection
- **Mock**: Simulated vision responses for CI

### Full Workflows (`test_full_workflows.py`)

End-to-end workflow tests:
- **Browser workflows**: Navigate → Find → Click → Screenshot
- **Hybrid workflows**: Browser + Desktop combined
- **Vision-guided**: Screenshot → Vision analysis → Automation

## Writing New Tests

When adding new integration tests:

1. Use appropriate markers:
   ```python
   import pytest
   
   pytestmark = [
       pytest.mark.integration,
       pytest.mark.requires_browser,  # if browser needed
   ]
   ```

2. Add skip conditions for missing dependencies:
   ```python
   @pytest.mark.skipif(
       not pytest.importorskip("playwright", reason="Playwright not installed"),
       reason="Playwright not installed"
   )
   ```

3. Include mock tests for CI:
   ```python
   class TestMyFeatureMock:
       """Mock tests that work without real dependencies."""
       
       def test_mock_behavior(self):
           """Test with mocks."""
           mock_obj = Mock()
           ...
   ```

4. Add fixtures to `conftest.py` if shared across tests

## Troubleshooting

### Browser tests failing

- Ensure Playwright browsers are installed: `playwright install chromium`
- Check browser dependencies: `playwright install-deps chromium`

### Desktop tests failing

- **macOS**: Grant Screen Recording permission in System Preferences > Security & Privacy
- **Linux**: Ensure X11 is running or use Xvfb for headless testing
- Tests will skip automatically if permissions are not granted

### Vision tests failing

- Set API keys: `export OPENAI_API_KEY="..."`
- Mock tests will run if API keys are not available

## License

These tests are part of the computer-use domain and follow the same license.
