use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use dashmap::DashMap;
use solana_account_decoder_client_types::token::UiTokenAccount;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_request::TokenAccountsFilter;
use solana_commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_program::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use spl_token::state::Mint;
use tokio::sync::RwLock;
use tracing::{info, debug};

use crate::traits::data_provider::PortfolioDataProvider;
use crate::models::token::TokenBalance;

/// RPC-based data provider (polling approach)
pub struct RpcDataProvider {
    rpc_client: Arc<RpcClient>,
    decimals_cache: Arc<RwLock<HashMap<Pubkey, u8>>>,
}

impl RpcDataProvider {
    /// Create a new RPC data provider
    pub fn new(rpc_url: String) -> Self {
        let client = RpcClient::new_with_commitment(
            rpc_url,
            CommitmentConfig { commitment: CommitmentLevel::Processed },
        );

        Self {
            rpc_client: Arc::new(client),
            decimals_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get RPC client (for internal use)
    pub fn rpc_client(&self) -> &RpcClient {
        &self.rpc_client
    }
}

#[async_trait]
impl PortfolioDataProvider for RpcDataProvider {
    async fn fetch_sol_balance(&self, wallet: &Pubkey) -> anyhow::Result<f64> {
        let balance = self.rpc_client.get_balance(wallet).await?;
        Ok(balance as f64 / 10f64.powi(9))
    }

    async fn fetch_token_balances(&self, wallet: &Pubkey) -> anyhow::Result<HashMap<Pubkey, TokenBalance>> {
        // Fetch accounts for both SPL Token and SPL Token-2022 programs
        let filter_spl = TokenAccountsFilter::ProgramId(spl_token::id());
        let filter_spl_2022 = TokenAccountsFilter::ProgramId(spl_token_2022::id());

        let mut accounts = self
            .rpc_client
            .get_token_accounts_by_owner(wallet, filter_spl_2022)
            .await?;

        let mut balances = HashMap::new();

        for keyed_account in accounts {
            if let solana_account_decoder::UiAccountData::Json(parsed_account) =
                keyed_account.account.data
            {
                if let Some(info) = parsed_account.parsed.get("info") {
                    if let Ok(token_data) =
                        serde_json::from_value::<UiTokenAccount>(info.clone())
                    {
                        let token_amount = token_data.token_amount;
                        if let Ok(amount_u64) = token_amount.amount.parse::<u64>() {
                            if amount_u64 > 0 {
                                let mint: Pubkey = token_data.mint.parse()?;

                                let balance = TokenBalance {
                                    mint,
                                    amount: amount_u64,
                                    decimals: token_amount.decimals,
                                    ui_amount: token_amount.ui_amount.unwrap_or(0.0),
                                    symbol: None,
                                    name: None,
                                };

                                balances.insert(mint, balance);
                            }
                        }
                    }
                }
            }
        }

        info!("Found {} tokens with non-zero balance", balances.len());
        Ok(balances)
    }

    async fn get_mint_decimals(&self, mint: &Pubkey) -> anyhow::Result<u8> {
        // Check local cache first
        {
            let cache = self.decimals_cache.read().await;
            if let Some(&decimals) = cache.get(mint) {
                return Ok(decimals);
            }
        }

        // Fetch raw account data
        let data = self.rpc_client.get_account_data(mint).await?;

        // Unpack the data into a Mint struct
        let mint_state = Mint::unpack(&data)
            .map_err(|_| anyhow::anyhow!("Failed to unpack Mint account data"))?;

        let decimals = mint_state.decimals;

        // Update cache
        {
            let mut cache = self.decimals_cache.write().await;
            cache.insert(*mint, decimals);
        }

        Ok(decimals)
    }
}