use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use solana_account_decoder::parse_token::UiTokenAmount;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
extern crate colored;

use colored::Colorize;
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
use tracing::{debug, error, info, warn};

// Add this import for serde_json
use serde_json;

#[derive(Debug, Clone)]
pub struct TokenBalance {
    pub mint: Pubkey,
    pub ui_amount: f64,
    pub decimals: u8,
    pub amount: u64,
    pub symbol: Option<String>,
    pub name: Option<String>,
}

fn format_token_map(token_map: &HashMap<Pubkey, TokenBalance>) -> String {
    token_map
        .iter()
        .map(|(mint, data)| {
            format!(
                "Mint: <code>{}</code> • Symbol: {} • Amount: {}",
                mint,
                data.symbol.as_deref().unwrap_or("N/A"),
                data.ui_amount
            )
        })
        .collect::<Vec<String>>()
        .join("")
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
    telegram_client: Option<reqwest::Client>,
    telegram_token: Option<String>,
    telegram_chat_id: Option<String>,
}

impl PortfolioTracker {
    pub fn new(rpc_url: String, wallet_address: Pubkey) -> Self {
        let client = RpcClient::new_with_commitment(
            rpc_url,
            CommitmentConfig {
                commitment: CommitmentLevel::Confirmed,
            },
        );

        // Check for Telegram configuration
        let telegram_token = std::env::var("TG_TOKEN").ok();
        let telegram_chat_id = std::env::var("CHAT_ID").ok();

        let telegram_client = if telegram_token.is_some() && telegram_chat_id.is_some() {
            info!("Telegram notifications enabled");
            Some(reqwest::Client::new())
        } else {
            warn!(
                "Telegram notifications disabled. Set TG_TOKEN and CHAT_ID in .env file to enable."
            );
            None
        };

        Self {
            rpc_client: Arc::new(client),
            wallet_address,
            current_snapshot: Arc::new(Mutex::new(None)),
            token_metadata: Arc::new(DashMap::new()),
            price_cache: Arc::new(DashMap::new()),
            decimals_cache: RwLock::new(HashMap::new()),
            telegram_client,
            telegram_token,
            telegram_chat_id,
        }
    }

