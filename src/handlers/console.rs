use async_trait::async_trait;
use tracing::info;

use crate::traits::event_handler::PortfolioEventHandler;
use crate::models::portfolio::PortfolioDiff;

/// Console logging event handler
pub struct ConsoleEventHandler;

impl ConsoleEventHandler {
    /// Create a new console event handler
    pub fn new() -> Self {
        Self
    }
}

impl Default for ConsoleEventHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PortfolioEventHandler for ConsoleEventHandler {
    async fn handle_portfolio_change(&self, diff: PortfolioDiff) {
        if diff.is_empty() {
            return;
        }

        info!("Portfolio changes detected:");
        info!("{}", "-".repeat(80));

        for added in &diff.added {
            let symbol = added.symbol.as_deref().unwrap_or("Unknown");
            let name = added.name.as_deref().unwrap_or("Unknown Token");
            info!("  + {} ({})", symbol, name);
            info!("     Mint: {}", added.mint.to_string());
            info!("     Amount: {:.8}", added.ui_amount);
            info!("");
        }

        for removed in &diff.removed {
            let symbol = removed.symbol.as_deref().unwrap_or("Unknown");
            let name = removed.name.as_deref().unwrap_or("Unknown Token");
            info!("  - {} ({})", symbol, name);
            info!("     Mint: {}", removed.mint.to_string());
            info!("");
        }

        if !diff.changes.is_empty() {
            info!("  Balance Changes:");
            for change in &diff.changes {
                let change_indicator = if change.change > 0.0 { "↑" } else { "↓" };
                info!(
                    "    {} Mint: {}",
                    change_indicator,
                    change.mint.to_string()
                );
                info!(
                    "       From: {:.8} → {:.8}",
                    change.old_amount, change.new_amount
                );
                info!(
                    "       Change: {:+.8} ({:.2}%)",
                    change.change, change.percentage_change
                );
                info!("");
            }
        }

        info!("{}", "=".repeat(80));
    }

    async fn handle_error(&self, error: &anyhow::Error) {
        info!("Portfolio tracker error: {}", error);
    }
}