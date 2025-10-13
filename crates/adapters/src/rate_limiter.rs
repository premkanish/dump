// crates/adapters/src/rate_limiter.rs
use std::sync::Arc;
use tokio::sync::Semaphore;
use std::time::{Duration, Instant};
use parking_lot::Mutex;

/// Token bucket rate limiter
pub struct RateLimiter {
    tokens: Arc<Mutex<TokenBucket>>,
    semaphore: Arc<Semaphore>,
}

struct TokenBucket {
    capacity: usize,
    available: f64,
    refill_rate: f64, // tokens per second
    last_refill: Instant,
}

impl RateLimiter {
    pub fn new(capacity: usize, refill_per_sec: f64) -> Self {
        Self {
            tokens: Arc::new(Mutex::new(TokenBucket {
                capacity,
                available: capacity as f64,
                refill_rate: refill_per_sec,
                last_refill: Instant::now(),
            })),
            semaphore: Arc::new(Semaphore::new(capacity)),
        }
    }
    
    /// Acquire a token, waiting if necessary
    pub async fn acquire(&self) -> RateLimitGuard {
        // Refill tokens based on elapsed time
        {
            let mut bucket = self.tokens.lock();
            let now = Instant::now();
            let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
            
            let new_tokens = elapsed * bucket.refill_rate;
            bucket.available = (bucket.available + new_tokens).min(bucket.capacity as f64);
            bucket.last_refill = now;
        }
        
        // Wait for semaphore
        let permit = self.semaphore.clone().acquire_owned().await.unwrap();
        
        RateLimitGuard {
            _permit: permit,
            tokens: self.tokens.clone(),
        }
    }
    
    /// Try to acquire without waiting
    pub fn try_acquire(&self) -> Option<RateLimitGuard> {
        // Refill first
        {
            let mut bucket = self.tokens.lock();
            let now = Instant::now();
            let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
            
            let new_tokens = elapsed * bucket.refill_rate;
            bucket.available = (bucket.available + new_tokens).min(bucket.capacity as f64);
            bucket.last_refill = now;
            
            if bucket.available < 1.0 {
                return None;
            }
        }
        
        let permit = self.semaphore.clone().try_acquire_owned().ok()?;
        
        Some(RateLimitGuard {
            _permit: permit,
            tokens: self.tokens.clone(),
        })
    }
}

pub struct RateLimitGuard {
    _permit: tokio::sync::OwnedSemaphorePermit,
    tokens: Arc<Mutex<TokenBucket>>,
}

impl Drop for RateLimitGuard {
    fn drop(&mut self) {
        let mut bucket = self.tokens.lock();
        bucket.available = (bucket.available - 1.0).max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_rate_limiter() {
        let limiter = RateLimiter::new(10, 5.0);
        
        // Should acquire immediately
        let _guard1 = limiter.acquire().await;
        let _guard2 = limiter.acquire().await;
        
        drop(_guard1);
        drop(_guard2);
        
        // Wait for refill
        tokio::time::sleep(Duration::from_millis(200)).await;
        
        let _guard3 = limiter.acquire().await;
    }
}