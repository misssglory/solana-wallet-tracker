//! Portfolio Tracker Library
//! 
//! A modular portfolio tracker for Solana wallets that supports both
//! polling-based and WebSocket-based tracking.

// Public modules - these are the API surface
pub mod models;
pub mod traits;
pub mod providers;
pub mod handlers;
pub mod tracker;
pub mod utils;
pub mod telegram_notifier;

// Re-export commonly used items for easier access
pub use models::{
    token::TokenBalance,
    portfolio::{PortfolioSnapshot, PortfolioDiff, TokenChange},
};
pub use traits::{
    data_provider::PortfolioDataProvider,
    price_provider::PriceProvider,
    event_handler::{PortfolioEventHandler, TokenEventHandler},
};
pub use providers::{
    rpc_provider::RpcDataProvider,
    websocket_provider::WebSocketDataProvider,
    price_provider::SimplePriceProvider,
};
pub use handlers::{
    console::ConsoleEventHandler,
    telegram::TelegramEventHandler,
    socket::SocketTokenEventHandler,
    composite::CompositeEventHandler,
};
pub use tracker::portfolio_tracker::PortfolioTracker;

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Result type alias for library functions
pub type Result<T> = std::result::Result<T, anyhow::Error>;