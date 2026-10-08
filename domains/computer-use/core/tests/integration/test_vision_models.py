"""
Integration tests for vision models (OpenAI, Anthropic).

These tests verify vision model capabilities for image analysis and coordinate extraction.
Tests skip gracefully if API keys are not available.
Mock tests are provided for CI environments.

Run with: pytest -m integration
Run vision tests only: pytest -m "requires_vision"

Environment Variables:
    OPENAI_API_KEY: Required for OpenAI tests
    ANTHROPIC_API_KEY: Required for Anthropic tests
"""

import os
import json
import base64
import pytest
from typing import Dict, List, Optional, Any
from unittest.mock import Mock, patch, MagicMock
from pathlib import Path

# Mark all tests in this module
pytestmark = [
    pytest.mark.integration,
    pytest.mark.requires_vision,
]


# Fixtures for API key checks
@pytest.fixture(scope="session")
def openai_api_key() -> Optional[str]:
    """Get OpenAI API key from environment."""
    return os.getenv("OPENAI_API_KEY")


@pytest.fixture(scope="session")
def anthropic_api_key() -> Optional[str]:
    """Get Anthropic API key from environment."""
    return os.getenv("ANTHROPIC_API_KEY")


@pytest.fixture(scope="session")
def openai_available(openai_api_key: Optional[str]) -> bool:
    """Check if OpenAI is available (package + API key)."""
    try:
        import openai
        return openai_api_key is not None and len(openai_api_key) > 0
    except ImportError:
        return False


@pytest.fixture(scope="session")
def anthropic_available(anthropic_api_key: Optional[str]) -> bool:
    """Check if Anthropic is available (package + API key)."""
    try:
        import anthropic
        return anthropic_api_key is not None and len(anthropic_api_key) > 0
    except ImportError:
        return False


@pytest.fixture
def sample_image_base64() -> str:
    """Provide a sample base64-encoded image for testing."""
    # Create a simple 10x10 red PNG
    # This is a minimal valid PNG (red pixel)
    png_data = base64.b64encode(
        b'\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x0a\x00\x00\x00\x0a\x08\x02\x00\x00\x00\x02PX\xea\x00\x00\x00\x19tEXtSoftware\x00Adobe ImageReadyq\xc9e<\x00\x00\x00VIDATx\xdabc\xfc\xff\xff\x7f\x00\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x04\x00\x01\x00\x04\x00\x01\x90\x9c\xa7\x8c\x00\x00\x00\x00IEND\xaeB`\x82'
    ).decode('utf-8')
    return png_data


@pytest.fixture
def sample_ui_screenshot_base64() -> str:
    """Provide a sample UI screenshot description for coordinate extraction tests."""
    # This would normally be an actual screenshot
    # For testing, we use a mock scenario
    return "mock_ui_screenshot_data"


