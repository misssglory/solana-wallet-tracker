// src/notifications/mod.rs
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedSender, UnboundedReceiver, unbounded_channel};
use tracing::{error, warn};

use crate::models::portfolio::PortfolioDiff;
use crate::traits::event_handler::PortfolioEventHandler;

/// Notification types
#[derive(Debug, Clone)]
pub enum Notification {
    PortfolioChange(PortfolioDiff),
    Error(String),
    Shutdown,
}

/// Notification queue for async processing
pub struct NotificationQueue {
    sender: UnboundedSender<Notification>,
    handler: Arc<dyn PortfolioEventHandler>,
}

impl NotificationQueue {
    /// Create a new notification queue
    pub fn new(handler: Arc<dyn PortfolioEventHandler>) -> Self {
        let (sender, receiver) = unbounded_channel();
        
        // Spawn a dedicated task for processing notifications
        tokio::spawn(Self::process_notifications(receiver, handler.clone()));
        
        Self { sender, handler }
    }
    
    /// Process notifications in a separate task
    async fn process_notifications(
        mut receiver: UnboundedReceiver<Notification>,
        handler: Arc<dyn PortfolioEventHandler>,
    ) {
        while let Some(notification) = receiver.recv().await {
            match notification {
                Notification::PortfolioChange(diff) => {
                    // Handle portfolio change - this can be slow (Telegram API calls)
                    handler.handle_portfolio_change(diff).await;
                }
                Notification::Error(err_msg) => {
                    let err = anyhow::anyhow!("{}", err_msg);
                    handler.handle_error(&err).await;
                }
                Notification::Shutdown => {
                    warn!("Notification processor shutting down");
                    break;
                }
            }
        }
    }
    
    /// Queue a portfolio change notification (non-blocking)
    pub fn notify_portfolio_change(&self, diff: PortfolioDiff) {
        if let Err(e) = self.sender.send(Notification::PortfolioChange(diff)) {
            error!("Failed to queue portfolio change notification: {}", e);
        }
    }
    
    /// Queue an error notification (non-blocking)
    pub fn notify_error(&self, error: &anyhow::Error) {
        if let Err(e) = self.sender.send(Notification::Error(error.to_string())) {
            error!("Failed to queue error notification: {}", e);
        }
    }
}

impl Clone for NotificationQueue {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            handler: self.handler.clone(),
        }
    }
}