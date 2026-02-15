//src/tracker/portfolio_tracker.rs
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use solana_sdk::pubkey::Pubkey;
use tokio::sync::Mutex;
use tracing::{info, error, debug, warn};

use crate::models::{
    token::TokenBalance,
    portfolio::{PortfolioSnapshot, PortfolioDiff, TokenChange},
};
use crate::traits::{
    data_provider::PortfolioDataProvider,
    price_provider::PriceProvider,
    event_handler::{PortfolioEventHandler, TokenEventHandler},
};

/// Main portfolio tracker
pub struct PortfolioTracker {
    wallet_address: Pubkey,
    data_provider: Arc<dyn PortfolioDataProvider>,
    price_provider: Arc<dyn PriceProvider>,
    event_handler: Arc<dyn PortfolioEventHandler>,
    token_event_handlers: Vec<Arc<dyn TokenEventHandler>>,
    token_metadata: Arc<DashMap<Pubkey, (String, String)>>,
    current_snapshot: Arc<Mutex<Option<PortfolioSnapshot>>>,
}

impl PortfolioTracker {
    /// Create a new portfolio tracker
    pub fn new(
        wallet_address: Pubkey,
        data_provider: Arc<dyn PortfolioDataProvider>,
        price_provider: Arc<dyn PriceProvider>,
        event_handler: Arc<dyn PortfolioEventHandler>,
    ) -> Self {
        Self {
            wallet_address,
            data_provider,
            price_provider,
            event_handler,
            token_event_handlers: Vec::new(),
            token_metadata: Arc::new(DashMap::new()),
            current_snapshot: Arc::new(Mutex::new(None)),
        }
    }

    /// Add a token event handler
    pub fn add_token_event_handler(&mut self, handler: Arc<dyn TokenEventHandler>) {
        self.token_event_handlers.push(handler);
    }

    /// Get wallet address
    pub fn wallet_address(&self) -> &Pubkey {
        &self.wallet_address
    }

