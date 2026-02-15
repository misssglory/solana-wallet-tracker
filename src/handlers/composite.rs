use std::sync::Arc;
use async_trait::async_trait;

use crate::traits::event_handler::PortfolioEventHandler;
use crate::models::portfolio::PortfolioDiff;

/// Composite event handler that can combine multiple handlers
pub struct CompositeEventHandler {
    handlers: Vec<Arc<dyn PortfolioEventHandler>>,
}

impl CompositeEventHandler {
    /// Create a new composite event handler
    pub fn new() -> Self {
        Self { handlers: Vec::new() }
    }

    /// Add a handler to the composite
    pub fn add_handler(&mut self, handler: Arc<dyn PortfolioEventHandler>) {
        self.handlers.push(handler);
    }

    /// Check if there are any handlers
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// Number of handlers
    pub fn len(&self) -> usize {
        self.handlers.len()
    }
}

impl Default for CompositeEventHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PortfolioEventHandler for CompositeEventHandler {
    async fn handle_portfolio_change(&self, diff: PortfolioDiff) {
        for handler in &self.handlers {
            handler.handle_portfolio_change(diff.clone()).await;
        }
    }

    async fn handle_error(&self, error: &anyhow::Error) {
        for handler in &self.handlers {
            handler.handle_error(error).await;  // Now matches: passing &anyhow::Error
        }
    }
}