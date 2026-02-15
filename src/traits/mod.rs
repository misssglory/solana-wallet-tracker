//! Core traits for the portfolio tracker

pub mod data_provider;
pub mod price_provider;
pub mod event_handler;

// Re-export for convenience
pub use data_provider::PortfolioDataProvider;
pub use price_provider::PriceProvider;
pub use event_handler::{PortfolioEventHandler, TokenEventHandler};