use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use solana_account_decoder::parse_token::UiTokenAmount;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
// use solana_client::rpc_request::TokenAccountOpts;
use colored::*;
use solana_account_decoder_client_types::token::UiTokenAccount;
use solana_client::rpc_request::TokenAccountsFilter;
use solana_commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_program::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use spl_token;
use spl_token::state::Account as TokenAccount;
use spl_token::state::Mint;
use spl_token_2022;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, error, info, warn}; // Add colored crate for better output

#[derive(Debug, Clone)]
pub struct TokenBalance {
    pub mint: Pubkey,
    pub ui_amount: f64,
    pub decimals: u8,
    pub amount: u64,
    pub symbol: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PortfolioSnapshot {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub wallet_address: Pubkey,
    pub balances: HashMap<Pubkey, TokenBalance>,
    pub total_value_usd: Option<f64>,
}

pub struct PortfolioTracker {
    rpc_client: Arc<RpcClient>,
    wallet_address: Pubkey,
    current_snapshot: Arc<Mutex<Option<PortfolioSnapshot>>>,
    token_metadata: Arc<DashMap<Pubkey, (String, String)>>, // mint -> (symbol, name)
    price_cache: Arc<DashMap<Pubkey, f64>>,                 // mint -> USD price
    decimals_cache: RwLock<HashMap<Pubkey, u8>>,
}

impl PortfolioTracker {
    pub fn new(rpc_url: String, wallet_address: Pubkey) -> Self {
        let client = RpcClient::new_with_commitment(
            rpc_url,
            CommitmentConfig {
                commitment: CommitmentLevel::Confirmed,
            },
        );

        Self {
            rpc_client: Arc::new(client),
            wallet_address,
            current_snapshot: Arc::new(Mutex::new(None)),
            token_metadata: Arc::new(DashMap::new()),
            price_cache: Arc::new(DashMap::new()),
            decimals_cache: RwLock::new(HashMap::new()),
        }
    }

