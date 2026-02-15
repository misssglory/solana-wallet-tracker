use reqwest::Client;
use serde_json;
use tracing::{debug, warn};

#[derive(Clone)]
pub struct TelegramNotifier {
  client: Option<Client>,
  token: Option<String>,
  chat_id: Option<String>,
}

impl TelegramNotifier {
  pub fn new() -> Self {
    let token = std::env::var("TG_TOKEN").ok();
    let chat_id = std::env::var("CHAT_ID").ok();

    let client = if token.is_some() && chat_id.is_some() {
      Some(Client::new())
    } else {
      None
    };

    Self { client, token, chat_id }
  }

  pub fn is_enabled(&self) -> bool {
    self.client.is_some() && self.token.is_some() && self.chat_id.is_some()
  }

  /// Send Telegram notification
  pub async fn send_notification(&self, message: &str) {
    if let (Some(client), Some(token), Some(chat_id)) =
      (&self.client, &self.token, &self.chat_id)
    {
      let url = format!("https://api.telegram.org/bot{}/sendMessage", token);

      let payload = serde_json::json!({
          "chat_id": chat_id,
          "text": message,
          "parse_mode": "HTML",
          "disable_web_page_preview": true
      });

      match client
        .post(&url)
        .body(payload.to_string())
        .header("Content-Type", "application/json")
        .send()
        .await
      {
        Ok(response) => {
          let response_status = response.status();
          if !response_status.is_success() {
            warn!("Telegram API error: Status {}", response_status);
            if let Ok(text) = response.text().await {
              warn!("Telegram API response: {}", text);
            }
          } else {
            debug!("Telegram notification sent successfully");
          }
        }
        Err(e) => {
          warn!("Failed to send Telegram notification: {}", e);
        }
      }
    }
  }
}

impl Default for TelegramNotifier {
  fn default() -> Self {
    Self::new()
  }
}
