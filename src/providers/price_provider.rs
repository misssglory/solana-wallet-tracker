use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use dashmap::DashMap;
use solana_sdk::pubkey::Pubkey;
use tracing::debug;

use crate::traits::price_provider::PriceProvider;

/// Simple price provider with caching
pub struct SimplePriceProvider {
    price_cache: Arc<DashMap<Pubkey, f64>>,
}

impl SimplePriceProvider {
    /// Create a new simple price provider
    pub fn new() -> Self {
        Self {
            price_cache: Arc::new(DashMap::new()),
        }
    }

    /// Fetch price from external API
    async fn fetch_external_price(&self, mint: &Pubkey) -> anyhow::Result<f64> {
        let mint_str = mint.to_string();

        // Known token prices (for demo purposes)
        let known_prices: HashMap<&str, f64> = [
            ("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", 1.0), // USDC
            ("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", 1.0), // USDT
            ("DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263", 0.0002), // BONK
            ("So11111111111111111111111111111111111111112", 100.0), // Wrapped SOL (placeholder)
        ]
        .iter()
        .cloned()
        .collect();

        if let Some(&price) = known_prices.get(mint_str.as_str()) {
            debug!("Found known price for {}: ${}", mint_str, price);
            return Ok(price);
        }

        // In production, fetch from Jupiter API or other price oracle
        // For now, return 0.0 for unknown tokens
        Ok(0.0)
    }
}

impl Default for SimplePriceProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PriceProvider for SimplePriceProvider {
    async fn get_token_price(&self, mint: &Pubkey) -> Option<f64> {
        // Check cache first
        if let Some(price) = self.price_cache.get(mint) {
            return Some(*price);
        }

        // Fetch from external API
        match self.fetch_external_price(mint).await {
            Ok(price) => {
                self.price_cache.insert(*mint, price);
                Some(price)
            }
            Err(e) => {
                debug!("Failed to fetch price for {}: {}", mint, e);
                None
            }
        }
    }

    async fn get_sol_price(&self) -> Option<f64> {
        // SOL price (in production, fetch from API)
        Some(100.0) // Placeholder
    }

    async fn get_batch_prices(&self, mints: &[Pubkey]) -> Vec<Option<f64>> {
        let mut prices = Vec::with_capacity(mints.len());
        
        // Check cache first
        let mut to_fetch = Vec::new();
        for mint in mints {
            if let Some(price) = self.price_cache.get(mint) {
                prices.push(Some(*price));
            } else {
                prices.push(None);
                to_fetch.push(*mint);
            }
        }

        // Fetch missing prices (in production, batch request)
        for (i, mint) in mints.iter().enumerate() {
            if prices[i].is_none() {
                if let Ok(price) = self.fetch_external_price(mint).await {
                    self.price_cache.insert(*mint, price);
                    prices[i] = Some(price);
                }
            }
        }

        prices
    }
}