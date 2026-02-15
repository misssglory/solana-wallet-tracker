// In src/providers/websocket_provider.rs

use async_trait::async_trait;
use futures_util::stream::StreamExt;
use solana_account_decoder::parse_token::UiTokenAccount;
use solana_client::{
  nonblocking::{pubsub_client::PubsubClient, rpc_client::RpcClient},
  rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig},
  rpc_filter::{Memcmp, RpcFilterType}, rpc_request::TokenAccountsFilter,
};
use solana_commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_sdk::pubkey::Pubkey;
use solana_program::program_pack::Pack;
use spl_token_2022::state::Mint;
use std::{
  collections::HashMap,
  sync::Arc,
};
use tokio::sync::{Mutex, RwLock};
use tracing::info;

use crate::{PortfolioDataProvider, TokenBalance};

use tokio::sync::mpsc::{
  unbounded_channel,
};

pub struct WebSocketDataProvider {
  rpc_client: Arc<RpcClient>,
  ws_url: String,
  subscription_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
  decimals_cache: Arc<RwLock<HashMap<Pubkey, u8>>>,
  wallet_address: Pubkey,
  on_balance_update: Arc<dyn Fn(Pubkey, TokenBalance) + Send + Sync>,
  _pubsub_client: Arc<Mutex<Option<PubsubClient>>>,
}

impl WebSocketDataProvider {
  pub fn new(
    rpc_url: String,
    ws_url: String,
    wallet_address: Pubkey,
    on_balance_update: Arc<dyn Fn(Pubkey, TokenBalance) + Send + Sync>,
  ) -> Self {
    let client = RpcClient::new_with_commitment(
      rpc_url,
      CommitmentConfig { commitment: CommitmentLevel::Processed },
    );

    Self {
      rpc_client: Arc::new(client),
      ws_url,
      subscription_handles: Arc::new(Mutex::new(Vec::new())),
      decimals_cache: Arc::new(RwLock::new(HashMap::new())),
      wallet_address,
      on_balance_update,
      _pubsub_client: Arc::new(Mutex::new(None)),
    }
  }

