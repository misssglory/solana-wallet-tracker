use solana_sdk::pubkey::Pubkey;

/// Represents a token balance with metadata
#[derive(Debug, Clone)]
pub struct TokenBalance {
    pub mint: Pubkey,
    pub ui_amount: f64,
    pub decimals: u8,
    pub amount: u64,
    pub symbol: Option<String>,
    pub name: Option<String>,
}

impl TokenBalance {
    /// Create a new token balance
    pub fn new(
        mint: Pubkey,
        amount: u64,
        decimals: u8,
        symbol: Option<String>,
        name: Option<String>,
    ) -> Self {
        let ui_amount = amount as f64 / 10f64.powi(decimals as i32);
        
        Self {
            mint,
            ui_amount,
            decimals,
            amount,
            symbol,
            name,
        }
    }

    /// Format token amount with symbol
    pub fn formatted_amount(&self) -> String {
        format!("{:.8} {}", self.ui_amount, self.symbol.as_deref().unwrap_or("Unknown"))
    }

    /// Check if this is SOL (special case with zero mint)
    pub fn is_sol(&self) -> bool {
        self.mint == Pubkey::default()
    }
}