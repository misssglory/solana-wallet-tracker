// In src/providers/websocket_provider.rs

use async_trait::async_trait;
use futures_util::stream::StreamExt;
use solana_account_decoder::parse_token::UiTokenAccount;
use solana_client::{
  nonblocking::{pubsub_client::PubsubClient, rpc_client::RpcClient},
  rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig},
  rpc_filter::{Memcmp, RpcFilterType},
  rpc_request::TokenAccountsFilter,
};
use solana_commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_program::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use spl_token_2022::state::Mint;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info};

use crate::{
  PortfolioDataProvider,
  TokenBalance,
  models::portfolio::{PortfolioDiff, TokenChange}, // Add this import
  notifications::NotificationQueue,                // Add this import
};

use tokio::sync::mpsc::unbounded_channel;

pub struct WebSocketDataProvider {
  rpc_client: Arc<RpcClient>,
  ws_url: String,
  subscription_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
  decimals_cache: Arc<RwLock<HashMap<Pubkey, u8>>>,
  wallet_address: Pubkey,
  notification_queue: NotificationQueue, // Replace callback with queue
  _pubsub_client: Arc<Mutex<Option<PubsubClient>>>,
  // Store last known balances to calculate changes
  last_balances: Arc<Mutex<HashMap<Pubkey, TokenBalance>>>,
}

impl WebSocketDataProvider {
  pub fn new(
    rpc_url: String,
    ws_url: String,
    wallet_address: Pubkey,
    notification_queue: NotificationQueue, // Change parameter type
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
      notification_queue, // Store queue
      _pubsub_client: Arc::new(Mutex::new(None)),
      last_balances: Arc::new(Mutex::new(HashMap::new())),
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

    // Clone the notification queue and last_balances for the SOL handler
    let notification_queue = self.notification_queue.clone();
    let wallet_address = self.wallet_address;
    let last_balances = self.last_balances.clone();

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

              // Get last known SOL balance
              let mut last_balances_guard = last_balances.lock().await;
              let last_sol =
                last_balances_guard.get(&Pubkey::default()).cloned();

              // Create diff if balance changed
              if let Some(last) = last_sol {
                if (last.ui_amount - sol_balance.ui_amount).abs() > f64::EPSILON
                {
                  let mut diff = PortfolioDiff::default();
                  diff.changes.push(TokenChange {
                    mint: Pubkey::default(),
                    old_amount: last.ui_amount,
                    new_amount: sol_balance.ui_amount,
                    change: sol_balance.ui_amount - last.ui_amount,
                    percentage_change: if last.ui_amount > 0.0 {
                      ((sol_balance.ui_amount - last.ui_amount)
                        / last.ui_amount)
                        * 100.0
                    } else {
                      100.0
                    },
                  });
                  notification_queue.notify_portfolio_change(diff);
                }
              } else {
                // First time seeing SOL balance
                let mut diff = PortfolioDiff::default();
                diff.added.push(sol_balance.clone());
                notification_queue.notify_portfolio_change(diff);
              }

              // Update last balance
              last_balances_guard.insert(Pubkey::default(), sol_balance);
            }
          }
        }
      }
    });

    // Clone for token handler
    let notification_queue = self.notification_queue.clone();
    let wallet_address = self.wallet_address;
    let last_balances = self.last_balances.clone();

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

                  // Get last known balance for this mint
                  let mut last_balances_guard = last_balances.lock().await;
                  let last = last_balances_guard.get(&mint).cloned();

                  // Create appropriate diff
                  let mut diff = PortfolioDiff::default();

                  match last {
                    Some(last_balance) => {
                      if (last_balance.ui_amount - balance.ui_amount).abs()
                        > f64::EPSILON
                      {
                        diff.changes.push(TokenChange {
                          mint,
                          old_amount: last_balance.ui_amount,
                          new_amount: balance.ui_amount,
                          change: balance.ui_amount - last_balance.ui_amount,
                          percentage_change: if last_balance.ui_amount > 0.0 {
                            ((balance.ui_amount - last_balance.ui_amount)
                              / last_balance.ui_amount)
                              * 100.0
                          } else {
                            100.0
                          },
                        });
                        notification_queue.notify_portfolio_change(diff);
                      }
                    }
                    None => {
                      diff.added.push(balance.clone());
                      notification_queue.notify_portfolio_change(diff);
                    }
                  }

                  // Update last balance
                  last_balances_guard.insert(mint, balance);
                } else {
                  // Token balance became zero - check if we had it before
                  let mint = Pubkey::from_str_const(&token_data.mint);
                  let mut last_balances_guard = last_balances.lock().await;

                  if let Some(removed_balance) =
                    last_balances_guard.remove(&mint)
                  {
                    let mut diff = PortfolioDiff::default();
                    diff.removed.push(removed_balance);
                    notification_queue.notify_portfolio_change(diff);
                  }
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