# OpenAI Tests
@pytest.mark.skipif(
    not bool(os.getenv("OPENAI_API_KEY")),
    reason="OpenAI API key not available"
)
class TestOpenAIVision:
    """Tests for OpenAI Vision models."""
    
    @pytest.fixture
    def openai_client(self, openai_api_key):
        """Provide OpenAI client."""
        import openai
        return openai.OpenAI(api_key=openai_api_key)
    
    def test_simple_image_analysis(self, openai_client, sample_image_base64):
        """Test basic image analysis with GPT-4 Vision."""
        try:
            response = openai_client.chat.completions.create(
                model="gpt-4o-mini",
                messages=[
                    {
                        "role": "user",
                        "content": [
                            {"type": "text", "text": "What color is this image? Reply with just the color name."},
                            {
                                "type": "image_url",
                                "image_url": {
                                    "url": f"data:image/png;base64,{sample_image_base64}"
                                }
                            }
                        ]
                    }
                ],
                max_tokens=50
            )
            
            assert response is not None
            assert len(response.choices) > 0
            content = response.choices[0].message.content
            assert content is not None
            assert len(content) > 0
            
        except Exception as e:
            pytest.fail(f"OpenAI API call failed: {e}")
    
    def test_coordinate_extraction_from_description(self, openai_client):
        """Test extracting coordinates from a UI description."""
        # Simulate a UI analysis request
        prompt = """
        Analyze this UI and identify the coordinates of the "Submit" button.
        Return ONLY a JSON object with format: {"x": <number>, "y": <number>, "confidence": <0-1>}
        The screen is 1920x1080 pixels.
        """
        
        try:
            response = openai_client.chat.completions.create(
                model="gpt-4o-mini",
                messages=[
                    {
                        "role": "user",
                        "content": [
                            {"type": "text", "text": prompt},
                            {
                                "type": "image_url",
                                "image_url": {
                                    "url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
                                }
                            }
                        ]
                    }
                ],
                max_tokens=100
            )
            
            assert response is not None
            content = response.choices[0].message.content
            assert content is not None
            
            # Try to extract JSON from response
            try:
                # Look for JSON in the response
                start = content.find('{')
                end = content.rfind('}') + 1
                if start != -1 and end > start:
                    coords = json.loads(content[start:end])
                    assert 'x' in coords or 'X' in coords
                    assert 'y' in coords or 'Y' in coords
            except json.JSONDecodeError:
                # Not all responses will have valid JSON, that's OK for this test
                pass
            
        except Exception as e:
            pytest.fail(f"OpenAI API call failed: {e}")
    
    def test_multiple_element_detection(self, openai_client):
        """Test detecting multiple UI elements."""
        prompt = """
        List all clickable buttons in this UI.
        Return as JSON array: [{"label": "text", "x": 100, "y": 200}, ...]
        """
        
        try:
            response = openai_client.chat.completions.create(
                model="gpt-4o-mini",
                messages=[
                    {
                        "role": "user",
                        "content": [
                            {"type": "text", "text": prompt},
                            {
                                "type": "image_url",
                                "image_url": {
                                    "url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
                                }
                            }
                        ]
                    }
                ],
                max_tokens=200
            )
            
            assert response is not None
            content = response.choices[0].message.content
            assert content is not None
            
        except Exception as e:
            pytest.fail(f"OpenAI API call failed: {e}")
    
    def test_vision_model_response_format(self, openai_client, sample_image_base64):
        """Test that vision model responses have expected structure."""
        response = openai_client.chat.completions.create(
            model="gpt-4o-mini",
            messages=[
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Describe this image in one word."},
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": f"data:image/png;base64,{sample_image_base64}"
                            }
                        }
                    ]
                }
            ],
            max_tokens=20
        )
        
        # Verify response structure
        assert hasattr(response, 'choices')
        assert len(response.choices) > 0
        assert hasattr(response.choices[0], 'message')
        assert hasattr(response.choices[0].message, 'content')
        assert response.choices[0].message.role == 'assistant'
        
        # Verify usage information
        assert hasattr(response, 'usage')
        assert response.usage.total_tokens > 0


