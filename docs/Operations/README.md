# Allternit NODE - ENTERPRISE CLOUD DEPLOYMENT

> **Production Readiness Checklist & Deployment Guide for Allternit Node**

This folder contains everything needed to deploy Allternit Node to enterprise cloud environments.

---

## 📁 Folder Contents

| File | Purpose |
|------|---------|
| `README.md` | This file - Enterprise deployment overview |
| `PORT_REGISTRY.md` | Official port assignments for all services |
| `PRODUCTION_DEPLOYMENT_GUIDE.md` | Complete deployment instructions |
| `PRODUCTION_READINESS_TRACKER.md` | Task tracking & action items |
| `PRODUCTION_PREPARATION_SUMMARY.md` | Quick reference summary |
| `service-config.sh` | Centralized port configuration |
| ~~`start-platform.sh`~~ | Retired — moved to `archive/bin-legacy/`; use `pnpm dev:platform-stack` from the repo root |

---

## 🎯 Enterprise Deployment Checklist

### Phase 1: Pre-Deployment Preparation

#### Infrastructure Requirements
- [ ] **Compute**: Minimum 4 CPU cores, 16GB RAM (8 cores, 32GB recommended)
- [ ] **Storage**: 100GB SSD minimum, with 500GB+ for production workloads
- [ ] **Network**: Static IP address, firewall rules configured
- [ ] **OS**: Ubuntu 22.04 LTS or later, macOS 14+ (development)
- [ ] **Region**: Multi-region deployment planned (if required)

