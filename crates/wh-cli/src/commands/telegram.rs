//! `wh telegram` subcommands.

use std::collections::HashMap;

use clap::Subcommand;
use serde::Serialize;

use crate::commands::secrets;
use crate::output::error::WhError;
use crate::output::{OutputEnvelope, OutputFormat};

/// Telegram subcommands.
#[derive(Debug, Subcommand)]
pub enum TelegramCommand {
    /// Resolve chats the bot is a member of via getUpdates.
    Resolve {
        /// Bot token (overrides keychain). If not provided, reads TELEGRAM_BOT_TOKEN from keychain.
        #[arg(long)]
        token: Option<String>,

        /// Output format: human (default) or json.
        #[arg(long, default_value = "human")]
        format: OutputFormat,
    },
}

/// A resolved chat that the bot is a member of.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedChat {
    pub chat_id: i64,
    pub title: String,
    #[serde(rename = "type")]
    pub chat_type: String,
}

/// Execute a telegram subcommand.
pub async fn execute(command: &TelegramCommand) -> Result<(), WhError> {
    match command {
        TelegramCommand::Resolve { token, format } => resolve(token.as_deref(), *format).await,
    }
}

/// Resolve all chats the bot is a member of by calling getUpdates.
async fn resolve(token: Option<&str>, format: OutputFormat) -> Result<(), WhError> {
    // Resolve token: CLI flag > keychain > error.
    let bot_token = match token {
        Some(t) => t.to_string(),
        None => secrets::read_secret("telegram_bot_token")?,
    };

    // Call getUpdates to discover chats.
    let url = format!(
        "https://api.telegram.org/bot{}/getUpdates?limit=100&allowed_updates={}",
        bot_token,
        url_encode("[\"my_chat_member\",\"message\"]"),
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| WhError::Internal(format!("Telegram API request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "unknown".to_string());
        return Err(WhError::Internal(format!(
            "Telegram API returned HTTP {status}: {body}"
        )));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| WhError::Internal(format!("Failed to parse Telegram response: {e}")))?;

    // Check Telegram API-level success.
    if body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let description = body
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(WhError::Internal(format!(
            "Telegram API error: {description}"
        )));
    }

    // Extract unique chats from all updates.
    let chats = extract_chats(&body);

    if chats.is_empty() {
        match format {
            OutputFormat::Human => {
                println!("No chats found. Make sure the bot has been added to groups or has received messages.");
                println!("Tip: send a message to the bot or add it to a group, then run this command again.");
            }
            OutputFormat::Json => {
                let envelope = OutputEnvelope::ok(Vec::<ResolvedChat>::new());
                let json = serde_json::to_string_pretty(&envelope)
                    .map_err(|e| WhError::Internal(e.to_string()))?;
                println!("{json}");
            }
        }
        return Ok(());
    }

    match format {
        OutputFormat::Human => {
            // Print table header.
            println!("{:<15} {:<30} {}", "CHAT_ID", "TITLE", "TYPE");
            println!("{}", "-".repeat(60));
            for chat in &chats {
                println!("{:<15} {:<30} {}", chat.chat_id, chat.title, chat.chat_type);
            }
        }
        OutputFormat::Json => {
            let envelope = OutputEnvelope::ok(&chats);
            let json = serde_json::to_string_pretty(&envelope)
                .map_err(|e| WhError::Internal(e.to_string()))?;
            println!("{json}");
        }
    }

    Ok(())
}

/// Extract unique chats from the Telegram getUpdates response.
fn extract_chats(body: &serde_json::Value) -> Vec<ResolvedChat> {
    let mut seen: HashMap<i64, ResolvedChat> = HashMap::new();

    if let Some(results) = body.get("result").and_then(|v| v.as_array()) {
        for update in results {
            // Check message.chat
            if let Some(chat) = update.get("message").and_then(|m| m.get("chat")) {
                if let Some(resolved) = parse_chat(chat) {
                    seen.entry(resolved.chat_id).or_insert(resolved);
                }
            }

            // Check my_chat_member.chat
            if let Some(chat) = update.get("my_chat_member").and_then(|m| m.get("chat")) {
                if let Some(resolved) = parse_chat(chat) {
                    seen.entry(resolved.chat_id).or_insert(resolved);
                }
            }

            // Check edited_message.chat
            if let Some(chat) = update.get("edited_message").and_then(|m| m.get("chat")) {
                if let Some(resolved) = parse_chat(chat) {
                    seen.entry(resolved.chat_id).or_insert(resolved);
                }
            }

            // Check channel_post.chat
            if let Some(chat) = update.get("channel_post").and_then(|m| m.get("chat")) {
                if let Some(resolved) = parse_chat(chat) {
                    seen.entry(resolved.chat_id).or_insert(resolved);
                }
            }
        }
    }

    let mut chats: Vec<ResolvedChat> = seen.into_values().collect();
    chats.sort_by_key(|c| c.chat_id);
    chats
}

/// Parse a Telegram chat JSON object into a ResolvedChat.
fn parse_chat(chat: &serde_json::Value) -> Option<ResolvedChat> {
    let chat_id = chat.get("id").and_then(|v| v.as_i64())?;
    let chat_type = chat
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Title: use "title" for groups/supergroups/channels, fall back to
    // "first_name" + "last_name" for private chats, then username.
    let title = chat
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            let first = chat
                .get("first_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let last = chat.get("last_name").and_then(|v| v.as_str()).unwrap_or("");
            let full = format!("{first} {last}").trim().to_string();
            if full.is_empty() {
                chat.get("username")
                    .and_then(|v| v.as_str())
                    .map(|s| format!("@{s}"))
            } else {
                Some(full)
            }
        })
        .unwrap_or_else(|| format!("chat_{chat_id}"));

    Some(ResolvedChat {
        chat_id,
        title,
        chat_type,
    })
}

