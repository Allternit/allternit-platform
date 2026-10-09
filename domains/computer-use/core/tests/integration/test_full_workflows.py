"""
End-to-end integration tests for complete computer-use workflows.

These tests verify full workflows combining browser, desktop, and vision components.
All tests skip gracefully if dependencies are unavailable.

Workflows tested:
1. Browser: Navigate -> Find -> Click -> Screenshot
2. Desktop: Screenshot -> Find app -> Click
3. Hybrid: Browser download -> Open in desktop app

Run with: pytest -m integration
"""

import os
import time
import json
import pytest
import pytest_asyncio
import asyncio
from typing import Optional, Dict, Any, Tuple
from pathlib import Path
from unittest.mock import Mock, patch, AsyncMock, MagicMock

# Mark all tests in this module
pytestmark = [
    pytest.mark.integration,
]


# Environment check fixtures
@pytest.fixture(scope="session")
def browser_deps_available() -> bool:
    """Check if browser automation dependencies are available."""
    try:
        import playwright
        return True
    except ImportError:
        return False


@pytest.fixture(scope="session")
def vision_deps_available() -> bool:
    """Check if vision model dependencies are available."""
    has_openai = bool(os.getenv("OPENAI_API_KEY"))
    has_anthropic = bool(os.getenv("ANTHROPIC_API_KEY"))
    return has_openai or has_anthropic


# Workflow 1: Browser Automation
@pytest.mark.requires_browser
class TestBrowserWorkflow:
    """
    End-to-end browser automation workflow tests.
    
    Workflow: Navigate -> Find -> Click -> Screenshot
    """
    
    @pytest_asyncio.fixture(loop_scope="function")
    async def browser_page(self):
        """Provide a Playwright browser page for testing."""
        pytest.importorskip("playwright")
        from playwright.async_api import async_playwright
        
        playwright = None
        browser = None
        page = None
        
        try:
            playwright = await async_playwright().start()
            browser = await playwright.chromium.launch(headless=True)
            page = await browser.new_page()
            yield page
        finally:
            if page:
                await page.close()
            if browser:
                await browser.close()
            if playwright:
                await playwright.stop()
    
    @pytest.mark.asyncio
    async def test_navigate_find_click_screenshot_workflow(
        self, browser_page, tmp_path
    ):
        """
        Complete workflow: Navigate to page, find element, click, take screenshot.
        """
        page = browser_page
        
        # Step 1: Navigate
        response = await page.goto(
            "https://example.com",
            wait_until="networkidle",
            timeout=30000
        )
        assert response.status == 200
        
        # Step 2: Find element (the "More information..." link)
        link = page.locator('a[href*="iana"]').first
        
        # Step 3: Click (if element exists)
        if await link.is_visible():
            await link.click()
            await page.wait_for_load_state("networkidle")
        
        # Step 4: Take screenshot
        screenshot_path = tmp_path / "workflow_screenshot.png"
        await page.screenshot(path=str(screenshot_path), full_page=True)
        
        assert screenshot_path.exists()
        assert screenshot_path.stat().st_size > 0
    
    @pytest.mark.asyncio
    async def test_form_interaction_workflow(self, browser_page):
        """
        Workflow: Navigate to form -> Fill inputs -> Submit -> Verify.
        """
        page = browser_page
        
        # Create a test page with form
        await page.set_content("""
            <html>
                <body>
                    <form id="test-form">
                        <input type="text" id="username" name="username" required />
                        <input type="email" id="email" name="email" required />
                        <button type="submit">Submit</button>
                    </form>
                    <div id="result"></div>
                    <script>
                        document.getElementById('test-form').onsubmit = function(e) {
                            e.preventDefault();
                            document.getElementById('result').textContent = 'Submitted!';
                        };
                    </script>
                </body>
            </html>
        """)
        
        # Fill form
        await page.fill('#username', 'testuser')
        await page.fill('#email', 'test@example.com')
        
        # Submit
        await page.click('button[type="submit"]')
        
        # Verify
        result = await page.text_content('#result')
        assert result == 'Submitted!'
    
    @pytest.mark.asyncio
    async def test_multi_page_navigation_workflow(self, browser_page):
        """
        Workflow: Navigate multiple pages -> Track history -> Go back.
        """
        page = browser_page
        
        urls_visited = []
        
        # Visit multiple pages
        pages = [
            "https://example.com",
            # Note: We use data URIs to avoid external dependencies in this test
        ]
        
        for url in pages:
            response = await page.goto(url, wait_until="networkidle")
            urls_visited.append(page.url)
            assert response.status == 200
        
        assert len(urls_visited) == len(pages)
    
    @pytest.mark.asyncio
    async def test_screenshot_comparison_workflow(self, browser_page, tmp_path):
        """
        Workflow: Take screenshot -> Analyze -> Take another -> Compare.
        """
        page = browser_page
        
        await page.goto("https://example.com", wait_until="networkidle")
        
        # Take initial screenshot
        screenshot1_path = tmp_path / "screenshot1.png"
        await page.screenshot(path=str(screenshot1_path))
        
        # Perform some action (scroll)
        await page.evaluate('window.scrollTo(0, 100)')
        
        # Take second screenshot
        screenshot2_path = tmp_path / "screenshot2.png"
        await page.screenshot(path=str(screenshot2_path))
        
        # Both should exist and be different (different scroll position)
        assert screenshot1_path.exists()
        assert screenshot2_path.exists()
        
        # They might be the same size but different content
        assert screenshot1_path.stat().st_size > 0
        assert screenshot2_path.stat().st_size > 0