  pub async fn start_subscriptions(&self) -> anyhow::Result<()> {
    // Create channels to forward notifications
    let (sol_tx, mut sol_rx) = unbounded_channel();
    let (token_tx, mut token_rx) = unbounded_channel();

    // Spawn a task to handle the pubsub client
    let ws_url = self.ws_url.clone();
    let wallet_address = self.wallet_address;
    let sol_tx_clone = sol_tx.clone();
    let token_tx_clone = token_tx.clone();

    let pubsub_handle = tokio::spawn(async move {
      // Create the client inside the task
      let pubsub_client = match PubsubClient::new(&ws_url).await {
        Ok(client) => client,
        Err(e) => {
          eprintln!("Failed to connect to WebSocket: {}", e);
          return;
        }
      };

      // Subscribe to SOL balance changes
      let (mut sol_notifications, _sol_sub) = pubsub_client
        .account_subscribe(
          &wallet_address,
          Some(RpcAccountInfoConfig {
            encoding: Some(
              solana_account_decoder::UiAccountEncoding::JsonParsed,
            ),
            commitment: Some(CommitmentConfig::confirmed()),
            data_slice: None,
            min_context_slot: None,
          }),
        )
        .await
        .expect("Failed to subscribe to SOL account");

      // Subscribe to token account changes
      let token_program_id =
        Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
      let wallet_bytes = wallet_address.to_bytes();

      let (mut token_notifications, _token_sub) = pubsub_client
        .program_subscribe(
          &token_program_id,
          Some(RpcProgramAccountsConfig {
            filters: Some(vec![RpcFilterType::Memcmp(Memcmp::new_raw_bytes(
              32,
              wallet_bytes.to_vec(),
            ))]),
            account_config: RpcAccountInfoConfig {
              encoding: Some(
                solana_account_decoder::UiAccountEncoding::JsonParsed,
              ),
              commitment: Some(CommitmentConfig::confirmed()),
              data_slice: None,
              min_context_slot: None,
            },
            with_context: None,
            sort_results: None,
          }),
        )
        .await
        .expect("Failed to subscribe to token program");

      // Handle both streams concurrently
      let sol_tx = sol_tx_clone;
      let token_tx = token_tx_clone;

      // Use a single task with select! to handle both streams
      loop {
        tokio::select! {
            Some(notification) = sol_notifications.next() => {
                if sol_tx.send(notification).is_err() {
                    break;
                }
            }
            Some(notification) = token_notifications.next() => {
                if token_tx.send(notification).is_err() {
                    break;
                }
            }
            else => break,
        }
      }
    });

    // Spawn SOL balance handler
    let wallet_address = self.wallet_address;
    let on_balance_update = self.on_balance_update.clone();

    let sol_handler = tokio::spawn(async move {
      while let Some(notification) = sol_rx.recv().await {
        if let solana_account_decoder::UiAccountData::Json(parsed_data) =
          notification.value.data
        {
          if let Some(lamports) = parsed_data.parsed.get("lamports") {
            if let Some(lamports_val) = lamports.as_u64() {
              let sol_balance = TokenBalance {
                mint: Pubkey::default(),
                amount: lamports_val,
                decimals: 9,
                ui_amount: lamports_val as f64 / 1e9,
                symbol: Some("SOL".to_string()),
                name: Some("Solana".to_string()),
              };
              on_balance_update(wallet_address, sol_balance);
            }
          }
        }
      }
    });

    // Spawn token balance handler
    let wallet_address = self.wallet_address;
    let on_balance_update = self.on_balance_update.clone();

    let token_handler = tokio::spawn(async move {
      while let Some(notification) = token_rx.recv().await {
        if let solana_account_decoder::UiAccountData::Json(parsed_data) =
          notification.value.account.data
        {
          if let Some(info) = parsed_data.parsed.get("info") {
            if let Ok(token_data) =
              serde_json::from_value::<UiTokenAccount>(info.clone())
            {
              if let Ok(amount_u64) =
                token_data.token_amount.amount.parse::<u64>()
              {
                if amount_u64 > 0 {
                  let mint = Pubkey::from_str_const(&token_data.mint);
                  let balance = TokenBalance {
                    mint,
                    amount: amount_u64,
                    decimals: token_data.token_amount.decimals,
                    ui_amount: token_data.token_amount.ui_amount.unwrap_or(0.0),
                    symbol: None,
                    name: None,
                  };
                  on_balance_update(wallet_address, balance);
                }
              }
            }
          }
        }
      }
    });

    let mut handles = self.subscription_handles.lock().await;
    handles.push(pubsub_handle);
    handles.push(sol_handler);
    handles.push(token_handler);

    info!(
      "WebSocket subscriptions started for wallet: {}",
      self.wallet_address
    );

    Ok(())
  }
}

#[async_trait]
impl PortfolioDataProvider for WebSocketDataProvider {
  async fn fetch_sol_balance(&self, wallet: &Pubkey) -> anyhow::Result<f64> {
    // Fallback to RPC for initial balance
    let balance = self.rpc_client.get_balance(wallet).await?;
    Ok(balance as f64 / 1e9)
  }

  async fn fetch_token_balances(
    &self,
    wallet: &Pubkey,
  ) -> anyhow::Result<HashMap<Pubkey, TokenBalance>> {
    // Fallback to RPC for initial balances
    let filter_spl_2022 = TokenAccountsFilter::ProgramId(spl_token_2022::id());

    let accounts = self
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

    // Fetch from RPC
    let data = self.rpc_client.get_account_data(mint).await?;
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

  async fn start_realtime_updates(&self) -> anyhow::Result<()> {
    self.start_subscriptions().await
  }
}