    /// Log initial portfolio with detailed information
    pub async fn log_initial_portfolio(&self) -> anyhow::Result<()> {
        info!("{}", "=".repeat(80).green());
        info!("{}", "INITIAL PORTFOLIO SNAPSHOT".bold().green());
        info!("{}", "=".repeat(80).green());

        info!("Wallet Address: {}", self.wallet_address.to_string().cyan());
        info!(
            "Timestamp: {}",
            chrono::Utc::now()
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string()
                .cyan()
        );

        let balances = self.fetch_token_balances().await?;

        if balances.is_empty() {
            info!("{}", "No token holdings found in wallet".yellow());
            return Ok(());
        }

        // Calculate total value
        let mut total_value = 0.0;
        let mut balance_vec: Vec<(&Pubkey, &TokenBalance)> = balances.iter().collect();

        // Sort by token amount (highest first)
        balance_vec.sort_by(|a, b| {
            b.1.ui_amount
                .partial_cmp(&a.1.ui_amount)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        info!("");
        info!("{}", "TOKEN HOLDINGS:".bold());
        info!("{}", "-".repeat(80));

        for (i, (mint, balance)) in balance_vec.iter().enumerate() {
            let symbol = balance.symbol.as_deref().unwrap_or("UNKNOWN");
            let name = balance.name.as_deref().unwrap_or("Unknown Token");

            // Get token price if available
            let price = self.get_token_price(**mint).await;
            let token_value = price.map(|p| balance.ui_amount * p);

            if let Some(value) = token_value {
                total_value += value;
            }

            info!("{}. {} ({})", i + 1, symbol.bold(), name);
            info!("   Mint: {}", mint.to_string().dimmed());
            info!("   Balance: {:.8}", balance.ui_amount);
            info!(
                "   Amount: {} (decimals: {})",
                balance.amount, balance.decimals
            );

            if let Some(price_val) = price {
                info!("   Price: ${:.8}", price_val);
                if let Some(value) = token_value {
                    info!("   Value: ${:.4}", value);
                }
            } else {
                info!("   Price: {}", "Not available".dimmed());
            }
            info!("");
        }

        // Summary
        info!("{}", "=".repeat(80));
        info!("{}", "PORTFOLIO SUMMARY".bold().green());
        info!("{}", "-".repeat(80));
        info!("Total Tokens: {}", balances.len());

        let sol_balance = self.fetch_sol_balance().await?;
        info!("SOL Balance: ◎{:.8}", sol_balance);

        // Try to get SOL price
        if let Some(sol_price) = self.get_sol_price().await {
            let sol_value = sol_balance * sol_price;
            total_value += sol_value;
            info!("SOL Value: ${:.2}", sol_value);
        }

        info!("{}", "-".repeat(80));
        info!("{} Total Portfolio Value: ${:.2}", "➤".green(), total_value);
        info!("{}", "=".repeat(80).green());

        Ok(())
    }

    /// Fetch SOL balance
    async fn fetch_sol_balance(&self) -> anyhow::Result<f64> {
        let balance = self.rpc_client.get_balance(&self.wallet_address).await?;
        Ok(balance as f64 / 10f64.powi(9)) // Convert lamports to SOL
    }

    /// Get SOL price (placeholder - implement real price feed)
    async fn get_sol_price(&self) -> Option<f64> {
        // In a real implementation, fetch from price oracle
        // For now, return None or a placeholder
        None
    }

    /// Fetch all token accounts for a wallet
    pub async fn fetch_token_balances(&self) -> anyhow::Result<HashMap<Pubkey, TokenBalance>> {
        let filter = TokenAccountsFilter::ProgramId(spl_token_2022::id());

        let accounts = self
            .rpc_client
            .get_token_accounts_by_owner(&self.wallet_address, filter)
            .await?;

        let mut balances = HashMap::new();
        info!("Found {} token accounts", accounts.len());

        for keyed_account in accounts {
            if let solana_account_decoder::UiAccountData::Json(parsed_account) =
                keyed_account.account.data
            {
                // 1. Get the "info" object out of the parsed data
                if let Some(info) = parsed_account.parsed.get("info") {
                    // 2. Deserialize ONLY the info object into UiTokenAccount
                    if let Ok(token_data) = serde_json::from_value::<UiTokenAccount>(info.clone()) {
                        let token_amount = token_data.token_amount;
                        // ... rest of your logic
                        // println!("{:?}", token_amount);
                        if let Ok(amount_u64) = token_amount.amount.parse::<u64>() {
                            if amount_u64 > 0 {
                                let mint: Pubkey = token_data.mint.parse()?;

                                let balance = TokenBalance {
                                    mint,
                                    amount: amount_u64,
                                    decimals: token_amount.decimals,
                                    ui_amount: token_amount.ui_amount.unwrap_or(0.0),
                                    symbol: self.get_token_symbol(mint).await, // Still needed for metadata
                                    name: self.get_token_name(mint).await,
                                };

                                balances.insert(mint, balance);
                            }
                        }
                    } else {
                        eprintln!("Failed to deserialize info into UiTokenAccount");
                    }
                }
            }
        }

        info!("Found {} tokens with non-zero balance", balances.len());
        Ok(balances)
    }

    pub async fn get_mint_decimals(&self, mint_pubkey: Pubkey) -> anyhow::Result<u8> {
        // 1. Check local cache first to avoid redundant RPC calls
        {
            let cache = self.decimals_cache.read().await;
            if let Some(&decimals) = cache.get(&mint_pubkey) {
                return Ok(decimals);
            }
        }

        // 2. Fetch raw account data from the blockchain
        let data = self.rpc_client.get_account_data(&mint_pubkey).await?;

        // 3. Unpack the data into a Mint struct
        let mint_state = Mint::unpack(&data)
            .map_err(|_| anyhow::anyhow!("Failed to unpack Mint account data"))?;

        let decimals = mint_state.decimals;

        // 4. Update the cache for future use
        {
            let mut cache = self.decimals_cache.write().await;
            cache.insert(mint_pubkey, decimals);
        }

        Ok(decimals)
    }

    async fn get_token_symbol(&self, mint: Pubkey) -> Option<String> {
        if let Some(entry) = self.token_metadata.get(&mint) {
            return Some(entry.value().0.clone());
        }

        // Try to fetch from token list or metadata
        match self.fetch_token_metadata(mint).await {
            Ok((symbol, _)) => {
                self.token_metadata
                    .insert(mint, (symbol.clone(), "".to_string()));
                Some(symbol)
            }
            Err(_) => None,
        }
    }

    async fn get_token_name(&self, mint: Pubkey) -> Option<String> {
        if let Some(entry) = self.token_metadata.get(&mint) {
            return Some(entry.value().1.clone());
        }

        match self.fetch_token_metadata(mint).await {
            Ok((_, name)) => {
                self.token_metadata
                    .insert(mint, ("".to_string(), name.clone()));
                Some(name)
            }
            Err(_) => None,
        }
    }

    async fn fetch_token_metadata(&self, mint: Pubkey) -> anyhow::Result<(String, String)> {
        // Enhanced metadata fetching - try multiple sources
        info!("Fetching metadata for mint: {}", mint);

        // First try to get from known token lists
        if let Ok(metadata) = self.fetch_from_token_list(mint).await {
            return Ok(metadata);
        }

        // Then try Metaplex metadata
        if let Ok(metadata) = self.fetch_metaplex_metadata(mint).await {
            return Ok(metadata);
        }

        // Fallback to mint address
        Ok((
            format!("TOKEN_{:.4}", &mint.to_string()[0..8]),
            "Unknown Token".to_string(),
        ))
    }

    async fn fetch_from_token_list(&self, mint: Pubkey) -> anyhow::Result<(String, String)> {
        // Implement token list fetching (e.g., from Jupiter token list)
        // This is a simplified version
        let mint_str = mint.to_string();

        // Check known tokens
        let known_tokens: HashMap<&str, (&str, &str)> = [
            (
                "So11111111111111111111111111111111111111112",
                ("SOL", "Wrapped SOL"),
            ),
            (
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                ("USDC", "USD Coin"),
            ),
            (
                "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",
                ("USDT", "USD Tether"),
            ),
        ]
        .iter()
        .cloned()
        .collect();

        if let Some((symbol, name)) = known_tokens.get(mint_str.as_str()) {
            return Ok((symbol.to_string(), name.to_string()));
        }

        Err(anyhow::anyhow!("Token not in known list"))
    }

    async fn fetch_metaplex_metadata(&self, mint: Pubkey) -> anyhow::Result<(String, String)> {
        // Placeholder for Metaplex metadata fetching
        Err(anyhow::anyhow!("Metaplex metadata not implemented"))
    }

    /// Take a snapshot of current portfolio
    pub async fn take_snapshot(&self) -> anyhow::Result<PortfolioSnapshot> {
        let balances = self.fetch_token_balances().await?;

        // Optional: Fetch prices for USD valuation
        let total_value = self.calculate_total_value(&balances).await;

        let snapshot = PortfolioSnapshot {
            timestamp: chrono::Utc::now(),
            wallet_address: self.wallet_address,
            balances,
            total_value_usd: total_value.ok(),
        };

        Ok(snapshot)
    }

    async fn calculate_total_value(
        &self,
        balances: &HashMap<Pubkey, TokenBalance>,
    ) -> anyhow::Result<f64> {
        let mut total = 0.0;

        for (mint, balance) in balances {
            if let Some(price) = self.get_token_price(*mint).await {
                total += balance.ui_amount * price;
            }
        }

        Ok(total)
    }

    async fn get_token_price(&self, mint: Pubkey) -> Option<f64> {
        // Check cache first
        if let Some(price) = self.price_cache.get(&mint) {
            return Some(*price);
        }

        // Fetch from price oracle (Jupiter, Birdeye, etc.)
        match self.fetch_external_price(mint).await {
            Ok(price) => {
                self.price_cache.insert(mint, price);
                Some(price)
            }
            Err(_) => None,
        }
    }

    async fn fetch_external_price(&self, mint: Pubkey) -> anyhow::Result<f64> {
        // Implement price fetching from external API
        // Example: Jupiter price API
        let mint_str = mint.to_string();

        // Known token prices (for demo purposes)
        let known_prices: HashMap<&str, f64> = [
            ("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", 1.0), // USDC
            ("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", 1.0), // USDT
        ]
        .iter()
        .cloned()
        .collect();

        if let Some(&price) = known_prices.get(mint_str.as_str()) {
            return Ok(price);
        }

        // For other tokens, return 0.0 (placeholder)
        Ok(0.0)
    }

    /// Compare two snapshots and return differences
    pub fn compare_snapshots(
        &self,
        old: &PortfolioSnapshot,
        new: &PortfolioSnapshot,
    ) -> PortfolioDiff {
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
                }
            }
        }