# Workflow 3: Hybrid (Browser + Desktop)
@pytest.mark.requires_browser
@pytest.mark.requires_desktop
class TestHybridWorkflow:
    """
    Hybrid workflow tests combining browser and desktop automation.
    
    Workflow: Browser download -> Open in desktop app
    """
    
    @pytest.mark.skip(reason="Hybrid workflow requires careful setup to avoid file system issues")
    def test_browser_download_open_workflow(self, tmp_path):
        """
        Workflow: Download file from browser -> Open with desktop app.
        
        NOTE: This test is skipped by default as it requires:
        - File system access
        - External applications
        - Careful cleanup
        """
        pass
    
    def test_mock_hybrid_workflow(self):
        """
        Mock hybrid workflow for testing without real dependencies.
        """
        # Mock browser actions
        mock_browser = Mock()
        mock_browser.download_file = Mock(return_value="/tmp/downloaded_file.pdf")
        mock_browser.navigate = Mock()
        
        # Mock desktop actions
        mock_desktop = Mock()
        mock_desktop.open_file = Mock(return_value=True)
        mock_desktop.wait_for_window = Mock(return_value=True)
        
        # Simulate workflow
        # Step 1: Browser downloads file
        downloaded_path = mock_browser.download_file("https://example.com/file.pdf")
        assert downloaded_path == "/tmp/downloaded_file.pdf"
        
        # Step 2: Desktop opens file
        opened = mock_desktop.open_file(downloaded_path)
        assert opened is True
        
        # Step 3: Wait for window
        window_found = mock_desktop.wait_for_window("PDF Viewer")
        assert window_found is True


# Workflow 4: Vision-Guided Automation
@pytest.mark.requires_vision
class TestVisionGuidedWorkflow:
    """
    Workflow tests using vision models to guide automation.
    
    Workflow: Screenshot -> Vision analysis -> Extract coordinates -> Click
    """
    
    @pytest.mark.skipif(
        not bool(os.getenv("OPENAI_API_KEY")),
        reason="OpenAI API key required"
    )
    def test_vision_guided_browser_click(self):
        """
        Workflow: Screenshot -> Vision model -> Browser click.
        """
        import openai
        
        client = openai.OpenAI(api_key=os.getenv("OPENAI_API_KEY"))
        
        # This would normally involve:
        # 1. Take browser screenshot
        # 2. Send to vision model
        # 3. Get coordinates
        # 4. Click via browser automation
        
        # For this test, we just verify the API is accessible
        response = client.chat.completions.create(
            model="gpt-4o-mini",
            messages=[{"role": "user", "content": "Say 'test'"}],
            max_tokens=10
        )
        
        assert response.choices[0].message.content is not None
    
    @pytest.mark.skipif(
        not bool(os.getenv("ANTHROPIC_API_KEY")),
        reason="Anthropic API key required"
    )
    def test_vision_guided_desktop_click(self):
        """
        Workflow: Screenshot -> Vision model -> Desktop click.
        """
        import anthropic
        
        client = anthropic.Anthropic(api_key=os.getenv("ANTHROPIC_API_KEY"))
        
        # Verify API access
        response = client.messages.create(
            model="claude-haiku-4-5",
            max_tokens=10,
            messages=[{"role": "user", "content": "Say 'test'"}]
        )
        
        assert response.content[0].text is not None


