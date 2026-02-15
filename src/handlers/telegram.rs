// In src/handlers/telegram.rs

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

    /// Format initial portfolio message
    fn format_initial_portfolio(&self, diff: &PortfolioDiff) -> String {
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S");
        
        let mut message = format!(
            "🚀 <b>Portfolio Tracker Started</b>\n\n\
             ⏰ <b>Time:</b> {}\n\
             👛 <b>Wallet:</b> <code>{}</code>\n\
             📊 <b>Total Tokens:</b> {}\n\n",
            timestamp,
            self.wallet_address,
            diff.added.len()
        );

        if !diff.added.is_empty() {
            message.push_str("<b>Initial Holdings:</b>\n");
            for (i, token) in diff.added.iter().enumerate().take(10) {
                let symbol = token.symbol.as_deref().unwrap_or("Unknown");
                message.push_str(&format!(
                    "{}. {}: {:.4}\n",
                    i + 1,
                    symbol,
                    token.ui_amount
                ));
                
                // Add mint for first few tokens as reference
                if i < 3 {
                    message.push_str(&format!("   <code>{}</code>\n", token.mint));
                }
            }
            
            if diff.added.len() > 10 {
                message.push_str(&format!("   ... and {} more tokens\n", diff.added.len() - 10));
            }
        }

        message.push_str("\n🔄 <b>Now tracking for changes!</b>");
        
        message
    }
}

#[async_trait]
impl PortfolioEventHandler for TelegramEventHandler {
    async fn handle_portfolio_change(&self, diff: PortfolioDiff) {
        if diff.is_empty() {
            return;
        }

        // Check if this is the initial portfolio (only added tokens, no changes/removals)
        let is_initial = !diff.added.is_empty() && diff.removed.is_empty() && diff.changes.is_empty();
        
        let telegram_msg = if is_initial {
            self.format_initial_portfolio(&diff)
        } else {
            diff.format_for_telegram()
        };
        
        if !telegram_msg.is_empty() {
            self.notifier.send_notification(&telegram_msg).await;
            info!("Sent Telegram notification: {}", if is_initial { "Initial portfolio" } else { "Portfolio change" });
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