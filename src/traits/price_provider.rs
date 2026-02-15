use async_trait::async_trait;
use solana_sdk::pubkey::Pubkey;

/// Trait for price feed providers
#[async_trait]
pub trait PriceProvider: Send + Sync {
    /// Get price for a token in USD
    async fn get_token_price(&self, mint: &Pubkey) -> Option<f64>;
    
    /// Get SOL price in USD
    async fn get_sol_price(&self) -> Option<f64>;
    
    /// Get prices for multiple tokens (optimized batch request)
    async fn get_batch_prices(&self, mints: &[Pubkey]) -> Vec<Option<f64>> {
        let mut prices = Vec::with_capacity(mints.len());
        for mint in mints {
            prices.push(self.get_token_price(mint).await);
        }
        prices
    }
}