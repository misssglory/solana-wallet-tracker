use async_trait::async_trait;
use solana_sdk::pubkey::Pubkey;
use tracing::info;

use crate::traits::event_handler::PortfolioEventHandler;
use crate::models::portfolio::PortfolioDiff;
use crate::telegram_notifier::TelegramNotifier;

/// Telegram notification event handler
pub struct TelegramEventHandler {
    notifier: TelegramNotifier,
    wallet_address: Pubkey,
}

impl TelegramEventHandler {
    /// Create a new Telegram event handler
    pub fn new(notifier: TelegramNotifier, wallet_address: Pubkey) -> Self {
        Self { notifier, wallet_address }
    }

    /// Check if Telegram is enabled
    pub fn is_enabled(&self) -> bool {
        self.notifier.is_enabled()
    }
}

#[async_trait]
impl PortfolioEventHandler for TelegramEventHandler {
    async fn handle_portfolio_change(&self, diff: PortfolioDiff) {
        if diff.is_empty() {
            return;
        }

        let telegram_msg = diff.format_for_telegram();
        if !telegram_msg.is_empty() {
            self.notifier.send_notification(&telegram_msg).await;
        }
    }

    async fn handle_error(&self, error: &anyhow::Error) {
        let error_msg = format!(
            "❌ <b>Portfolio Tracker Error</b>\n\n\
             👛 <b>Wallet:</b> <code>{}</code>\n\
             ⚠️ <b>Error:</b> {}",
            self.wallet_address, error
        );
        self.notifier.send_notification(&error_msg).await;
    }
}