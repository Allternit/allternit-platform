# Allternit Computer Use Observability

Adapter-agnostic session recording, replay, and analysis for browser/desktop automation.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Action Execution                         │
│                         │                                    │
│    ┌────────────────────┼────────────────────┐              │
│    │              Recorder                    │              │
│    │  ┌──────────────┐  ┌──────────────┐     │              │
│    │  │ Before Frame │→ │ After Frame  │     │              │
│    │  │ + Screenshot │  │ + Screenshot │     │              │
│    │  └──────────────┘  └──────────────┘     │              │
│    │                    ↓                     │              │
│    │              Timeline Storage            │              │
│    └────────────────────┼────────────────────┘              │
│                         ↓                                    │
│              ┌─────────────────────┐                         │
│              │   Run Finalization  │                         │
│              │  (async, post-run)  │                         │
│              └─────────────────────┘                         │
│                         ↓                                    │
│         ┌───────────────┼───────────────┐                   │
│         ↓               ↓               ↓                   │
│    ┌─────────┐    ┌──────────┐    ┌──────────┐             │
│    │   GIF   │    │  WebM    │    │ Analysis │             │
│    │ Builder │    │  Builder │    │          │             │
│    └─────────┘    └──────────┘    └──────────┘             │
│                                                         │
└─────────────────────────────────────────────────────────────┘
```

## Components

### 1. Recorder (`recorder/`)

**CU-030: Event Schema** | **CU-031: Frame Capture**

Captures action execution with before/after screenshots.

```python
from observability.recorder import SessionRecorder

recorder = SessionRecorder(config)

# Before action
frame = await recorder.start_frame(
    run_id="run_123",
    session_id="sess_456",
    step_index=0,
    action="click",
    target="#submit",
    before_screenshot=screenshot_bytes,
)

# After action
await recorder.complete_frame(
    frame=frame,
    status="completed",
    after_screenshot=screenshot_bytes,
)
```

**Features:**
- Async frame capture
- Screenshot storage (filesystem or memory)
- Timeline management
- Zero-impact execution (failures don't break actions)

### 2. Replay (`replay/`)

**CU-032: GIF/WebM Builder**

Builds replay artifacts from recorded frames.

```python
from observability.replay import MultiFormatBuilder, ReplayConfig, ReplayFormat

config = ReplayConfig(
    formats=[ReplayFormat.GIF, ReplayFormat.WEBM],
    gif_fps=2,
    video_fps=10,
)
builder = MultiFormatBuilder(config)

artifacts = await builder.build_from_frames(
    frames=[(0, "/path/to/ss1.png", "Step 1"), ...],
    output_dir="/tmp/replays",
    run_id="run_123",
)
# Returns: {"gif": "/path/to/run_123.gif", "webm": "/path/to/run_123.webm"}
```

**Output Formats:**
- **GIF**: Lightweight, shareable, good for quick review
- **WebM**: Better quality, smaller than GIF for long sequences
- **MP4**: Universal compatibility
- **Contact Sheet**: Grid of all screenshots for debugging
- **Timeline JSON**: Structured data for custom players

### 3. Analyzer (`analyzer/`)

**CU-033: Run Analyzer**

Analyzes runs for performance issues and failure patterns.

```python
from observability.analyzer import RunAnalyzer

analyzer = RunAnalyzer()
result = await analyzer.analyze(timeline)

print(result.summary)
# "✓ Run completed successfully: 5 steps | Total time: 12.3s"

for issue in result.performance_issues:
    print(f"Slow step {issue.step_index}: {issue.description}")

if result.is_golden_path_candidate:
    print(f"Golden path score: {result.golden_path_score}")
```

**Analysis Output:**
- Timeline summary
- Performance issues (slow steps >5s)
- Failure patterns with categories
- Suggested fixes
- Golden path candidacy
- Planner tips for system prompt

### 4. Cookbook (`cookbook/`)

**CU-034: Cookbook Promotion**

Promotes successful runs to reusable examples.

```python
from observability.cookbook import CookbookPromoter, CookbookRepository