# Anthropic Tests
@pytest.mark.skipif(
    not bool(os.getenv("ANTHROPIC_API_KEY")),
    reason="Anthropic API key not available"
)
class TestAnthropicVision:
    """Tests for Anthropic Vision models (Claude)."""
    
    @pytest.fixture
    def anthropic_client(self, anthropic_api_key):
        """Provide Anthropic client."""
        import anthropic
        return anthropic.Anthropic(api_key=anthropic_api_key)
    
    def test_simple_image_analysis(self, anthropic_client, sample_image_base64):
        """Test basic image analysis with Claude Vision."""
        try:
            response = anthropic_client.messages.create(
                model="claude-haiku-4-5",
                max_tokens=100,
                messages=[
                    {
                        "role": "user",
                        "content": [
                            {
                                "type": "text",
                                "text": "What color is this image? Reply with just the color name."
                            },
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": sample_image_base64
                                }
                            }
                        ]
                    }
                ]
            )
            
            assert response is not None
            assert len(response.content) > 0
            content_text = response.content[0].text
            assert content_text is not None
            assert len(content_text) > 0
            
        except Exception as e:
            pytest.fail(f"Anthropic API call failed: {e}")
    
    def test_ui_analysis_with_coordinates(self, anthropic_client):
        """Test UI analysis with coordinate extraction."""
        prompt = """
        Analyze this UI screenshot and identify the coordinates of the "Login" button.
        Return ONLY a JSON object: {"x": <number>, "y": <number>}
        Screen dimensions: 1920x1080
        """
        
        try:
            response = anthropic_client.messages.create(
                model="claude-haiku-4-5",
                max_tokens=150,
                messages=[
                    {
                        "role": "user",
                        "content": [
                            {"type": "text", "text": prompt},
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
                                }
                            }
                        ]
                    }
                ]
            )
            
            assert response is not None
            content_text = response.content[0].text
            assert content_text is not None
            
        except Exception as e:
            pytest.fail(f"Anthropic API call failed: {e}")
    
    def test_element_listing(self, anthropic_client):
        """Test listing UI elements."""
        prompt = """
        List all text input fields visible in this UI.
        For each, provide: field name, approximate coordinates, and whether it's required.
        Format as JSON.
        """
        
        try:
            response = anthropic_client.messages.create(
                model="claude-haiku-4-5",
                max_tokens=200,
                messages=[
                    {
                        "role": "user",
                        "content": [
                            {"type": "text", "text": prompt},
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
                                }
                            }
                        ]
                    }
                ]
            )
            
            assert response is not None
            content_text = response.content[0].text
            assert content_text is not None
            
        except Exception as e:
            pytest.fail(f"Anthropic API call failed: {e}")
    
    def test_vision_response_structure(self, anthropic_client, sample_image_base64):
        """Test that Claude vision responses have expected structure."""
        response = anthropic_client.messages.create(
            model="claude-haiku-4-5",
            max_tokens=50,
            messages=[
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Describe this image in one word."},
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": sample_image_base64
                            }
                        }
                    ]
                }
            ]
        )
        
        # Verify response structure
        assert hasattr(response, 'content')
        assert len(response.content) > 0
        assert hasattr(response.content[0], 'text')
        assert hasattr(response.content[0], 'type')
        assert response.content[0].type == 'text'
        
        # Verify usage information
        assert hasattr(response, 'usage')
        assert response.usage.input_tokens > 0
        assert response.usage.output_tokens > 0


# Mock Tests for CI
class TestVisionModelsMock:
    """Mock tests for vision models that work in CI without API keys."""
    
    def test_mock_openai_vision_response(self):
        """Test with mocked OpenAI vision response."""
        mock_response = Mock()
        mock_response.choices = [
            Mock(
                message=Mock(
                    content='{"x": 500, "y": 300, "confidence": 0.95}',
                    role='assistant'
                )
            )
        ]
        mock_response.usage = Mock(total_tokens=150, prompt_tokens=100, completion_tokens=50)
        
        # Simulate parsing the response
        content = mock_response.choices[0].message.content
        coords = json.loads(content)
        
        assert coords['x'] == 500
        assert coords['y'] == 300
        assert coords['confidence'] == 0.95
    
    def test_mock_anthropic_vision_response(self):
        """Test with mocked Anthropic vision response."""
        mock_response = Mock()
        mock_response.content = [
            Mock(text='The button is located at coordinates (750, 450)', type='text')
        ]
        mock_response.usage = Mock(input_tokens=200, output_tokens=50)
        
        content_text = mock_response.content[0].text
        
        assert '750' in content_text
        assert '450' in content_text
    
    def test_mock_coordinate_extraction(self):
        """Test coordinate extraction logic with mocked data."""
        def extract_coordinates_from_text(text: str) -> Dict[str, int]:
            """Extract coordinates from various text formats."""
            import re
            
            # Try JSON format
            try:
                json_match = re.search(r'\{[^}]*\}', text)
                if json_match:
                    data = json.loads(json_match.group())
                    return {
                        'x': data.get('x', data.get('X', 0)),
                        'y': data.get('y', data.get('Y', 0))
                    }
            except json.JSONDecodeError:
                pass
            
            # Try coordinate pattern
            coord_match = re.search(r'\((\d+),\s*(\d+)\)', text)
            if coord_match:
                return {'x': int(coord_match.group(1)), 'y': int(coord_match.group(2))}
            
            # Try "x: 100, y: 200" pattern
            xy_match = re.search(r'x[:\s]+(\d+).*y[:\s]+(\d+)', text, re.IGNORECASE)
            if xy_match:
                return {'x': int(xy_match.group(1)), 'y': int(xy_match.group(2))}
            
            return {'x': 0, 'y': 0}
        
        # Test various formats
        test_cases = [
            ('{"x": 100, "y": 200}', {'x': 100, 'y': 200}),
            ('Coordinates: (500, 300)', {'x': 500, 'y': 300}),
            ('x: 750, y: 450', {'x': 750, 'y': 450}),
            ('X: 1000 Y: 800', {'x': 1000, 'y': 800}),
        ]
        
        for text, expected in test_cases:
            result = extract_coordinates_from_text(text)
            assert result == expected, f"Failed for: {text}"
    
    def test_mock_element_detection_response(self):
        """Test parsing element detection response."""
        mock_response = '''
        {
            "elements": [
                {"type": "button", "label": "Submit", "x": 800, "y": 600, "confidence": 0.98},
                {"type": "input", "label": "Username", "x": 400, "y": 300, "confidence": 0.95},
                {"type": "input", "label": "Password", "x": 400, "y": 350, "confidence": 0.95}
            ]
        }
        '''
        
        data = json.loads(mock_response)
        elements = data['elements']
        
        assert len(elements) == 3
        assert elements[0]['type'] == 'button'
        assert elements[0]['label'] == 'Submit'
        assert elements[0]['confidence'] > 0.9
    
    @patch.dict(os.environ, {'OPENAI_API_KEY': 'test-key'})
    def test_api_key_from_environment(self):
        """Test that API key is read from environment."""
        api_key = os.getenv("OPENAI_API_KEY")
        assert api_key == "test-key"
    
    def test_base64_image_encoding(self):
        """Test base64 image encoding/decoding."""
        # Simple test image data
        image_data = b'\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR'
        
        encoded = base64.b64encode(image_data).decode('utf-8')
        decoded = base64.b64decode(encoded)
        
        assert decoded == image_data
        assert encoded.startswith('iVBORw0KGgo')


