// crates/universe/src/scoring.rs
use common::AssetMetrics;

/// Crypto asset scorer
pub struct CryptoScorer {
    liquidity_weight: f64,
    volume_weight: f64,
    onchain_weight: f64,
    social_weight: f64,
    funding_weight: f64,
}

impl CryptoScorer {
    pub fn new() -> Self {
        Self {
            liquidity_weight: 0.25,
            volume_weight: 0.30,
            onchain_weight: 0.20,
            social_weight: 0.15,
            funding_weight: 0.10,
        }
    }
    
    pub fn score(&self, metrics: &AssetMetrics) -> f64 {
        let liquidity_score = self.normalize_liquidity(metrics.liquidity_usd);
        let volume_score = self.normalize_volume(metrics.volume_24h_usd);
        let onchain_score = self.normalize_onchain(metrics.tx_count_1h.unwrap_or(0));
        let social_score = self.normalize_social(metrics.social_mentions_24h.unwrap_or(0));
        let funding_score = self.normalize_funding(metrics.funding_rate_bps.unwrap_or(0.0));
        
        liquidity_score * self.liquidity_weight
            + volume_score * self.volume_weight
            + onchain_score * self.onchain_weight
            + social_score * self.social_weight
            + funding_score * self.funding_weight
    }
    
    fn normalize_liquidity(&self, liq: f64) -> f64 {
        // Log scale normalization
        if liq < 1.0 {
            return 0.0;
        }
        ((liq.ln() - 13.0) / 8.0).clamp(0.0, 1.0) // ln(500k) ~ 13, ln(1B) ~ 21
    }
    
    fn normalize_volume(&self, vol: f64) -> f64 {
        if vol < 1.0 {
            return 0.0;
        }
        ((vol.ln() - 13.8) / 9.0).clamp(0.0, 1.0) // ln(1M) ~ 13.8, ln(1B) ~ 20.7
    }
    
    fn normalize_onchain(&self, tx_count: u64) -> f64 {
        // Higher tx count indicates activity
        let score = (tx_count as f64).ln() / 10.0;
        score.clamp(0.0, 1.0)
    }
    
    fn normalize_social(&self, mentions: u64) -> f64 {
        let score = (mentions as f64).ln() / 8.0;
        score.clamp(0.0, 1.0)
    }
    
    fn normalize_funding(&self, funding_bps: f64) -> f64 {
        // Prefer moderate funding rates (extreme rates = risk)
        let abs_funding = funding_bps.abs();
        if abs_funding < 10.0 {
            1.0
        } else if abs_funding < 50.0 {
            0.5
        } else {
            0.0
        }
    }
}

/// Equity asset scorer
pub struct EquityScorer {
    liquidity_weight: f64,
    volume_weight: f64,
    short_interest_weight: f64,
    options_weight: f64,
    news_weight: f64,
    fundamentals_weight: f64,
}

impl EquityScorer {
    pub fn new() -> Self {
        Self {
            liquidity_weight: 0.30,
            volume_weight: 0.25,
            short_interest_weight: 0.15,
            options_weight: 0.15,
            news_weight: 0.10,
            fundamentals_weight: 0.05,
        }
    }
    
    pub fn score(&self, metrics: &AssetMetrics) -> f64 {
        let liquidity_score = self.normalize_liquidity(metrics.liquidity_usd);
        let volume_score = self.normalize_volume(metrics.volume_24h_usd);
        let short_score = self.normalize_short_interest(metrics.short_interest_pct.unwrap_or(0.0));
        let options_score = self.normalize_options(metrics.options_volume.unwrap_or(0));
        let news_score = self.normalize_news(metrics.analyst_rating.unwrap_or(0.0));
        let fundamental_score = self.normalize_volatility(metrics.volatility_30d.unwrap_or(0.0));
        
        liquidity_score * self.liquidity_weight
            + volume_score * self.volume_weight
            + short_score * self.short_interest_weight
            + options_score * self.options_weight
            + news_score * self.news_weight
            + fundamental_score * self.fundamentals_weight
    }
    
    fn normalize_liquidity(&self, liq: f64) -> f64 {
        if liq < 1.0 {
            return 0.0;
        }
        ((liq.ln() - 18.0) / 8.0).clamp(0.0, 1.0)
    }
    
    fn normalize_volume(&self, vol: f64) -> f64 {
        if vol < 1.0 {
            return 0.0;
        }
        ((vol.ln() - 18.0) / 8.0).clamp(0.0, 1.0)
    }
    
    fn normalize_short_interest(&self, si: f64) -> f64 {
        // High short interest can indicate opportunity
        if si < 5.0 {
            0.3
        } else if si < 15.0 {
            0.7
        } else if si < 30.0 {
            1.0
        } else {
            0.5 // Too high = risky
        }
    }
    
    fn normalize_options(&self, volume: u64) -> f64 {
        if volume == 0 {
            return 0.0;
        }
        ((volume as f64).ln() / 15.0).clamp(0.0, 1.0)
    }
    
    fn normalize_news(&self, rating: f64) -> f64 {
        (rating / 5.0).clamp(0.0, 1.0)
    }
    
    fn normalize_volatility(&self, vol: f64) -> f64 {
        // Prefer moderate volatility
        if vol < 0.2 {
            vol / 0.2 * 0.5
        } else if vol < 0.6 {
            1.0
        } else {
            (1.0 - (vol - 0.6) / 0.4).max(0.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_crypto_scoring() {
        let scorer = CryptoScorer::new();
        
        let metrics = AssetMetrics {
            volume_24h_usd: 10_000_000.0,
            liquidity_usd: 5_000_000.0,
            funding_rate_bps: Some(5.0),
            open_interest_usd: Some(50_000_000.0),
            tx_count_1h: Some(1000),
            social_mentions_24h: Some(500),
            ..Default::default()
        };
        
        let score = scorer.score(&metrics);
        assert!(score > 0.0 && score <= 1.0);
    }
    
    #[test]
    fn test_equity_scoring() {
        let scorer = EquityScorer::new();
        
        let metrics = AssetMetrics {
            volume_24h_usd: 100_000_000.0,
            liquidity_usd: 500_000_000.0,
            market_cap_usd: Some(10_000_000_000.0),
            short_interest_pct: Some(15.0),
            options_volume: Some(10000),
            analyst_rating: Some(4.0),
            volatility_30d: Some(0.35),
            ..Default::default()
        };
        
        let score = scorer.score(&metrics);
        assert!(score > 0.0 && score <= 1.0);
    }
}