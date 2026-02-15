//! Event handlers for portfolio changes

pub mod console;
pub mod telegram;
pub mod socket;
pub mod composite;

// Re-export for convenience
pub use console::ConsoleEventHandler;
pub use telegram::TelegramEventHandler;
pub use socket::SocketTokenEventHandler;
pub use composite::CompositeEventHandler;