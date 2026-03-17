//! `wh telegram` subcommands.
//!
//! Also provides [`resolve_telegram_surfaces`] for deploy-time name resolution
//! (Task 3: resolves group display names to `chat_id` values and creates forum
//! topics, writing results to `.wh/telegram-state.json`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::commands::secrets;
use crate::model::WhFile;
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
            println!("{:<15} {:<30} TYPE", "CHAT_ID", "TITLE");
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

// ---------------------------------------------------------------------------
// Deploy-time Telegram name resolution (Task 3)
// ---------------------------------------------------------------------------

/// Serialization format matching `wh-telegram/src/state.rs::StateFile`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TelegramStateFile {
    #[serde(default)]
    groups: HashMap<String, i64>,
    #[serde(default)]
    topics: HashMap<String, i32>,
}

/// Returns true if `id` is a plain display name (not `@username` and not numeric).
fn is_display_name(id: &str) -> bool {
    !id.starts_with('@') && id.parse::<i64>().is_err()
}

/// Format a topic key the same way `wh-telegram/src/state.rs` does: `"chat_id:topic_name"`.
fn topic_key(chat_id: i64, topic_name: &str) -> String {
    format!("{chat_id}:{topic_name}")
}

/// Resolve Telegram group names and create topics at deploy time.
///
/// Called from `execute_apply()` before container provisioning. For every
/// telegram surface that declares a `chats` block with display-name IDs,
/// this function:
///
/// 1. Calls `getUpdates` to discover the bot's chats and resolve names → `chat_id`.
/// 2. Creates forum topics via `createForumTopic` if not already in state.
/// 3. Writes the updated state to `.wh/telegram-state.json`.
///
/// Returns `Ok(())` if there are no telegram surfaces with `chats`, or if
/// the bot token is unavailable (backward compat — skip silently).
/// A single entry in the flat routing table written to `telegram-routing.json`.
#[derive(Debug, Serialize, Deserialize)]
pub struct TelegramRoutingEntry {
    pub chat_id: i64,
    pub thread_id: Option<i32>,
    pub stream: String,
    /// Human-readable topic name (e.g., "Iktos", "General").
    /// Present for forum-topic routes, absent for non-topic routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_name: Option<String>,
}