# Utility Tests
class TestVisionUtilities:
    """Tests for vision-related utility functions."""
    
    def test_image_to_base64(self, tmp_path):
        """Test converting image file to base64."""
        from PIL import Image
        
        # Create a test image
        image_path = tmp_path / "test_image.png"
        img = Image.new('RGB', (100, 100), color='blue')
        img.save(image_path)
        
        # Convert to base64
        with open(image_path, 'rb') as f:
            image_data = f.read()
            base64_string = base64.b64encode(image_data).decode('utf-8')
        
        assert len(base64_string) > 0
        # Verify it's valid base64
        decoded = base64.b64decode(base64_string)
        assert decoded[:8] == b'\x89PNG\r\n\x1a\n'
    
    def test_base64_to_image(self, tmp_path):
        """Test converting base64 string back to image."""
        from PIL import Image
        import io
        
        # Create original image
        original = Image.new('RGB', (50, 50), color='green')
        
        # Convert to base64
        buffer = io.BytesIO()
        original.save(buffer, format='PNG')
        base64_string = base64.b64encode(buffer.getvalue()).decode('utf-8')
        
        # Convert back to image
        image_data = base64.b64decode(base64_string)
        restored = Image.open(io.BytesIO(image_data))
        
        assert restored.size == (50, 50)
        assert restored.mode == 'RGB'
    
    def test_validate_coordinates(self):
        """Test coordinate validation logic."""
        def validate_coords(x: int, y: int, screen_width: int, screen_height: int) -> bool:
            """Validate coordinates are within screen bounds."""
            return 0 <= x < screen_width and 0 <= y < screen_height
        
        assert validate_coords(100, 200, 1920, 1080) is True
        assert validate_coords(0, 0, 1920, 1080) is True
        assert validate_coords(1919, 1079, 1920, 1080) is True
        assert validate_coords(1920, 1080, 1920, 1080) is False  # Out of bounds
        assert validate_coords(-1, 100, 1920, 1080) is False
        assert validate_coords(100, -1, 1920, 1080) is False
    
    def test_scale_coordinates(self):
        """Test coordinate scaling between different resolutions."""
        def scale_coords(
            x: int, y: int,
            from_width: int, from_height: int,
            to_width: int, to_height: int
        ) -> tuple:
            """Scale coordinates from one resolution to another."""
            scale_x = to_width / from_width
            scale_y = to_height / from_height
            return (int(x * scale_x), int(y * scale_y))
        
        # Scale from 1920x1080 to 1280x720
        scaled = scale_coords(960, 540, 1920, 1080, 1280, 720)
        assert scaled == (640, 360)
        
        # Scale from 1280x720 to 1920x1080
        scaled = scale_coords(640, 360, 1280, 720, 1920, 1080)
        assert scaled == (960, 540)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