        // Check for removed tokens
        for (mint, old_balance) in &old.balances {
            if !new.balances.contains_key(mint) {
                diff.removed.push(old_balance.clone());
            }
        }

        diff
    }

    /// Start continuous tracking with configurable interval
    pub async fn start_tracking(
        self: Arc<Self>,
        tick_interval_ms: u64,
        mut on_portfolio_change: impl FnMut(PortfolioDiff) + Send + 'static,
    ) -> anyhow::Result<()> {
        let mut interval = interval(Duration::from_millis(tick_interval_ms));

        // Initial snapshot with logging
        info!("{}", "Taking initial portfolio snapshot...".cyan());
        let initial_snapshot = self.take_snapshot().await?;

        // Log initial portfolio
        if let Err(e) = self.log_initial_portfolio().await {
            error!("Failed to log initial portfolio: {}", e);
        }

        *self.current_snapshot.lock().await = Some(initial_snapshot.clone());

        info!("Started tracking wallet: {}", self.wallet_address);
        info!("Tracking interval: {}ms", tick_interval_ms);

        loop {
            interval.tick().await;

            let start_time = Instant::now();

            match self.take_snapshot().await {
                Ok(new_snapshot) => {
                    let old_snapshot = self.current_snapshot.lock().await.clone();

                    if let Some(old) = old_snapshot {
                        let diff = self.compare_snapshots(&old, &new_snapshot);

                        if !diff.is_empty() {
                            on_portfolio_change(diff);
                        }
                    }

                    *self.current_snapshot.lock().await = Some(new_snapshot);

                    let elapsed = start_time.elapsed();
                    debug!("Tick completed in {:?}", elapsed);
                }
                Err(e) => {
                    error!("Error taking snapshot: {}", e);
                }
            }
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct PortfolioDiff {
    pub added: Vec<TokenBalance>,
    pub removed: Vec<TokenBalance>,
    pub changes: Vec<TokenChange>,
}

impl PortfolioDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changes.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct TokenChange {
    pub mint: Pubkey,
    pub old_amount: f64,
    pub new_amount: f64,
    pub change: f64,
    pub percentage_change: f64,
}

fn main() -> anyhow::Result<()> {
    // Initialize logging with colors
    tracing_subscriber::fmt()
        .with_ansi(true)
        .with_level(true)
        .with_target(true)
        .init();

    dotenvy::dotenv().ok();

    tokio::runtime::Runtime::new()?.block_on(async {
        // Use your RPC endpoint
        let rpc_url = std::env::var("SOLANA_RPC_URL")
            .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());

        // Wallet address to track
        let wallet_address_str = std::env::var("WALLET_ADDRESS").unwrap_or_else(|_| {
            // Example wallet for demo (Raydium LP wallet)
            "5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1".to_string()
        });

        info!("Initializing portfolio tracker...");
        info!("RPC URL: {}", rpc_url);
        info!("Wallet Address: {}", wallet_address_str);

        let wallet_address = Pubkey::from_str(&wallet_address_str)?;

        let tracker = Arc::new(PortfolioTracker::new(rpc_url, wallet_address));

        // Log initial portfolio before starting tracking
        if let Err(e) = tracker.log_initial_portfolio().await {
            error!("Failed to log initial portfolio: {}", e);
        }

        // Start tracking with 5 second intervals
        let tracker_clone = tracker.clone();
        tokio::spawn(async move {
            if let Err(e) = tracker_clone
                .start_tracking(5000, |diff| {
                    if !diff.is_empty() {
                        info!("{}", "Portfolio changes detected:".bold().yellow());

                        for added in &diff.added {
                            info!(
                                "  {} Added: {} ({}) - {:.8}",
                                "+".green(),
                                added.symbol.as_deref().unwrap_or("Unknown"),
                                added.mint.to_string()[0..8].to_string().dimmed(),
                                added.ui_amount
                            );
                        }

                        for removed in &diff.removed {
                            info!(
                                "  {} Removed: {} ({})",
                                "-".red(),
                                removed.symbol.as_deref().unwrap_or("Unknown"),
                                removed.mint.to_string()[0..8].to_string().dimmed()
                            );
                        }

                        for change in &diff.changes {
                            let change_emoji = if change.change > 0.0 {
                                "↑".green()
                            } else {
                                "↓".red()
                            };

                            info!(
                                "  {} Changed: {} - {:.8} → {:.8} (Δ: {:+.8}, {:.2}%)",
                                change_emoji,
                                change.mint.to_string()[0..8].to_string().dimmed(),
                                change.old_amount,
                                change.new_amount,
                                change.change,
                                change.percentage_change
                            );
                        }
                    }
                })
                .await
            {
                error!("Tracking error: {}", e);
            }
        });

        info!("Portfolio tracker is running. Press Ctrl+C to stop.");

        // Keep the program running
        tokio::signal::ctrl_c().await?;
        info!("Shutting down...");

        Ok(())
    })
}
