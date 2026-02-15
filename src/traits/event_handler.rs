use async_trait::async_trait;
use crate::models::portfolio::PortfolioDiff;

/// Handler for portfolio change events
#[async_trait]
pub trait PortfolioEventHandler: Send + Sync {
    /// Handle portfolio change
    async fn handle_portfolio_change(&self, diff: PortfolioDiff);
    
    /// Handle error - using reference to avoid cloning issues
    async fn handle_error(&self, error: &anyhow::Error);  // Changed to explicit reference
}

/// Handler for token added/removed events
#[async_trait]
pub trait TokenEventHandler: Send + Sync {
    /// Called when a token is added to the portfolio
    async fn on_token_added(&self, token_address: &str, balance: &crate::models::token::TokenBalance);
    
    /// Called when a token is removed from the portfolio
    async fn on_token_removed(&self, token_address: &str, balance: &crate::models::token::TokenBalance);
}