/// Minimal percent-encoding for URL query parameters.
fn url_encode(s: &str) -> String {
    s.replace('[', "%5B")
        .replace(']', "%5D")
        .replace('"', "%22")
        .replace(',', "%2C")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_chats_from_updates() {
        let body = serde_json::json!({
            "ok": true,
            "result": [
                {
                    "update_id": 1,
                    "message": {
                        "message_id": 1,
                        "chat": {
                            "id": -1001234567890_i64,
                            "title": "Test Group",
                            "type": "supergroup"
                        }
                    }
                },
                {
                    "update_id": 2,
                    "message": {
                        "message_id": 2,
                        "chat": {
                            "id": 42,
                            "first_name": "Alice",
                            "last_name": "Smith",
                            "type": "private"
                        }
                    }
                },
                {
                    "update_id": 3,
                    "message": {
                        "message_id": 3,
                        "chat": {
                            "id": -1001234567890_i64,
                            "title": "Test Group",
                            "type": "supergroup"
                        }
                    }
                }
            ]
        });

        let chats = extract_chats(&body);
        assert_eq!(chats.len(), 2, "should deduplicate by chat_id");

        // Sorted by chat_id: -1001234567890 < 42
        assert_eq!(chats[0].chat_id, -1001234567890);
        assert_eq!(chats[0].title, "Test Group");
        assert_eq!(chats[0].chat_type, "supergroup");

        assert_eq!(chats[1].chat_id, 42);
        assert_eq!(chats[1].title, "Alice Smith");
        assert_eq!(chats[1].chat_type, "private");
    }

    #[test]
    fn extract_chats_empty_result() {
        let body = serde_json::json!({
            "ok": true,
            "result": []
        });
        let chats = extract_chats(&body);
        assert!(chats.is_empty());
    }

    #[test]
    fn parse_chat_private_with_username_only() {
        let chat = serde_json::json!({
            "id": 99,
            "username": "testbot",
            "type": "private"
        });
        let resolved = parse_chat(&chat).unwrap();
        assert_eq!(resolved.chat_id, 99);
        assert_eq!(resolved.title, "@testbot");
        assert_eq!(resolved.chat_type, "private");
    }

    #[test]
    fn parse_chat_group_with_title() {
        let chat = serde_json::json!({
            "id": -100,
            "title": "My Group",
            "type": "group"
        });
        let resolved = parse_chat(&chat).unwrap();
        assert_eq!(resolved.chat_id, -100);
        assert_eq!(resolved.title, "My Group");
        assert_eq!(resolved.chat_type, "group");
    }

    #[test]
    fn parse_chat_missing_id() {
        let chat = serde_json::json!({
            "title": "No ID",
            "type": "group"
        });
        assert!(parse_chat(&chat).is_none());
    }

    #[test]
    fn url_encode_brackets_and_quotes() {
        let encoded = url_encode("[\"test\"]");
        assert_eq!(encoded, "%5B%22test%22%5D");
    }

    #[test]
    fn resolved_chat_json_serialization() {
        let chat = ResolvedChat {
            chat_id: -1001234567890,
            title: "Ops Group".to_string(),
            chat_type: "supergroup".to_string(),
        };
        let json = serde_json::to_string(&chat).unwrap();
        assert!(json.contains("\"chat_id\":-1001234567890"));
        assert!(json.contains("\"title\":\"Ops Group\""));
        assert!(json.contains("\"type\":\"supergroup\""));
    }

    #[test]
    fn extract_chats_from_my_chat_member() {
        let body = serde_json::json!({
            "ok": true,
            "result": [
                {
                    "update_id": 1,
                    "my_chat_member": {
                        "chat": {
                            "id": -999,
                            "title": "Admin Group",
                            "type": "supergroup"
                        },
                        "new_chat_member": {
                            "status": "administrator"
                        }
                    }
                }
            ]
        });
        let chats = extract_chats(&body);
        assert_eq!(chats.len(), 1);
        assert_eq!(chats[0].chat_id, -999);
        assert_eq!(chats[0].title, "Admin Group");
    }
}
