use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Token bucket shared by every connection of one or several downloads.
pub struct Limiter {
    inner: Mutex<Bucket>,
}

struct Bucket {
    rate: Option<u64>,
    tokens: f64,
    last: Instant,
}

impl Limiter {
    pub fn new(rate: Option<u64>) -> Self {
        Self { inner: Mutex::new(Bucket { rate, tokens: 0.0, last: Instant::now() }) }
    }

    pub fn set_rate(&self, rate: Option<u64>) {
        self.inner.lock().unwrap().rate = rate;
    }

    pub fn rate(&self) -> Option<u64> {
        self.inner.lock().unwrap().rate
    }

    /// Waits until `n` bytes may be consumed.
    pub async fn acquire(&self, n: usize) {
        let wait = {
            let mut b = self.inner.lock().unwrap();
            let Some(rate) = b.rate.filter(|r| *r > 0) else { return };
            let now = Instant::now();
            let rate = rate as f64;
            // At most one second of burst.
            b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate).min(rate);
            b.last = now;
            b.tokens -= n as f64;
            if b.tokens >= 0.0 {
                return;
            }
            Duration::from_secs_f64(-b.tokens / rate)
        };
        tokio::time::sleep(wait).await;
    }
}