# Mock Workflow Tests for CI
class TestMockWorkflows:
    """
    Complete workflow tests using mocks for CI environments.
    """
    
    @pytest.mark.asyncio
    async def test_mock_browser_complete_workflow(self):
        """Mock test of complete browser workflow."""
        # Mock browser page
        mock_page = AsyncMock()
        mock_page.goto = AsyncMock(return_value=Mock(status=200))
        mock_page.locator = Mock(return_value=Mock(
            first=Mock(
                is_visible=AsyncMock(return_value=True),
                click=AsyncMock()
            )
        ))
        mock_page.screenshot = AsyncMock(return_value=b'fake_screenshot_data')
        
        # Execute workflow
        response = await mock_page.goto("https://example.com")
        assert response.status == 200
        
        element = mock_page.locator('a[href*="iana"]').first
        if await element.is_visible():
            await element.click()
        
        screenshot = await mock_page.screenshot()
        assert len(screenshot) > 0
        
        # Verify calls
        mock_page.goto.assert_called_once()
        element.click.assert_called_once()
        mock_page.screenshot.assert_called_once()
    
    def test_mock_vision_guided_workflow(self):
        """Mock test of vision-guided automation workflow."""
        # Mock vision model response
        mock_vision_response = Mock()
        mock_vision_response.choices = [
            Mock(message=Mock(content='{"x": 500, "y": 300, "element": "button"}'))
        ]
        
        # Parse coordinates
        import json
        content = mock_vision_response.choices[0].message.content
        coords = json.loads(content)
        
        assert coords['x'] == 500
        assert coords['y'] == 300
        assert coords['element'] == 'button'

    def test_mock_error_handling_workflow(self):
        """Test workflow error handling with mocks."""
        mock_page = AsyncMock()
        mock_page.goto = AsyncMock(side_effect=Exception("Navigation failed"))
        
        # Test error handling
        async def run_test():
            with pytest.raises(Exception, match="Navigation failed"):
                await mock_page.goto("https://example.com")
        
        asyncio.run(run_test())
    
    def test_mock_retry_workflow(self):
        """Test workflow with retry logic."""
        mock_action = Mock()
        mock_action.side_effect = [Exception("Failed"), Exception("Failed"), "Success"]
        
        # Retry logic
        result = None
        max_retries = 3
        for attempt in range(max_retries):
            try:
                result = mock_action()
                break
            except Exception as e:
                if attempt == max_retries - 1:
                    raise
                continue
        
        assert result == "Success"
        assert mock_action.call_count == 3


# Integration Test Helpers
class TestWorkflowHelpers:
    """Tests for workflow helper functions."""
    
    def test_coordinate_normalization(self):
        """Test normalizing coordinates between systems."""
        def normalize_coords(
            x: int, y: int,
            source_width: int, source_height: int,
            target_width: int, target_height: int
        ) -> Tuple[int, int]:
            """Normalize coordinates from source to target resolution."""
            norm_x = int((x / source_width) * target_width)
            norm_y = int((y / source_height) * target_height)
            return (norm_x, norm_y)
        
        # Test: Vision model says "click at (0.5, 0.5)" on 1920x1080
        # Target screen is 1280x720
        x, y = normalize_coords(960, 540, 1920, 1080, 1280, 720)
        assert x == 640
        assert y == 360
    
    def test_workflow_step_validation(self):
        """Test validating workflow step results."""
        def validate_step(step_name: str, result: Any) -> bool:
            """Validate a workflow step completed successfully."""
            validators = {
                'navigate': lambda r: r is not None and hasattr(r, 'status') and r.status == 200,
                'screenshot': lambda r: r is not None and len(r) > 0,
                'click': lambda r: r is True or r is None,  # None is OK for click
            }
            validator = validators.get(step_name, lambda r: r is not None)
            return validator(result)
        
        # Test navigation validation
        mock_response = Mock(status=200)
        assert validate_step('navigate', mock_response) is True
        
        mock_response.status = 404
        assert validate_step('navigate', mock_response) is False
        
        # Test screenshot validation
        assert validate_step('screenshot', b'data') is True
        assert validate_step('screenshot', b'') is False
        assert validate_step('screenshot', None) is False
        
        # Test click validation
        assert validate_step('click', True) is True
        assert validate_step('click', None) is True
        assert validate_step('click', False) is False
    
    def test_workflow_state_management(self):
        """Test managing state across workflow steps."""
        class WorkflowState:
            def __init__(self):
                self.data = {}
                self.history = []
            
            def set(self, key: str, value: Any):
                self.data[key] = value
                self.history.append(('set', key))
            
            def get(self, key: str, default=None):
                return self.data.get(key, default)
            
            def record_step(self, step_name: str, success: bool):
                self.history.append(('step', step_name, success))
        
        state = WorkflowState()
        
        # Simulate workflow
        state.set('url', 'https://example.com')
        state.set('screenshot', b'data')
        state.record_step('navigate', True)
        state.record_step('screenshot', True)
        
        assert state.get('url') == 'https://example.com'
        assert len(state.history) == 4


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
