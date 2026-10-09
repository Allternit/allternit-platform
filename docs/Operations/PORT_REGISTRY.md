# Allternit PLATFORM - PORT REGISTRY

This document defines the official port assignments for all Allternit services.

## Production Port Assignments

### Core Platform Services (3000-3099)
| Port | Service | Path | Protocol | Production Required |
|------|---------|------|----------|---------------------|
| 3000 | API Service | `7-apps/api/` | HTTP | ✅ Yes |
| 3003 | Policy Service | `2-governance/` | HTTP | ✅ Yes |
| 3004 | Kernel Service | `4-services/orchestration/kernel-service/` | HTTP | ✅ Yes |
| 3010 | Operator Service | `4-services/allternit-operator/` | HTTP | ✅ Yes |
| 3011 | Allternit Factory engine | `cmd/allternit-factory/` (`factory/`) | HTTP | ✅ Yes |
| 3090 | Link Card Service | `4-services/infrastructure/link-card-service/` | HTTP | ⚠️ Optional |

### AI/ML Services (8000-8009)
| Port | Service | Path | Protocol | Production Required |
|------|---------|------|----------|---------------------|
| 8001 | Voice Service (TTS) | `4-services/ml-ai-services/voice-service/` | HTTP | ⚠️ Optional |
| 8002 | WebVM Service | `4-services/ai/webvm-service/` | HTTP | ⚠️ Optional |
| ~~8003~~ | ~~Browser Runtime~~ | Retired — both TS duplicates deleted; canonical provider is `infrastructure/chrome-stream/agent-systems/allternit-browser` (ADR-0042) | — | ❌ No |

### Gateway Services (8010-8019)
| Port | Service | Path | Protocol | Production Required |
|------|---------|------|----------|---------------------|
| ~~8010~~ | ~~AGUI Gateway~~ | Retired — package deleted | — | ❌ No |
| ~~8012~~ | ~~A2A Gateway~~ | Retired — package deleted | — | ❌ No |
| 8013 | Main Gateway | `4-services/gateway/src/` | HTTP | ✅ Yes |

### Local Daemons
| Port | Service | Path | Protocol | Production Required |
|------|---------|------|----------|---------------------|
| 7788 | Subscription Gateway (optional TCP; UDS `~/.allternit/subscriptions/gateway.sock` is the default transport, TCP is loopback-only and always token-authed) | `services/subscription-gateway/` | HTTP | ❌ Local Only |

### Infrastructure Services (3200-3299)
| Port | Service | Path | Protocol | Production Required |
|------|---------|------|----------|---------------------|
| 3200 | Memory Service | `4-services/memory/` | HTTP | ✅ Yes |
| 3201 | Allternit Gateway (Test) | `4-services/allternit-gateway/` | HTTP | ❌ Dev Only |

### Registry Services (8080-8089)
| Port | Service | Path | Protocol | Production Required |
|------|---------|------|----------|---------------------|
| 8080 | Registry Service | `4-services/registry/` | HTTP | ✅ Yes |

### Terminal & Shell (4096-4199)
| Port | Service | Path | Protocol | Production Required |
|------|---------|------|----------|---------------------|
| 4096 | Terminal Server | `7-apps/shell/terminal/` | HTTP | ✅ Yes |
| 5177 | Shell UI (Vite) | `7-apps/shell/web/` | HTTP | ⚠️ Dev Only |

### Third-Party Integration
| Port | Service | Path | Protocol | Production Required |
|------|---------|------|----------|---------------------|
| 18789 | OpenClaw Gateway | (npm package) | HTTP | ⚠️ Optional |

---

## Service Dependencies

```
External Traffic
       │
       ▼
┌─────────────────┐
│  Main Gateway   │ Port 8013
│  (4-services/   │
│   gateway/)     │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│   API Service   │ Port 3000
│   (7-apps/api/) │
└────────┬────────┘
         │
    ┌────┴────┬────────────┬───────────┐
    ▼         ▼            ▼           ▼
┌───────┐ ┌───────┐ ┌──────────┐ ┌──────────┐
│Kernel │ │Policy │ │ Memory   │ │Registry  │
│:3004  │ │:3003  │ │ :3200    │ │ :8080    │
└───────┘ └───────┘ └──────────┘ └──────────┘
```

---

## Environment Configuration

### Production (.env.production)
```bash
# Core Services
Allternit_API_URL=http://127.0.0.1:3000
Allternit_KERNEL_URL=http://127.0.0.1:3004
Allternit_POLICY_URL=http://127.0.0.1:3003
Allternit_MEMORY_URL=http://127.0.0.1:3200
Allternit_REGISTRY_URL=http://127.0.0.1:8080

# AI Services
Allternit_VOICE_URL=http://127.0.0.1:8001
Allternit_WEBVM_URL=http://127.0.0.1:8002
Allternit_OPERATOR_URL=http://127.0.0.1:3010
ALLTERNIT_FACTORY_URL=http://127.0.0.1:3011

# Gateways
Allternit_GATEWAY_URL=http://127.0.0.1:8013
# Allternit_AGUI_URL / Allternit_A2A_URL retired — agui/a2a gateway packages deleted

# Terminal
Allternit_TERMINAL_URL=http://127.0.0.1:4096
```

### Development (.env.development)
```bash
# Same as production, plus:
Allternit_SHELL_UI_URL=http://127.0.0.1:5177
```

---

## Conflict Resolution

### Known Conflicts & Solutions

1. **Gateway on 8013** - Multiple gateway implementations
   - **Solution**: Use `4-services/gateway/src/main.py` as main gateway
   - Others (allternit-gateway) use test ports (3296-3299)

2. **Kernel Service** - May not exist as separate binary
   - **Solution**: API service proxies kernel functionality
   - Kernel runs embedded or on 3004

3. **Shell UI Port 5177** - Dev only
   - **Solution**: Production uses static files served by API/Gateway

---

## Production Deployment Checklist

- [ ] All services configured with correct ports
- [ ] Gateway (8013) is single entry point for external traffic
- [ ] Internal services (3000-3099) not exposed externally
- [ ] Firewall rules configured
- [ ] Health check endpoints verified
- [ ] Load balancer configured (if needed)
- [ ] SSL/TLS termination at gateway

---

## Quick Reference

**Minimum Production Services:**
1. Gateway (8013) - External entry point
2. API (3000) - Business logic
3. Kernel (3004) - Agent orchestration
4. Policy (3003) - Security/governance
5. Memory (3200) - State management
6. Registry (8080) - Service discovery

**Optional Services:**
- Voice (8001) - TTS features
- WebVM (8002) - Browser automation
- Operator (3010) - UI automation
- Factory engine (3011) - Agent planning
- ~~AGUI (8010)~~ - Retired (package deleted)
- ~~A2A (8012)~~ - Retired (package deleted)
- OpenClaw (18789) - LLM integration