/// Resolve Telegram group names and create topics at deploy time.
///
/// Returns the path to `telegram-routing.json` when chats are resolved,
/// or `None` when the surface uses legacy single-stream mode.
pub fn resolve_telegram_surfaces(topology_path: &Path) -> Result<Option<PathBuf>, String> {
    // Parse the topology to find telegram surfaces with chats.
    let content = std::fs::read_to_string(topology_path)
        .map_err(|e| format!("cannot read topology file: {e}"))?;
    let wh_file: WhFile =
        serde_yaml::from_str(&content).map_err(|e| format!("cannot parse topology: {e}"))?;

    let surfaces = match &wh_file.surfaces {
        Some(s) => s,
        None => return Ok(None),
    };

    // Collect telegram surfaces that have a `chats` block.
    let telegram_surfaces: Vec<_> = surfaces
        .iter()
        .filter(|s| s.kind.as_deref() == Some("telegram") && s.chats.is_some())
        .collect();

    if telegram_surfaces.is_empty() {
        return Ok(None);
    }

    // Check if any surface has display-name chat IDs that need resolution.
    let needs_resolution = telegram_surfaces.iter().any(|s| {
        s.chats.as_ref().is_some_and(|chats| {
            chats
                .iter()
                .any(|c| c.id.as_ref().is_some_and(|id| is_display_name(id)))
        })
    });

    let has_threads = telegram_surfaces.iter().any(|s| {
        s.chats
            .as_ref()
            .is_some_and(|chats| chats.iter().any(|c| c.threads.is_some()))
    });

    if !needs_resolution && !has_threads {
        debug!("No display-name chat IDs or threads to resolve — skipping Telegram resolution");
        return Ok(None);
    }

    // Resolve bot token: surface env > keychain. Skip if unavailable (backward compat).
    let bot_token = resolve_bot_token(&telegram_surfaces);
    let bot_token = match bot_token {
        Some(t) => t,
        None => {
            warn!("Telegram bot token not found in keychain or surface env — skipping name resolution");
            return Ok(None);
        }
    };

    // Load existing state.
    let state_dir = topology_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".wh");
    let mut state = load_state_file(&state_dir);

    // Call getUpdates once to discover chats.
    let discovered_chats = call_get_updates_blocking(&bot_token)?;

    // Build a lookup: title -> chat_id from discovered chats.
    let title_to_id: HashMap<String, i64> = discovered_chats
        .iter()
        .map(|c| (c.title.clone(), c.chat_id))
        .collect();

    // Accumulate flat routing entries: (chat_id, thread_id) -> stream.
    let mut routing_entries: Vec<TelegramRoutingEntry> = Vec::new();

    // Resolve each surface's chats.
    for surface in &telegram_surfaces {
        let chats = match &surface.chats {
            Some(c) => c,
            None => continue,
        };

        for chat_spec in chats {
            let chat_id_str = match &chat_spec.id {
                Some(id) => id,
                None => continue,
            };

            // Determine the numeric chat_id.
            let chat_id: i64 = if chat_id_str.starts_with('@') {
                // DMs by @username don't need numeric resolution for routing.
                debug!("Skipping @username chat '{chat_id_str}' — no numeric resolution needed");
                continue;
            } else if let Ok(numeric_id) = chat_id_str.parse::<i64>() {
                numeric_id
            } else {
                // Display name — resolve via getUpdates or existing state.
                if let Some(&id) = state.groups.get(chat_id_str) {
                    debug!("Group '{chat_id_str}' already in state: {id}");
                    id
                } else if let Some(&id) = title_to_id.get(chat_id_str) {
                    info!("Resolved group '{chat_id_str}' to chat_id {id}");
                    state.groups.insert(chat_id_str.clone(), id);
                    id
                } else {
                    return Err(format!(
                        "Bot has not been added to group '{chat_id_str}' \u{2014} add it as admin with Manage Topics permission"
                    ));
                }
            };

            // Create forum topics for any declared threads.
            if let Some(threads) = &chat_spec.threads {
                for thread_spec in threads {
                    let topic_name = match &thread_spec.id {
                        Some(name) => name,
                        None => continue,
                    };

                    let key = topic_key(chat_id, topic_name);
                    if state.topics.contains_key(&key) {
                        debug!("Topic '{topic_name}' in chat {chat_id} already in state — skipping creation");
                        // Still add routing entry from existing state.
                        if let Some(&thread_id) = state.topics.get(&key) {
                            routing_entries.push(TelegramRoutingEntry {
                                chat_id,
                                thread_id: Some(thread_id),
                                stream: thread_spec.stream.clone().unwrap_or_default(),
                                topic_name: Some(topic_name.clone()),
                            });
                        }
                        continue;
                    }

                    // Create the forum topic via Telegram API.
                    // Handle group→supergroup migration: Telegram returns the new chat_id.
                    let (thread_id, effective_chat_id) =
                        match create_forum_topic_blocking(&bot_token, chat_id, topic_name)? {
                            (0, Some(new_id)) => {
                                info!(
                                "Group '{chat_id_str}' migrated to supergroup {new_id} — retrying"
                            );
                                // Update state so future lookups use the new id.
                                state.groups.insert(chat_id_str.clone(), new_id);
                                let (tid, _) =
                                    create_forum_topic_blocking(&bot_token, new_id, topic_name)?;
                                (tid, new_id)
                            }
                            (tid, _) => (tid, chat_id),
                        };
                    info!("Created topic '{topic_name}' in chat {effective_chat_id} → thread_id {thread_id}");
                    state
                        .topics
                        .insert(topic_key(effective_chat_id, topic_name), thread_id);
                    routing_entries.push(TelegramRoutingEntry {
                        chat_id: effective_chat_id,
                        thread_id: Some(thread_id),
                        stream: thread_spec.stream.clone().unwrap_or_default(),
                        topic_name: Some(topic_name.clone()),
                    });
                }
            }
        }
    }

    // Persist updated state.
    save_state_file(&state_dir, &state)?;
    info!(
        "Telegram state saved to {}",
        state_dir.join("telegram-state.json").display()
    );

    // Write flat routing table so wh-telegram can build its RoutingTable at startup.
    let routing_path = state_dir.join("telegram-routing.json");
    let routing_json = serde_json::to_string_pretty(&routing_entries)
        .map_err(|e| format!("failed to serialize routing table: {e}"))?;
    std::fs::create_dir_all(&state_dir).map_err(|e| format!("failed to create state dir: {e}"))?;
    std::fs::write(&routing_path, routing_json)
        .map_err(|e| format!("failed to write routing file: {e}"))?;
    info!(
        "Telegram routing table written to {}",
        routing_path.display()
    );

    Ok(Some(routing_path))
}

/// Try to find the bot token from surface env vars or keychain.
fn resolve_bot_token(surfaces: &[&crate::model::SurfaceSpec]) -> Option<String> {
    // Check surface env blocks first.
    for surface in surfaces {
        if let Some(env) = &surface.env {
            if let Some(token) = env.get("TELEGRAM_BOT_TOKEN") {
                return Some(token.clone());
            }
        }
    }
    // Fall back to keychain.
    secrets::read_secret("telegram_bot_token").ok()
}

