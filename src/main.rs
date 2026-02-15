//src.main.rs
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{error, info, warn};
use tracing_subscriber;

// Use crate:: instead of portfolio_tracker::
use portfolio_tracker::{
  // Handlers
  handlers::{
    composite::CompositeEventHandler, console::ConsoleEventHandler,
    socket::SocketTokenEventHandler, telegram::TelegramEventHandler,
  },

  // Models
  models::token::TokenBalance,

  // Providers
  providers::{
    price_provider::SimplePriceProvider, rpc_provider::RpcDataProvider,
    websocket_provider::WebSocketDataProvider,
  },

  telegram_notifier::TelegramNotifier,
  // Tracker
  tracker::PortfolioTracker,

  // Traits
  traits::{
    data_provider::PortfolioDataProvider,
    event_handler::{PortfolioEventHandler, TokenEventHandler},
    price_provider::PriceProvider,
  },

  // Utils
  utils::parse_pubkey,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  // Initialize logging
  tracing_subscriber::fmt()
    .with_level(true)
    .with_target(false)
    .with_max_level(tracing::level_filters::LevelFilter::DEBUG)
    .with_file(true)
    .with_line_number(true)
    .init();

  dotenvy::dotenv().ok();

  // Load configuration
  let rpc_url = std::env::var("SOLANA_RPC_URL")
    .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());

  let ws_url = std::env::var("SOLANA_WS_URL")
    .unwrap_or_else(|_| "wss://api.mainnet-beta.solana.com".to_string());

  let wallet_address_str =
    std::env::var("WALLET_ADDRESS").unwrap_or_else(|_| {
      "5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1".to_string()
    });

  let tracking_mode =
    std::env::var("TRACKING_MODE").unwrap_or_else(|_| "polling".to_string()); // "polling" or "websocket"

  let tick_interval = std::env::var("TICK_INTERVAL_MS")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(1100);

  info!("Initializing portfolio tracker...");
  info!("RPC URL: {}", rpc_url);
  info!("WebSocket URL: {}", ws_url);
  info!("Wallet Address: {}", wallet_address_str);
  info!("Tracking Mode: {}", tracking_mode);

  let wallet_address = parse_pubkey(&wallet_address_str)?;

  // Create data provider based on mode
  let data_provider: Arc<dyn PortfolioDataProvider> =
    if tracking_mode == "websocket" {
      info!("Using WebSocket data provider (Helius style)");

      rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install AWS-LC-RS crypto provider");
      // For WebSocket mode, we need a callback for real-time updates
      let on_update = Arc::new(|wallet: Pubkey, balance: TokenBalance| {
        info!(
          "Real-time update for {}: {} {}",
          wallet,
          balance.ui_amount,
          balance.mint.to_string() //   balance.symbol.as_deref().unwrap_or("Unknown")
        );
      });

      Arc::new(WebSocketDataProvider::new(
        rpc_url.clone(),
        ws_url.clone(),
        wallet_address,
        on_update,
      ))
    } else {
      info!("Using RPC polling data provider");
      Arc::new(RpcDataProvider::new(rpc_url.clone()))
    };

  // Create price provider
  let price_provider = Arc::new(SimplePriceProvider::new());

  // Create event handlers
  let mut composite_handler = CompositeEventHandler::new();
  composite_handler.add_handler(Arc::new(ConsoleEventHandler::new()));

  // Add Telegram handler if configured
  let telegram_notifier = TelegramNotifier::new();
  if telegram_notifier.is_enabled() {
    composite_handler.add_handler(Arc::new(TelegramEventHandler::new(
      telegram_notifier,
      wallet_address,
    )));
    info!("Telegram notifications enabled");
  } else {
    warn!(
      "Telegram notifications disabled. Set TG_TOKEN and CHAT_ID in .env file to enable."
    );
  }

  // Create token event handler for sockets
  let socket_handler = Arc::new(SocketTokenEventHandler::new());

  // Create main tracker
  let mut tracker = PortfolioTracker::new(
    wallet_address,
    data_provider.clone(),
    price_provider,
    Arc::new(composite_handler),
  );

  tracker.add_token_event_handler(socket_handler);

  // Log initial portfolio
  if let Err(e) = tracker.log_initial_portfolio().await {
    error!("Failed to log initial portfolio: {}", e);
  }

  // Start tracking based on mode
  if tracking_mode == "websocket" {
    // Start WebSocket subscriptions
    if let Err(e) = data_provider.start_realtime_updates().await {
      error!("Failed to start WebSocket subscriptions: {}", e);
      return Err(e);
    }

    info!("WebSocket portfolio tracker is running. Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await?;
  } else {
    // Polling mode
    info!(
      "Polling portfolio tracker is running with interval {}ms. Press Ctrl+C to stop.",
      tick_interval
    );

    tokio::select! {
        result = tracker.start_tracking_polling(tick_interval) => {
            if let Err(e) = result {
                error!("Tracking error: {}", e);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Shutting down...");
        }
    }
  }

  info!("Shutdown complete.");
  Ok(())
}
