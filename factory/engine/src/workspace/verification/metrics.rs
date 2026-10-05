//! Visual Verification Metrics
//!
//! Prometheus-compatible metrics for observability

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Global metrics collector for visual verification
pub struct VerificationMetrics {
    /// Total number of verification requests
    requests_total: AtomicU64,
    
    /// Number of successful verifications
    successes_total: AtomicU64,
    
    /// Number of failed verifications
    failures_total: AtomicU64,
    
    /// Number of timeout failures
    timeouts_total: AtomicU64,
    
    /// Total duration of all verifications (ms)
    duration_ms_total: AtomicU64,
    
    /// Histogram of confidence scores (buckets: 0.0, 0.25, 0.5, 0.75, 0.9, 1.0)
    confidence_buckets: Mutex<HashMap<&'static str, AtomicU64>>,
    
    /// Count by provider type
    provider_requests: Mutex<HashMap<String, AtomicU64>>,
    
    /// Current in-flight verifications
    in_flight: AtomicU64,
}

impl VerificationMetrics {
    pub fn new() -> Self {
        let mut confidence_buckets = HashMap::new();
        confidence_buckets.insert("le_0.25", AtomicU64::new(0));
        confidence_buckets.insert("le_0.50", AtomicU64::new(0));
        confidence_buckets.insert("le_0.75", AtomicU64::new(0));
        confidence_buckets.insert("le_0.90", AtomicU64::new(0));
        confidence_buckets.insert("le_1.00", AtomicU64::new(0));
        
        Self {
            requests_total: AtomicU64::new(0),
            successes_total: AtomicU64::new(0),
            failures_total: AtomicU64::new(0),
            timeouts_total: AtomicU64::new(0),
            duration_ms_total: AtomicU64::new(0),
            confidence_buckets: Mutex::new(confidence_buckets),
            provider_requests: Mutex::new(HashMap::new()),
            in_flight: AtomicU64::new(0),
        }
    }
    
    /// Record a verification request started
    pub fn request_started(&self, provider_type: &str) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        
        let mut providers = self.provider_requests.lock().unwrap();
        providers
            .entry(provider_type.to_string())
            .or_insert_with(AtomicU64::new)
            .fetch_add(1, Ordering::Relaxed);
    }
    
    /// Record a successful verification
    pub fn request_success(&self, duration_ms: u64, confidence: f64) {
        self.successes_total.fetch_add(1, Ordering::Relaxed);
        self.duration_ms_total.fetch_add(duration_ms, Ordering::Relaxed);
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
        
        // Record confidence bucket
        let bucket = if confidence <= 0.25 {
            "le_0.25"
        } else if confidence <= 0.50 {
            "le_0.50"
        } else if confidence <= 0.75 {
            "le_0.75"
        } else if confidence <= 0.90 {
            "le_0.90"
        } else {
            "le_1.00"
        };
        
        let buckets = self.confidence_buckets.lock().unwrap();
        if let Some(counter) = buckets.get(bucket) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        
        // Also increment all higher buckets (cumulative histogram)
        for (key, counter) in buckets.iter() {
            if key >= &bucket {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    
    /// Record a failed verification
    pub fn request_failed(&self, duration_ms: u64, is_timeout: bool) {
        self.failures_total.fetch_add(1, Ordering::Relaxed);
        self.duration_ms_total.fetch_add(duration_ms, Ordering::Relaxed);
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
        
        if is_timeout {
            self.timeouts_total.fetch_add(1, Ordering::Relaxed);
        }
    }
    
    /// Get current metrics snapshot
    pub fn snapshot(&self) -> MetricsSnapshot {
        let requests = self.requests_total.load(Ordering::Relaxed);
        let successes = self.successes_total.load(Ordering::Relaxed);
        let failures = self.failures_total.load(Ordering::Relaxed);
        let total_duration = self.duration_ms_total.load(Ordering::Relaxed);
        
        MetricsSnapshot {
            requests_total: requests,
            successes_total: successes,
            failures_total: failures,
            success_rate: if requests > 0 {
                successes as f64 / requests as f64
            } else {
                0.0
            },
            average_duration_ms: if (successes + failures) > 0 {
                total_duration / (successes + failures)
            } else {
                0
            },
            in_flight: self.in_flight.load(Ordering::Relaxed),
            timeouts_total: self.timeouts_total.load(Ordering::Relaxed),
        }
    }
    
    /// Export metrics in Prometheus format
    pub fn prometheus_format(&self) -> String {
        let mut output = String::new();
        
        // Counters
        output.push_str(&format!(
            "# HELP visual_verification_requests_total Total verification requests\n"
        ));
        output.push_str(&format!(
            "# TYPE visual_verification_requests_total counter\n"
        ));
        output.push_str(&format!(
            "visual_verification_requests_total {}\n",
            self.requests_total.load(Ordering::Relaxed)
        ));
        
        output.push_str(&format!(
            "# HELP visual_verification_successes_total Successful verifications\n"
        ));
        output.push_str(&format!(
            "# TYPE visual_verification_successes_total counter\n"
        ));
        output.push_str(&format!(
            "visual_verification_successes_total {}\n",
            self.successes_total.load(Ordering::Relaxed)
        ));
        
        output.push_str(&format!(
            "# HELP visual_verification_failures_total Failed verifications\n"
        ));
        output.push_str(&format!(
            "# TYPE visual_verification_failures_total counter\n"
        ));
        output.push_str(&format!(
            "visual_verification_failures_total{}\n",
            self.failures_total.load(Ordering::Relaxed)
        ));
        
        // Gauge
        output.push_str(&format!(
            "# HELP visual_verification_in_flight Current in-flight verifications\n"
        ));
        output.push_str(&format!(
            "# TYPE visual_verification_in_flight gauge\n"
        ));
        output.push_str(&format!(
            "visual_verification_in_flight {}\n",
            self.in_flight.load(Ordering::Relaxed)
        ));
        
        output
    }
}

impl Default for VerificationMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Metrics snapshot for reporting
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub requests_total: u64,
    pub successes_total: u64,
    pub failures_total: u64,
    pub success_rate: f64,
    pub average_duration_ms: u64,
    pub in_flight: u64,
    pub timeouts_total: u64,
}

/// Global metrics instance
lazy_static::lazy_static! {
    pub static ref GLOBAL_METRICS: VerificationMetrics = VerificationMetrics::new();
}

/// Convenience function to record request start
pub fn record_request_started(provider_type: &str) {
    GLOBAL_METRICS.request_started(provider_type);
}

/// Convenience function to record success
pub fn record_request_success(duration_ms: u64, confidence: f64) {
    GLOBAL_METRICS.request_success(duration_ms, confidence);
}

/// Convenience function to record failure
pub fn record_request_failed(duration_ms: u64, is_timeout: bool) {
    GLOBAL_METRICS.request_failed(duration_ms, is_timeout);
}

/// Get metrics snapshot
pub fn get_metrics() -> MetricsSnapshot {
    GLOBAL_METRICS.snapshot()
}

/// Get Prometheus-formatted metrics
pub fn get_prometheus_metrics() -> String {
    GLOBAL_METRICS.prometheus_format()
}
