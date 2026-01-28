use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use solana_account_decoder::parse_token::UiTokenAmount;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use spl_token;
use tokio::sync::Mutex;
use tokio::time::interval;
use tracing::{error, info, warn};
use solana_program::program_pack::Pack;
use spl_token::state::Account as TokenAccount;
use tokio::sync::RwLock; // Recommended for async thread-safety
use spl_token::state::Mint;


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
    price_cache: Arc<DashMap<Pubkey, f64>>, // mint -> USD price
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

    /// Fetch all token accounts for a wallet

    pub async fn fetch_token_balances(&self) -> anyhow::Result<HashMap<Pubkey, TokenBalance>> {

        

        let filters = vec![
            RpcFilterType::DataSize(TokenAccount::LEN as u64), // 165 bytes
            RpcFilterType::Memcmp(Memcmp::new_base58_encoded(
                32, // Offset for 'owner' field in TokenAccount
                &self.wallet_address.to_bytes(),
            )),
        ];

        let config = RpcProgramAccountsConfig {
            filters: Some(filters),
            account_config: RpcAccountInfoConfig {
                encoding: Some(solana_account_decoder::UiAccountEncoding::Base64),
                ..Default::default()
            },
            ..Default::default()
        };

        let accounts = self
            .rpc_client
            .get_program_accounts_with_config(&spl_token::id(), config)
            .await?;

        let mut balances = HashMap::new();

        for (_pubkey, account) in accounts {
            // Direct unpacking of binary data into the TokenAccount struct
            if let Ok(token_account) = TokenAccount::unpack(&account.data) {
                if token_account.amount > 0 {
                    let mint = token_account.mint;
                    
                    // Fetch decimals from Mint account (or use a local cache/registry)
                    let decimals = self.get_mint_decimals(mint).await?; 

                    let balance = TokenBalance {
                        mint,
                        amount: token_account.amount,
                        decimals,
                        ui_amount: (token_account.amount as f64) / 10f64.powi(decimals as i32),
                        symbol: self.get_token_symbol(mint).await,
                        name: self.get_token_name(mint).await,
                    };

                    balances.insert(mint, balance);
                }
            }
        }

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
                self.token_metadata.insert(mint, (symbol.clone(), "".to_string()));
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
                self.token_metadata.insert(mint, ("".to_string(), name.clone()));
                Some(name)
            }
            Err(_) => None,
        }
    }

    async fn fetch_token_metadata(&self, mint: Pubkey) -> anyhow::Result<(String, String)> {
        // Implement metadata fetching from token registry or Metaplex
        // This is a placeholder - implement based on your needs
        Ok(("UNKNOWN".to_string(), "Unknown Token".to_string()))
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

    async fn calculate_total_value(&self, balances: &HashMap<Pubkey, TokenBalance>) -> anyhow::Result<f64> {
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

    async fn fetch_external_price(&self, _mint: Pubkey) -> anyhow::Result<f64> {
        // Implement price fetching from external API
        // Example: Jupiter price API, Birdeye, etc.
        Ok(0.0) // Placeholder
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
        
        // Initial snapshot
        let initial_snapshot = self.take_snapshot().await?;
        *self.current_snapshot.lock().await = Some(initial_snapshot.clone());
        
        info!("Started tracking wallet: {}", self.wallet_address);
        
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
                    info!("Tick completed in {:?}", elapsed);
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

// Custom error type
#[derive(thiserror::Error, Debug)]
pub enum TrackerError {
    #[error("RPC error: {0}")]
    RpcError(#[from] solana_client::client_error::ClientError),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

// Helper function for Pubkey parsing (no longer needed, using std::str::FromStr directly)

fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    dotenvy::dotenv().ok(); 

    tokio::runtime::Runtime::new()?.block_on(async {
        // Use your RPC endpoint (consider using multiple for redundancy)
        let rpc_url = std::env::var("SOLANA_RPC_URL")
            .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());
        
        // Replace with the wallet address you want to track
        let wallet_address_str = std::env::var("WALLET_ADDRESS")
            .unwrap_or_else(|_| "YOUR_WALLET_HERE".to_string());
        
        println!("Wallet: {:?}", wallet_address_str); 

        let wallet_address = Pubkey::from_str(&wallet_address_str)?;
        
        let tracker = Arc::new(PortfolioTracker::new(rpc_url, wallet_address));
        
        // Start tracking with 5 second intervals
        let tracker_clone = tracker.clone();
        tokio::spawn(async move {
            if let Err(e) = tracker_clone
                .start_tracking(5000, |diff| {
                    if !diff.is_empty() {
                        info!("Portfolio changes detected:");
                        
                        for added in &diff.added {
                            info!("  + Added: {} ({}) - {}", 
                                added.symbol.as_deref().unwrap_or("Unknown"),
                                added.mint,
                                added.ui_amount
                            );
                        }
                        
                        for removed in &diff.removed {
                            info!("  - Removed: {} ({})", 
                                removed.symbol.as_deref().unwrap_or("Unknown"),
                                removed.mint
                            );
                        }
                        
                        for change in &diff.changes {
                            info!("  Δ Changed: {} - {} → {} (Δ: {:.4}, {:.2}%)", 
                                change.mint,
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
        
        // Keep the program running
        tokio::signal::ctrl_c().await?;
        info!("Shutting down...");
        
        Ok(())
    })
}