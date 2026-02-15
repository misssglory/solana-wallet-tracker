use std::collections::HashMap;
use solana_sdk::pubkey::Pubkey;
use chrono::{DateTime, Utc};
use super::token::TokenBalance;

/// Snapshot of portfolio at a specific time
#[derive(Debug, Clone)]
pub struct PortfolioSnapshot {
    pub timestamp: DateTime<Utc>,
    pub wallet_address: Pubkey,
    pub balances: HashMap<Pubkey, TokenBalance>,
    pub total_value_usd: Option<f64>,
}

impl PortfolioSnapshot {
    /// Create a new snapshot
    pub fn new(
        wallet_address: Pubkey,
        balances: HashMap<Pubkey, TokenBalance>,
        total_value_usd: Option<f64>,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            wallet_address,
            balances,
            total_value_usd,
        }
    }

    /// Get balance for a specific mint
    pub fn get_balance(&self, mint: &Pubkey) -> Option<&TokenBalance> {
        self.balances.get(mint)
    }

    /// Check if snapshot is empty (no tokens)
    pub fn is_empty(&self) -> bool {
        self.balances.is_empty()
    }

    /// Number of tokens in portfolio
    pub fn token_count(&self) -> usize {
        self.balances.len()
    }
}

/// Difference between two portfolio snapshots
#[derive(Debug, Default, Clone)]
pub struct PortfolioDiff {
    pub added: Vec<TokenBalance>,
    pub removed: Vec<TokenBalance>,
    pub changes: Vec<TokenChange>,
}

impl PortfolioDiff {
    /// Create a new empty diff
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if there are any changes
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changes.is_empty()
    }

    /// Format changes for Telegram
    pub fn format_for_telegram(&self) -> String {
        let mut lines = Vec::new();
        let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S");

        if !self.is_empty() {
            lines.push(format!("⏰ <b>{}</b>", timestamp));
            lines.push("🔔 <b>Portfolio Changes Detected</b>".to_string());
        }

        if !self.added.is_empty() {
            lines.push("\n➕ <b>New Tokens Added:</b>".to_string());
            for added in &self.added {
                let symbol = added.symbol.as_deref().unwrap_or("Unknown");
                lines.push(format!(
                    "• {} ({})\n  Amount: {:.8}\n  Mint: <code>{}</code>",
                    symbol,
                    added.name.as_deref().unwrap_or("Unknown Token"),
                    added.ui_amount,
                    added.mint
                ));
            }
        }

        if !self.removed.is_empty() {
            lines.push("\n➖ <b>Tokens Removed:</b>".to_string());
            for removed in &self.removed {
                let symbol = removed.symbol.as_deref().unwrap_or("Unknown");
                lines.push(format!(
                    "• {} ({})\n  Mint: <code>{}</code>",
                    symbol,
                    removed.name.as_deref().unwrap_or("Unknown Token"),
                    removed.mint
                ));
            }
        }

        if !self.changes.is_empty() {
            lines.push("\n📈 <b>Balance Changes:</b>".to_string());
            for change in &self.changes {
                let change_emoji = if change.change > 0.0 { "📈" } else { "📉" };
                let change_sign = if change.change > 0.0 { "+" } else { "" };
                lines.push(format!(
                    "• {} Mint: <code>{}</code>\n  From: {:.8} → {:.8}\n  Change: {}{:.8} ({:.2}%)",
                    change_emoji,
                    change.mint,
                    change.old_amount,
                    change.new_amount,
                    change_sign,
                    change.change,
                    change.percentage_change
                ));
            }
        }

        lines.join("\n")
    }
}

/// Change in a specific token balance
#[derive(Debug, Clone)]
pub struct TokenChange {
    pub mint: Pubkey,
    pub old_amount: f64,
    pub new_amount: f64,
    pub change: f64,
    pub percentage_change: f64,
}

impl TokenChange {
    /// Create a new token change
    pub fn new(mint: Pubkey, old_amount: f64, new_amount: f64) -> Self {
        let change = new_amount - old_amount;
        let percentage_change = if old_amount > 0.0 {
            (change / old_amount) * 100.0
        } else {
            100.0
        };

        Self {
            mint,
            old_amount,
            new_amount,
            change,
            percentage_change,
        }
    }
}