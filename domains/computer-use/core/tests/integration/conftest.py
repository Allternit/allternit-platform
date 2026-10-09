"""
Pytest configuration and shared fixtures for integration tests.

This module defines custom markers and shared fixtures for all integration tests.
"""

import pytest


def pytest_configure(config):
    """Configure custom pytest markers."""
    config.addinivalue_line(
        "markers",
        "integration: marks tests as integration tests (may require external dependencies)"
    )
    config.addinivalue_line(
        "markers",
        "requires_browser: marks tests as requiring a browser (Playwright/Chrome)"
    )
    config.addinivalue_line(
        "markers",
        "requires_desktop: marks tests as requiring desktop automation (native desktop + permissions)"
    )
    config.addinivalue_line(
        "markers",
        "requires_vision: marks tests as requiring vision model API keys"
    )
    config.addinivalue_line(
        "markers",
        "slow: marks tests as slow (deselect with '-m \"not slow\"')"
    )


@pytest.fixture(scope="session")
def event_loop():
    """
    Create an instance of the default event loop for the test session.
    Required for async tests.
    """
    import asyncio
    loop = asyncio.get_event_loop_policy().new_event_loop()
    yield loop
    loop.close()


@pytest.fixture(scope="session")
def test_environment():
    """
    Provide information about the test environment.
    """
    import platform
    import sys
    
    return {
        "platform": platform.system(),
        "platform_version": platform.version(),
        "python_version": sys.version,
        "is_ci": bool(__import__('os').getenv("CI")),
    }


@pytest.fixture
def temp_directory(tmp_path):
    """
    Provide a temporary directory for test files.
    """
    return tmp_path


@pytest.fixture(scope="session")
def check_dependencies():
    """
    Check and report which dependencies are available.
    """
    import os
    
    deps = {
        "playwright": False,
        "openai": False,
        "anthropic": False,
        "websockets": False,
        "openai_api_key": bool(os.getenv("OPENAI_API_KEY")),
        "anthropic_api_key": bool(os.getenv("ANTHROPIC_API_KEY")),
    }
    
    try:
        import playwright
        deps["playwright"] = True
    except ImportError:
        pass
    
    try:
        import openai
        deps["openai"] = True
    except ImportError:
        pass
    
    try:
        import anthropic
        deps["anthropic"] = True
    except ImportError:
        pass
    
    try:
        import websockets
        deps["websockets"] = True
    except ImportError:
        pass
    
    return deps
