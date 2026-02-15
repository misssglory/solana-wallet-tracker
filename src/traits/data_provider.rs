use std::collections::HashMap;
use async_trait::async_trait;
use solana_sdk::pubkey::Pubkey;
use crate::models::token::TokenBalance;

/// Core trait for fetching portfolio data
#[async_trait]
pub trait PortfolioDataProvider: Send + Sync {
    /// Fetch SOL balance for a wallet
    async fn fetch_sol_balance(&self, wallet: &Pubkey) -> anyhow::Result<f64>;
    
    /// Fetch all token balances for a wallet
    async fn fetch_token_balances(&self, wallet: &Pubkey) -> anyhow::Result<HashMap<Pubkey, TokenBalance>>;
    
    /// Get decimals for a mint
    async fn get_mint_decimals(&self, mint: &Pubkey) -> anyhow::Result<u8>;
    
    /// Optional: Start real-time updates (for WebSocket providers)
    async fn start_realtime_updates(&self) -> anyhow::Result<()> {
        // Default implementation does nothing
        Ok(())
    }
}