#### Security Requirements
- [ ] SSL/TLS certificates provisioned (Let's Encrypt or commercial)
- [ ] Firewall rules configured (only required ports exposed)
- [ ] SSH key-based authentication enabled
- [ ] Secrets management solution ready (AWS Secrets Manager, HashiCorp Vault)
- [ ] Security audit completed
- [ ] Penetration testing scheduled

#### Compliance Requirements (Enterprise)
- [ ] Data residency requirements documented
- [ ] GDPR compliance verified (if applicable)
- [ ] SOC 2 requirements addressed
- [ ] Audit logging enabled
- [ ] Data retention policies defined
- [ ] Backup and recovery procedures documented

---

### Phase 2: Code Readiness

#### Critical Fixes (Must Complete)
- [ ] **Issue #1**: All services use environment variables for ports
- [ ] **Issue #2**: Database migration system implemented
- [ ] **Issue #3**: Health check endpoints on all services
- [ ] **Issue #4**: Structured logging (JSON format) enabled
- [ ] **Issue #5**: Graceful shutdown handlers implemented

#### Testing Requirements
- [ ] Unit tests passing (>80% coverage)
- [ ] Integration tests passing
- [ ] Load testing completed (target: 1000 req/s per node)
- [ ] Failover testing completed
- [ ] Disaster recovery tested
- [ ] Security scan passed (no critical vulnerabilities)

#### Documentation Requirements
- [ ] API documentation generated and published
- [ ] Architecture diagrams up to date
- [ ] Runbooks created for common operations
- [ ] Incident response plan documented
- [ ] On-call rotation schedule created

---

### Phase 3: Deployment

#### Environment Setup
```bash
# 1. Clone repository
git clone https://github.com/your-org/allternit.git
cd allternit/8-cloud/allternit-production-ready

# 2. Configure environment
cp .env.example .env.production
# Edit .env.production with production values

# 3. Validate configuration
./service-config.sh
./validate-config.sh

# 4. Build all services
cargo build --release --workspace
npm run build --prefix 4-services/gateway/agui-gateway
npm run build --prefix 4-services/gateway/a2a-gateway

# 5. Deploy (start-platform.sh retired → archive/bin-legacy/)
pnpm dev:platform-stack
```

#### Service Verification
```bash
# Verify all services are running
./verify-deployment.sh

# Expected output:
# ✅ API (port 3000) - Healthy
# ✅ Kernel (port 3004) - Healthy
# ✅ Gateway (port 8013) - Healthy
# ✅ Memory (port 3200) - Healthy
# ✅ Registry (port 8080) - Healthy
# ✅ All services healthy!
```

---

### Phase 4: Post-Deployment

#### Monitoring Setup
- [ ] Prometheus metrics endpoint accessible
- [ ] Grafana dashboards imported
- [ ] Alert rules configured
- [ ] Log aggregation enabled (ELK stack or similar)
- [ ] Distributed tracing enabled (Jaeger/Zipkin)

#### Performance Validation
- [ ] P95 latency < 100ms
- [ ] P99 latency < 500ms
- [ ] Error rate < 0.1%
- [ ] Throughput meets requirements
- [ ] Resource utilization within limits

#### Security Validation
- [ ] All endpoints require authentication
- [ ] Rate limiting enabled and tested
- [ ] CORS properly configured
- [ ] Security headers present
- [ ] No sensitive data in logs

---

## 🏗️ Allternit Node Architecture

### Single Node Deployment
```
┌─────────────────────────────────────────────────┐
│              Load Balancer                       │
│              (Port 443/80)                       │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│              Allternit Node                            │
│  ┌──────────────────────────────────────────┐   │
│  │         Gateway (8013)                    │   │
│  │  - SSL Termination                        │   │
│  │  - Authentication                         │   │
│  │  - Rate Limiting                          │   │
│  └─────────────┬────────────────────────────┘   │
│                │                                 │
│  ┌─────────────▼────────────────────────────┐   │
│  │         API Service (3000)                │   │
│  │  - Business Logic                         │   │
│  │  - Request Routing                        │   │
│  └─────────────┬────────────────────────────┘   │
│                │                                 │
│  ┌─────────────▼────────────────────────────┐   │
│  │         Internal Services                 │   │
│  │  ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐    │   │
│  │  │Kernel│ │Memory│ │Policy│ │Regist│    │   │
│  │  │:3004 │ │:3200 │ │:3003 │ │:8080 │    │   │
│  │  └──────┘ └──────┘ └──────┘ └──────┘    │   │
│  │                                           │   │
│  │  ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐    │   │
│  │  │Voice │ │WebVM │ │Opertr│ │Rails │    │   │
│  │  │:8001 │ │:8002 │ │:3010 │ │:3011 │    │   │
│  │  └──────┘ └──────┘ └──────┘ └──────┘    │   │
│  └──────────────────────────────────────────┘   │
│                                                  │
│  Database: /var/lib/allternit/allternit.db           │
│  Logs: /var/log/allternit/                             │
└─────────────────────────────────────────────────┘
```

### Multi-Node Cluster Deployment
```
┌──────────────────────────────────────────────────────────┐
│                    Global Load Balancer                   │
│                    (DNS-based Routing)                    │
└────────────┬─────────────────────────────┬────────────────┘
             │                             │
             ▼                             ▼
    ┌─────────────────┐           ┌─────────────────┐
    │   Region 1      │           │   Region 2      │
    │   (US-East)     │           │   (EU-West)     │
    └────────┬────────┘           └────────┬────────┘
             │                             │
    ┌────────▼────────┐           ┌────────▼────────┐
    │  Allternit Node 1     │           │  Allternit Node 2     │
    │  - Gateway      │           │  - Gateway      │
    │  - API          │           │  - API          │
    │  - All Services │           │  - All Services │
    └─────────────────┘           └─────────────────┘
             │                             │
    ┌────────▼────────┐           ┌────────▼────────┐
    │  Shared DB      │◄─────────►│  Shared DB      │
    │  (Replicated)   │           │  (Replicated)   │
    └─────────────────┘           └─────────────────┘
```

---

## 📊 Service Port Matrix

| Service | Port | Protocol | External | Production |
|---------|------|----------|----------|------------|
| **Gateway** | 8013 | HTTP/HTTPS | ✅ Yes | ✅ Required |
| **API** | 3000 | HTTP | ❌ No | ✅ Required |
| **Kernel** | 3004 | HTTP | ❌ No | ✅ Required |
| **Memory** | 3200 | HTTP | ❌ No | ✅ Required |
| **Registry** | 8080 | HTTP | ❌ No | ✅ Required |
| **Policy** | 3003 | HTTP | ❌ No | ✅ Required |
| **Voice** | 8001 | HTTP | ❌ No | ⚠️ Optional |
| **WebVM** | 8002 | HTTP | ❌ No | ⚠️ Optional |
| **Operator** | 3010 | HTTP | ❌ No | ⚠️ Optional |
| **Rails** | 3011 | HTTP | ❌ No | ⚠️ Optional |
| **AGUI** | 8010 | WS/HTTP | ❌ No | ⚠️ Optional |
| **A2A** | 8012 | HTTP | ❌ No | ⚠️ Optional |
| **OpenClaw** | 18789 | HTTP | ❌ No | ⚠️ Optional |
| **Terminal** | 4096 | HTTP | ❌ No | ✅ Required |
| **Shell UI** | 5177 | HTTP | ❌ No | ❌ Dev Only |

**Note:** Only Gateway (8013) should be exposed externally. All other services are internal-only.

---

## 🔐 Security Hardening Checklist

### Network Security
- [ ] Only port 443 (HTTPS) and 8013 (Gateway) exposed to internet
- [ ] All internal services bind to 127.0.0.1 only
- [ ] Firewall rules configured (ufw/iptables)
- [ ] DDoS protection enabled
- [ ] Rate limiting configured at load balancer

### Application Security
- [ ] JWT authentication enabled
- [ ] API keys rotated regularly
- [ ] CORS properly configured
- [ ] Input validation on all endpoints
- [ ] SQL injection prevention enabled
- [ ] XSS protection headers set

### Infrastructure Security
- [ ] OS security updates automated
- [ ] Fail2ban or similar installed
- [ ] SSH hardened (no root login, key-only)
- [ ] File integrity monitoring enabled
- [ ] Intrusion detection system (IDS) configured

---

## 📈 Monitoring & Alerting

### Key Metrics to Track

**Infrastructure:**
- CPU usage (alert if >80% for 5min)
- Memory usage (alert if >85% for 5min)
- Disk usage (alert if >80%)
- Network I/O

**Application:**
- Request rate (req/s)
- Error rate (alert if >1% for 5min)
- P95 latency (alert if >500ms)
- P99 latency (alert if >2s)
- Active connections

**Business:**
- Active users
- API calls per tenant
- Feature usage metrics

### Alert Severity Levels

| Severity | Response Time | Examples |
|----------|---------------|----------|
| **P1 - Critical** | Immediate (page) | Service down, data loss, security breach |
| **P2 - High** | 1 hour | High error rate, performance degradation |
| **P3 - Medium** | 4 hours | Resource utilization warnings |
| **P4 - Low** | Next business day | Minor issues, feature requests |

---

## 🔄 Deployment Strategies

### Blue-Green Deployment (Recommended)
```
Production Traffic
       │
       ▼
┌──────────────┐
│ Load Balancer│
└──────┬───────┘
       │
   ┌───┴───┐
   ▼       ▼
┌─────┐ ┌─────┐
│Blue │ │Green│  ← Deploy new version to Green
│(v1) │ │(v2) │  ← Test Green
└─────┘ └─────┘  ← Switch traffic to Green
                 ← Keep Blue as rollback
```

### Rolling Deployment
```
Update instances one at a time:
Instance 1: v1 → v2 ✓
Instance 2: v1 → v2 ✓
Instance 3: v1 → v2 ✓
```

### Canary Deployment
```
Route 5% of traffic to v2:
95% → v1 (Blue)
 5% → v2 (Canary)
 
Monitor metrics, then gradually increase:
50% → v1
50% → v2
```

---

## 📋 Enterprise Readiness Scorecard

Rate your deployment readiness (1-5):

| Category | Score | Notes |
|----------|-------|-------|
| **Infrastructure** | _/5 | Compute, storage, network ready |
| **Security** | _/5 | Auth, encryption, compliance |
| **Monitoring** | _/5 | Metrics, logs, alerts |
| **Testing** | _/5 | Unit, integration, load tests |
| **Documentation** | _/5 | Runbooks, APIs, architecture |
| **Support** | _/5 | On-call, escalation, training |

**Minimum for Production:** 4/5 in all categories

---

## 🚀 Quick Start Commands

### Development
```bash
# From the repo root
pnpm dev:platform-stack
```

### Production Build
```bash
# Build
cargo build --release --workspace

# Deploy
sudo systemctl start allternit-node

# Verify
./verify-deployment.sh
```

### Health Check
```bash
curl http://127.0.0.1:8013/health
```

---

## 📞 Support & Escalation

### Getting Help
- **Documentation:** This folder
- **Issues:** GitHub Issues
- **Emergency:** On-call rotation

### Escalation Path
1. Check logs: `/var/log/allternit/`
2. Review runbooks
3. Contact on-call engineer
4. Escalate to platform team
5. Engage vendor support (if applicable)

---

## 📝 Deployment Log

| Date | Version | Deployed By | Status | Notes |
|------|---------|-------------|--------|-------|
| | | | | |

---

*Last updated: 2026-02-26*
*Version: 1.0.0*
*Classification: Internal Use*