repository = CookbookRepository("/path/to/cookbook")
promoter = CookbookPromoter(repository)

if promoter.should_promote(timeline, analysis):
    entry = await promoter.promote(
        timeline=timeline,
        analysis=analysis,
        task_description="Fill out contact form",
        example_prompt="Go to example.com and fill the contact form",
        task_category="form_filling",
        tags=["forms", "contact"],
    )
```

**Cookbook Entry Contains:**
- Task description and category
- Action sequence (normalized)
- Replay artifacts (GIF/WebM)
- Example prompt for planner training
- System prompt addition
- Failure patterns to avoid

## Gateway Integration

The gateway automatically records actions when observability is enabled:

```bash
# Enable observability
export Allternit_ENABLE_OBSERVABILITY=true
export Allternit_RECORDINGS_PATH=/var/lib/allternit/recordings

# Start gateway
python -m uvicorn main:app --host 127.0.0.1 --port 8080
```

### API Endpoints

**Execute Action:**
```bash
POST /v1/execute
# Automatically records frames with screenshots
```

**Finalize Run:**
```bash
POST /v1/finalize?run_id=run_123&build_replay=true
# Generates replay GIF and analysis
```

**Get Recording:**
```bash
GET /v1/recordings/{run_id}
# Returns timeline metadata and replay paths
```

## Usage Example

```python
import httpx

run_id = "my_run_001"

# Execute actions
for action in ["goto", "click", "fill"]:
    httpx.post("http://localhost:8080/v1/execute", json={
        "action": action,
        "session_id": "session_001",
        "run_id": run_id,
        "target": "...",
    })

# Finalize and generate artifacts
result = httpx.post(
    f"http://localhost:8080/v1/finalize?run_id={run_id}&build_replay=true"
).json()

print(f"Steps: {result['steps']}")
print(f"Replay: {result['replay_artifacts'].get('gif')}")
print(f"Analysis: {result['analysis']['summary']}")
```

## Configuration

| Environment Variable | Default | Description |
|---------------------|---------|-------------|
| `Allternit_ENABLE_OBSERVABILITY` | `true` | Enable/disable recording |
| `Allternit_RECORDINGS_PATH` | `/tmp/allternit-recordings` | Storage directory |
| `Allternit_COMPUTER_USE_URL` | `http://localhost:8080` | Gateway URL |

## Storage Structure

```
Allternit_RECORDINGS_PATH/
├── 2026-03-14/
│   ├── run_abc123/
│   │   ├── timeline.json
│   │   ├── summary.json
│   │   ├── frames/
│   │   │   ├── frame_001_before.png
│   │   │   ├── frame_001_after.png
│   │   │   └── frame_001.json
│   │   └── analysis.json
│   └── run_def456/
│       └── ...
└── replays/
    ├── run_abc123.gif
    └── run_abc123.webm
```

## Testing

```bash
# Run all observability tests
cd packages/computer-use/gateway
python3 tests/test_observability.py

# Run conformance tests
python3 tests/run_conformance.py
```

## Dependencies

**Required:**
- Python 3.10+
- Pydantic 2.0+

**Optional (for replay):**
- Pillow (PIL) - for GIF generation
- ffmpeg - for WebM/MP4 generation
- imageio - alternative video generation

Install optional dependencies:
```bash
pip install Pillow imageio imageio-ffmpeg
brew install ffmpeg  # macOS
```

## Design Principles

1. **Adapter-Agnostic**: Works with Playwright, CDP, extension, desktop
2. **Zero-Impact**: Recording failures don't break execution
3. **Async Post-Processing**: Replay generation happens after run
4. **Truth vs Replay**: Receipts/timeline are source of truth, replay is derived
5. **Privacy-Aware**: No sensitive data in recordings (passwords filtered)

## Future Work

- [ ] Real-time streaming for live sessions
- [ ] WebSocket feed for progress tracking
- [ ] Integration with cloud storage (S3)
- [ ] Automatic cookbook promotion
- [ ] Conformance visual assertions
- [ ] Failure prediction from patterns