    /// Send Telegram notification
    async fn send_telegram_notification(&self, message: &str) {
        if let (Some(client), Some(token), Some(chat_id)) = (
            &self.telegram_client,
            &self.telegram_token,
            &self.telegram_chat_id,
        ) {
            let url = format!("https://api.telegram.org/bot{}/sendMessage", token);

            let payload = serde_json::json!({
                "chat_id": chat_id,
                "text": message,
                "parse_mode": "HTML",
                "disable_web_page_preview": true
            });

            match client
                .post(&url)
                .body(payload.to_string())
                .header("Content-Type", "application/json")
                .send()
                .await
            {
                Ok(response) => {
                    let response_status = response.status();
                    if !response_status.is_success() {
                        warn!("Telegram API error: Status {}", response_status);
                        if let Ok(text) = response.text().await {
                            warn!("Telegram API response: {}", text);
                        }
                    } else {
                        debug!("Telegram notification sent successfully");
                    }
                }
                Err(e) => {
                    warn!("Failed to send Telegram notification: {}", e);
                }
            }
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

        // Send Telegram notification for initial portfolio
        let telegram_msg = format!(
            "💰 <b>Portfolio Tracker Started</b>\n\n\
            👛 <b>Wallet:</b> <code>{}</code>\n\
            ⏰ <b>Time:</b> {}\n\
            📊 <b>Total Tokens:</b> {}\n\
            🪙 <b>SOL Balance:</b> ◎{:.4}\n\
            💵 <b>Total Value:</b> ${:.2}\n\n\
            <b>Balances: {:?}</b>\n\n
            🔄 <i>Tracking started successfully!</i>",
            self.wallet_address,
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S"),
            balances.len(),
            sol_balance,
            total_value,
            format_token_map(&balances)
        );

        self.send_telegram_notification(&telegram_msg).await;

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
        // Fetch accounts for both SPL Token and SPL Token-2022 programs
        let filter_spl = TokenAccountsFilter::ProgramId(spl_token::id()); // Standard SPL Token
        let filter_spl_2022 = TokenAccountsFilter::ProgramId(spl_token_2022::id()); // SPL Token-2022

        let mut accounts_spl_2022 = self
            .rpc_client
            .get_token_accounts_by_owner(&self.wallet_address, filter_spl_2022)
            .await?;

        // let accounts_spl = self
        //     .rpc_client
        //     .get_token_accounts_by_owner(&self.wallet_address, filter_spl)
        //     .await?;

        // // Combine results from both filters
        // accounts_spl_2022.extend(accounts_spl);

        let accounts = accounts_spl_2022;

        let mut balances = HashMap::new();
        info!("Found {} token accounts", accounts.len());

        for keyed_account in accounts {
            if let solana_account_decoder::UiAccountData::Json(parsed_account) =
                keyed_account.account.data
            {
                if let Some(info) = parsed_account.parsed.get("info") {
                    if let Ok(token_data) = serde_json::from_value::<UiTokenAccount>(info.clone()) {
                        let token_amount = token_data.token_amount;
                        if let Ok(amount_u64) = token_amount.amount.parse::<u64>() {
                            if amount_u64 > 0 {
                                let mint: Pubkey = token_data.mint.parse()?;

                                let balance = TokenBalance {
                                    mint,
                                    amount: amount_u64,
                                    decimals: token_amount.decimals,
                                    ui_amount: token_amount.ui_amount.unwrap_or(0.0),
                                    symbol: self.get_token_symbol(mint).await,
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

    async fn fetch_metaplex_metadata(&self, _mint: Pubkey) -> anyhow::Result<(String, String)> {
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
    pub async fn start_tracking<F>(
        &self,
        tick_interval_ms: u64,
        mut on_portfolio_change: F,
    ) -> anyhow::Result<()>
    where
        F: FnMut(PortfolioDiff) + Send + 'static,
    {
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

    /// Format changes for Telegram
    pub fn format_for_telegram(&self) -> String {
        let mut lines = Vec::new();

        if !self.added.is_empty() || !self.removed.is_empty() || !self.changes.is_empty() {
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

#[derive(Debug, Clone)]
pub struct TokenChange {
    pub mint: Pubkey,
    pub old_amount: f64,
    pub new_amount: f64,
    pub change: f64,
    pub percentage_change: f64,
}

#[derive(serde::Serialize)]
struct Message {
    channel: String,
    text: String,
    timestamp: Option<String>,
}

// Create a global HTTP client (more efficient)
lazy_static::lazy_static! {
    static ref HTTP_CLIENT: reqwest::Client = reqwest::Client::new();
}

async fn send_message(token_address: String) -> anyhow::Result<()> {
    let message = Message {
        channel: "testpfun".to_string(),
        text: format!("New token detected: {}", token_address),
        // timestamp: chrono::Local::now().format("%H:%M").to_string(),
        timestamp: Some("blank".to_string()),
    };

    let response: reqwest::Response = HTTP_CLIENT
        .post("http://localhost:3000/message")
        .json(&message)
        .send()
        .await?;

    if response.status().is_success() {
        info!("✅ Message sent successfully");
    } else {
        let error_text = response.text().await?;
        warn!("❌ Failed to send message: {}", error_text);
    }

    Ok(())
}

fn send_message_sync(token_address: String) -> anyhow::Result<()> {
    // Create a new runtime for synchronous execution
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(send_message(token_address))
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

        // Check for Telegram configuration
        let telegram_enabled =
            std::env::var("TG_TOKEN").is_ok() && std::env::var("CHAT_ID").is_ok();
        if telegram_enabled {
            info!("Telegram notifications: {}", "ENABLED".green().bold());
        } else {
            info!("Telegram notifications: {}", "DISABLED".yellow().bold());
            info!("Set TG_TOKEN and CHAT_ID in .env file to enable Telegram notifications");
        }

        let wallet_address = Pubkey::from_str(&wallet_address_str)?;

        let tracker = Arc::new(PortfolioTracker::new(rpc_url, wallet_address));

        // Log initial portfolio before starting tracking
        if let Err(e) = tracker.log_initial_portfolio().await {
            error!("Failed to log initial portfolio: {}", e);
        }

        // Clone the tracker for the async task
        let tracker_for_task = tracker.clone();
        let tracker_for_telegram = tracker.clone();

        // Start tracking with 5 second intervals
        tokio::spawn(async move {
            if let Err(e) = tracker_for_task
                .start_tracking(1100, move |diff| {
                    if !diff.is_empty() {
                        // Console output with full mint addresses
                        info!("{}", "Portfolio changes detected:".bold().yellow());
                        info!("{}", "-".repeat(80));

                        if !diff.added.is_empty() {
                            info!("{} NEW TOKENS ADDED:", "▶".green());
                            for added in &diff.added {
                                match send_message_sync(added.mint.to_string()) {
                                    Ok(_) => println!("✅ Message sent successfully!"),
                                    Err(e) => eprintln!("❌ Error sending message: {}", e),
                                }
                                let symbol = added.symbol.as_deref().unwrap_or("Unknown");
                                let name = added.name.as_deref().unwrap_or("Unknown Token");
                                info!("  {} {} ({})", "+".green(), symbol, name);
                                info!("     Mint: <code>{}</code>", added.mint.to_string());
                                info!("     Amount: {:.8}", added.ui_amount);
                                info!(
                                    "     Full Mint Address: {}",
                                    added.mint.to_string().dimmed()
                                );
                                info!("");
                            }
                        }

                        if !diff.removed.is_empty() {
                            info!("{} TOKENS REMOVED:", "▶".red());
                            for removed in &diff.removed {
                                let symbol = removed.symbol.as_deref().unwrap_or("Unknown");
                                let name = removed.name.as_deref().unwrap_or("Unknown Token");
                                info!("  {} {} ({})", "-".red(), symbol, name);
                                info!("     Mint: <code>{}</code>", removed.mint.to_string());
                                info!(
                                    "     Full Mint Address: {}",
                                    removed.mint.to_string().dimmed()
                                );
                                info!("");
                            }
                        }

                        if !diff.changes.is_empty() {
                            info!("{} BALANCE CHANGES:", "▶".yellow());
                            for change in &diff.changes {
                                let change_emoji = if change.change > 0.0 {
                                    "↑".green()
                                } else {
                                    "↓".red()
                                };

                                info!(
                                    "  {} Mint: <code>{}</code>",
                                    change_emoji,
                                    change.mint.to_string()
                                );
                                info!(
                                    "     From: {:.8} → {:.8}",
                                    change.old_amount, change.new_amount
                                );
                                info!(
                                    "     Change: {:+.8} ({:.2}%)",
                                    change.change, change.percentage_change
                                );
                                info!(
                                    "     Full Mint Address: {}",
                                    change.mint.to_string().dimmed()
                                );
                                info!("");
                            }
                        }

                        info!("{}", "=".repeat(80));

                        // Send Telegram notification
                        let telegram_msg = diff.format_for_telegram();
                        if !telegram_msg.is_empty() {
                            let tracker_ref = tracker_for_telegram.clone();
                            tokio::spawn(async move {
                                tracker_ref.send_telegram_notification(&telegram_msg).await;
                            });
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

        // Send shutdown notification
        let shutdown_msg = format!(
            "🛑 <b>Portfolio Tracker Stopped</b>\n\n\
            👛 <b>Wallet:</b> <code>{}</code>\n\
            ⏰ <b>Time:</b> {}\n\n\
            <i>Tracker has been shut down.</i>",
            tracker.wallet_address,
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S")
        );

        tracker.send_telegram_notification(&shutdown_msg).await;

        info!("Shutting down...");

        Ok(())
    })
}
