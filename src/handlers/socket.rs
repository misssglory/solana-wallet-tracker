use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tracing::{debug, warn};

use crate::traits::event_handler::TokenEventHandler;
use crate::models::token::TokenBalance;

/// Socket-based token event handler
pub struct SocketTokenEventHandler {
    socket_token_added: Option<String>,
    socket_token_removed: Option<String>,
}

impl SocketTokenEventHandler {
    /// Create a new socket token event handler
    pub fn new() -> Self {
        Self {
            socket_token_added: std::env::var("SOCKET_TOKEN_ADDED").ok(),
            socket_token_removed: std::env::var("SOCKET_TOKEN_REMOVED").ok(),
        }
    }

    async fn send_to_socket(&self, socket_path: &str, token_address: &str) {
        match UnixStream::connect(socket_path).await {
            Ok(mut stream) => {
                if let Err(e) = stream.write_all(token_address.as_bytes()).await {
                    warn!("Failed to send to socket {}: {}", socket_path, e);
                } else {
                    debug!("Sent to socket {}: {}", socket_path, token_address);
                }
            }
            Err(e) => {
                warn!("Failed to connect to socket {}: {}", socket_path, e);
            }
        }
    }
}

impl Default for SocketTokenEventHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TokenEventHandler for SocketTokenEventHandler {
    async fn on_token_added(&self, token_address: &str, _balance: &TokenBalance) {
        if let Some(socket_path) = &self.socket_token_added {
            self.send_to_socket(socket_path, token_address).await;
        }
    }

    async fn on_token_removed(&self, token_address: &str, _balance: &TokenBalance) {
        if let Some(socket_path) = &self.socket_token_removed {
            self.send_to_socket(socket_path, token_address).await;
        }
    }
}