    /// Log initial portfolio state
    pub async fn log_initial_portfolio(&self) -> anyhow::Result<()> {
        info!("{}", "=".repeat(80));
        info!("INITIAL PORTFOLIO SNAPSHOT");
        info!("{}", "=".repeat(80));

        info!("Wallet Address: {}", self.wallet_address.to_string());
        info!(
            "Timestamp: {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string()
        );

        let balances = self.data_provider.fetch_token_balances(&self.wallet_address).await?;

        if balances.is_empty() {
            info!("No token holdings found in wallet");
            
            // Create and send empty portfolio notification
            let empty_message = self.format_initial_portfolio_message(&balances, 0.0).await;
            self.event_handler.handle_portfolio_change(PortfolioDiff {
                added: vec![],
                removed: vec![],
                changes: vec![],
            }).await; // This will still trigger the handler
            
            return Ok(());
        }

        // Calculate total value
        let mut total_value = 0.0;
        let mut balance_vec: Vec<(&Pubkey, &TokenBalance)> = balances.iter().collect();

        // Sort by token amount (highest first)
        balance_vec.sort_by(|a, b| {
            b.1
                .ui_amount
                .partial_cmp(&a.1.ui_amount)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        info!("");
        info!("TOKEN HOLDINGS:");
        info!("{}", "-".repeat(80));

        for (i, (mint, balance)) in balance_vec.iter().enumerate() {
            let symbol = balance.symbol.as_deref().unwrap_or("UNKNOWN");
            let name = balance.name.as_deref().unwrap_or("Unknown Token");

            // Get token price if available
            let price = self.price_provider.get_token_price(*mint).await;
            let token_value = price.map(|p| balance.ui_amount * p);

            if let Some(value) = token_value {
                total_value += value;
            }

            info!("{}. {} ({})", i + 1, symbol, name);
            info!("   Mint: {}", mint.to_string());
            info!("   Balance: {:.8}", balance.ui_amount);
            info!("   Amount: {} (decimals: {})", balance.amount, balance.decimals);

            if let Some(price_val) = price {
                info!("   Price: ${:.8}", price_val);
                if let Some(value) = token_value {
                    info!("   Value: ${:.4}", value);
                }
            } else {
                info!("   Price: Not available");
            }
            info!("");
        }

        // Summary
        info!("{}", "=".repeat(80));
        info!("PORTFOLIO SUMMARY");
        info!("{}", "-".repeat(80));
        info!("Total Tokens: {}", balances.len());

        let sol_balance = self.data_provider.fetch_sol_balance(&self.wallet_address).await?;
        info!("SOL Balance: ◎{:.8}", sol_balance);

        // Try to get SOL price
        if let Some(sol_price) = self.price_provider.get_sol_price().await {
            let sol_value = sol_balance * sol_price;
            total_value += sol_value;
            info!("SOL Value: ${:.2}", sol_value);
        }

        info!("{}", "-".repeat(80));
        info!("➤ Total Portfolio Value: ${:.2}", total_value);
        info!("{}", "=".repeat(80));

        // Send initial portfolio notification through event handler
        // We create a special diff for initial portfolio
        let initial_diff = PortfolioDiff {
            added: balances.values().cloned().collect(),
            removed: vec![],
            changes: vec![],
        };
        
        self.event_handler.handle_portfolio_change(initial_diff).await;

        Ok(())
    }

    /// Format initial portfolio message for notifications
    async fn format_initial_portfolio_message(&self, balances: &HashMap<Pubkey, TokenBalance>, total_value: f64) -> String {
        let mut message = String::new();
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S");
        
        message.push_str(&format!("🚀 <b>Portfolio Tracker Started</b>\n\n"));
        message.push_str(&format!("⏰ <b>Time:</b> {}\n", timestamp));
        message.push_str(&format!("👛 <b>Wallet:</b> <code>{}</code>\n", self.wallet_address));
        message.push_str(&format!("📊 <b>Total Tokens:</b> {}\n\n", balances.len()));

        if !balances.is_empty() {
            message.push_str("<b>Holdings:</b>\n");
            
            // Sort balances by value (if price available) or amount
            let mut balance_vec: Vec<&TokenBalance> = balances.values().collect();
            
            // Try to sort by USD value if prices available
            let mut balance_with_value: Vec<(f64, &TokenBalance)> = Vec::new();
            for balance in &balance_vec {
                if let Some(price) = self.price_provider.get_token_price(&balance.mint).await {
                    balance_with_value.push((balance.ui_amount * price, balance));
                } else {
                    balance_with_value.push((0.0, balance));
                }
            }
            
            // Sort by value descending
            balance_with_value.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            
            for (value, balance) in balance_with_value.iter().take(10) { // Show top 10
                let symbol = balance.symbol.as_deref().unwrap_or("Unknown");
                if *value > 0.0 {
                    message.push_str(&format!("• {}: {:.4} (${:.2})\n", symbol, balance.ui_amount, value));
                } else {
                    message.push_str(&format!("• {}: {:.4}\n", symbol, balance.ui_amount));
                }
            }
            
            if balances.len() > 10 {
                message.push_str(&format!("  ... and {} more\n", balances.len() - 10));
            }
            message.push_str("\n");
        }

        let sol_balance = match self.data_provider.fetch_sol_balance(&self.wallet_address).await {
            Ok(bal) => bal,
            Err(_) => 0.0,
        };
        
        message.push_str(&format!("🪙 <b>SOL Balance:</b> ◎{:.4}\n", sol_balance));
        message.push_str(&format!("💰 <b>Total Value:</b> ${:.2}\n", total_value));
        message.push_str("\n");
        message.push_str("🔄 <b>Tracking started successfully!</b>");

        message
    }

    /// Take a snapshot of current portfolio
    pub async fn take_snapshot(&self) -> anyhow::Result<PortfolioSnapshot> {
        let balances = self.data_provider.fetch_token_balances(&self.wallet_address).await?;
        
        // Try to calculate USD value
        let total_value = self.calculate_total_value(&balances).await.ok();

        Ok(PortfolioSnapshot {
            timestamp: chrono::Utc::now(),
            wallet_address: self.wallet_address,
            balances,
            total_value_usd: total_value,
        })
    }

    /// Calculate total portfolio value in USD
    async fn calculate_total_value(&self, balances: &HashMap<Pubkey, TokenBalance>) -> anyhow::Result<f64> {
        let mut total = 0.0;

        for (mint, balance) in balances {
            if let Some(price) = self.price_provider.get_token_price(mint).await {
                total += balance.ui_amount * price;
            }
        }

        // Add SOL balance
        let sol_balance = self.data_provider.fetch_sol_balance(&self.wallet_address).await?;
        if let Some(sol_price) = self.price_provider.get_sol_price().await {
            total += sol_balance * sol_price;
        }

        Ok(total)
    }

    /// Compare two snapshots and return differences
    fn compare_snapshots(&self, old: &PortfolioSnapshot, new: &PortfolioSnapshot) -> PortfolioDiff {
        let mut diff = PortfolioDiff::default();

        // Check for new tokens
        for (mint, new_balance) in &new.balances {
            match old.balances.get(mint) {
                Some(old_balance) => {
                    let amount_change = new_balance.ui_amount - old_balance.ui_amount;
                    if amount_change.abs() > f64::EPSILON {
                        diff.changes.push(TokenChange {
                            mint: *mint,
                            old_amount: old_balance.ui_amount,
                            new_amount: new_balance.ui_amount,
                            change: amount_change,
                            percentage_change: if old_balance.ui_amount > 0.0 {
                                (amount_change / old_balance.ui_amount) * 100.0
                            } else {
                                100.0
                            },
                        });
                    }
                }
                None => {
                    diff.added.push(new_balance.clone());
                    
                    // Notify token added handlers
                    for handler in &self.token_event_handlers {
                        let token_address = mint.to_string();
                        let handler_clone = handler.clone();
                        let balance_clone = new_balance.clone();
                        tokio::spawn(async move {
                            handler_clone.on_token_added(&token_address, &balance_clone).await;
                        });
                    }
                }
            }
        }

        // Check for removed tokens
        for (mint, old_balance) in &old.balances {
            if !new.balances.contains_key(mint) {
                diff.removed.push(old_balance.clone());
                
                // Notify token removed handlers
                for handler in &self.token_event_handlers {
                    let token_address = mint.to_string();
                    let handler_clone = handler.clone();
                    let balance_clone = old_balance.clone();
                    tokio::spawn(async move {
                        handler_clone.on_token_removed(&token_address, &balance_clone).await;
                    });
                }
            }
        }

        diff
    }

    /// Start polling-based tracking
    pub async fn start_tracking_polling(&self, tick_interval_ms: u64) -> anyhow::Result<()> {
        info!("Starting polling-based tracking with interval: {}ms", tick_interval_ms);
        
        let mut timedelta = Instant::now();

        // Initial snapshot
        let initial_snapshot = self.take_snapshot().await?;
        *self.current_snapshot.lock().await = Some(initial_snapshot);

        loop {
            let sleep_ms = tick_interval_ms as i128 - timedelta.elapsed().as_millis() as i128;
            if sleep_ms > 0 {
                tokio::time::sleep(Duration::from_millis(sleep_ms as u64)).await;
            }
            timedelta = Instant::now();

            match self.take_snapshot().await {
                Ok(new_snapshot) => {
                    let old_snapshot = self.current_snapshot.lock().await.clone();

                    if let Some(old) = old_snapshot {
                        let diff = self.compare_snapshots(&old, &new_snapshot);

                        if !diff.is_empty() {
                            self.event_handler.handle_portfolio_change(diff).await;
                        }
                    }

                    *self.current_snapshot.lock().await = Some(new_snapshot);
                }
                Err(e) => {
                    self.event_handler.handle_error(&e).await;
                }
            }
        }
    }

    /// Start WebSocket-based tracking
    pub async fn start_tracking_websocket(&self) -> anyhow::Result<()> {
        info!("Starting WebSocket-based real-time tracking");
        
        // Start real-time updates from data provider
        self.data_provider.start_realtime_updates().await?;

        // Keep the tracker running
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

// Implement Clone for PortfolioTracker
impl Clone for PortfolioTracker {
    fn clone(&self) -> Self {
        Self {
            wallet_address: self.wallet_address,
            data_provider: self.data_provider.clone(),
            price_provider: self.price_provider.clone(),
            event_handler: self.event_handler.clone(),
            token_event_handlers: self.token_event_handlers.clone(),
            token_metadata: self.token_metadata.clone(),
            current_snapshot: self.current_snapshot.clone(),
        }
    }
}