/// Load the state file from `.wh/telegram-state.json`, or return a default if absent.
fn load_state_file(state_dir: &Path) -> TelegramStateFile {
    let path = state_dir.join("telegram-state.json");
    if !path.exists() {
        return TelegramStateFile::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => TelegramStateFile::default(),
    }
}

/// Save the state file to `.wh/telegram-state.json`.
fn save_state_file(state_dir: &Path, state: &TelegramStateFile) -> Result<(), String> {
    std::fs::create_dir_all(state_dir).map_err(|e| format!("failed to create state dir: {e}"))?;
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| format!("failed to serialize state: {e}"))?;
    let path = state_dir.join("telegram-state.json");
    std::fs::write(&path, json).map_err(|e| format!("failed to write state file: {e}"))?;
    Ok(())
}

/// Call Telegram `getUpdates` using blocking reqwest. Returns discovered chats.
fn call_get_updates_blocking(bot_token: &str) -> Result<Vec<ResolvedChat>, String> {
    let url = format!(
        "https://api.telegram.org/bot{}/getUpdates?limit=100&allowed_updates={}",
        bot_token,
        url_encode("[\"my_chat_member\",\"message\",\"channel_post\"]"),
    );

    let client = reqwest::blocking::Client::new();
    let response = client
        .get(&url)
        .send()
        .map_err(|e| format!("Telegram API request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_else(|_| "unknown".to_string());
        return Err(format!("Telegram API returned HTTP {status}: {body}"));
    }

    let body: serde_json::Value = response
        .json()
        .map_err(|e| format!("Failed to parse Telegram response: {e}"))?;

    if body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let description = body
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!("Telegram API error: {description}"));
    }

    Ok(extract_chats(&body))
}

/// Call Telegram `createForumTopic` using blocking reqwest.
///
/// Returns `Ok((thread_id, None))` on success.
/// Returns `Ok((0, Some(new_chat_id)))` when Telegram signals a group→supergroup migration
/// via `migrate_to_chat_id` in the 400 response body — the caller should update state and retry.
fn create_forum_topic_blocking(
    bot_token: &str,
    chat_id: i64,
    topic_name: &str,
) -> Result<(i32, Option<i64>), String> {
    let url = format!("https://api.telegram.org/bot{bot_token}/createForumTopic");

    let client = reqwest::blocking::Client::new();
    let response = client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "name": topic_name,
        }))
        .send()
        .map_err(|e| format!("createForumTopic request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body_text = response.text().unwrap_or_else(|_| "unknown".to_string());
        // Telegram returns migrate_to_chat_id when a group was upgraded to a supergroup.
        if let Ok(body) = serde_json::from_str::<serde_json::Value>(&body_text) {
            if let Some(new_id) = body
                .get("parameters")
                .and_then(|p| p.get("migrate_to_chat_id"))
                .and_then(|v| v.as_i64())
            {
                return Ok((0, Some(new_id)));
            }
        }
        return Err(format!(
            "createForumTopic returned HTTP {status}: {body_text}"
        ));
    }

    let body: serde_json::Value = response
        .json()
        .map_err(|e| format!("Failed to parse createForumTopic response: {e}"))?;

    if body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let description = body
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!("createForumTopic API error: {description}"));
    }

    let thread_id = body
        .get("result")
        .and_then(|r| r.get("message_thread_id"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .ok_or_else(|| "createForumTopic response missing message_thread_id".to_string())?;

    Ok((thread_id, None))
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
    fn is_display_name_classification() {
        // @username → not display name
        assert!(!is_display_name("@ndohuu"));
        // Numeric → not display name
        assert!(!is_display_name("-1001234567890"));
        assert!(!is_display_name("42"));
        // Plain display name → is display name
        assert!(is_display_name("Wheelhouse Ops"));
        assert!(is_display_name("My Group"));
    }

    #[test]
    fn topic_key_format() {
        assert_eq!(topic_key(-100123, "General"), "-100123:General");
        assert_eq!(topic_key(42, "Topic: Sub"), "42:Topic: Sub");
    }

    #[test]
    fn state_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join(".wh");

        let mut state = TelegramStateFile::default();
        state.groups.insert("Test Group".to_string(), -100123);
        state.topics.insert(topic_key(-100123, "General"), 42);

        save_state_file(&state_dir, &state).unwrap();

        let loaded = load_state_file(&state_dir);
        assert_eq!(loaded.groups.get("Test Group"), Some(&-100123));
        assert_eq!(loaded.topics.get(&topic_key(-100123, "General")), Some(&42));
    }

    #[test]
    fn load_state_file_missing_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let state = load_state_file(&dir.path().join(".wh"));
        assert!(state.groups.is_empty());
        assert!(state.topics.is_empty());
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
