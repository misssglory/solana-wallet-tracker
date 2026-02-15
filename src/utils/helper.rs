use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

/// Parse a pubkey from string, with better error messages
pub fn parse_pubkey(s: &str) -> anyhow::Result<Pubkey> {
    Pubkey::from_str(s).map_err(|e| anyhow::anyhow!("Invalid pubkey {}: {}", s, e))
}

/// Format lamports as SOL
pub fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / 1e9
}

/// Format SOL as lamports
pub fn sol_to_lamports(sol: f64) -> u64 {
    (sol * 1e9) as u64
}

/// Truncate a string to a maximum length
pub fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

/// Format a pubkey for display (truncated)
pub fn format_pubkey(pubkey: &Pubkey) -> String {
    let s = pubkey.to_string();
    format!("{}...{}", &s[..4], &s[s.len()-4..])
}