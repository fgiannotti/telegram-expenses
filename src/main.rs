mod config;
mod dates;
mod db;
mod handlers;
mod money;
mod parse;

use crate::db::Db;
use crate::handlers::{handle_text, split_message, Reply};
use anyhow::{anyhow, Result};
use chrono::Utc;
use frankenstein::client_ureq::Bot;
use frankenstein::input_file::{FileUpload, InputFile};
use frankenstein::methods::{GetUpdatesParams, SendDocumentParams, SendMessageParams};
use frankenstein::types::AllowedUpdate;
use frankenstein::updates::{Update, UpdateContent};
use frankenstein::TelegramApi;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

const DEFAULT_DB_PATH: &str = "expenses.db";
const POLL_TIMEOUT_SECONDS: u32 = 30;
const RETRY_DELAY: Duration = Duration::from_secs(5);
/// After this many consecutive failures on the same update we skip it, so one
/// poisonous message cannot wedge the bot forever.
const MAX_ATTEMPTS: u32 = 3;

fn main() -> ExitCode {
    if let Err(error) = run() {
        eprintln!("expense-bot: {error:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

struct Settings {
    token: String,
    allowed_user_id: i64,
    db_path: PathBuf,
}

fn run() -> Result<()> {
    // Only for local development; on the server systemd supplies the
    // environment from /etc/expense-bot.env.
    dotenvy::dotenv().ok();

    let settings = settings()?;
    let mut db = Db::open(&settings.db_path)?;
    println!("expense-bot: database {}", settings.db_path.display());
    println!(
        "expense-bot: accepting messages from user {} only",
        settings.allowed_user_id
    );

    let bot = Bot::new(&settings.token);
    poll(&bot, &mut db, settings.allowed_user_id)
}

fn settings() -> Result<Settings> {
    let token = required("BOT_TOKEN", "the token @BotFather gave you for this bot")?;
    let raw_id = required(
        "ALLOWED_USER_ID",
        "your numeric Telegram user id, which @userinfobot will tell you",
    )?;
    let allowed_user_id: i64 = raw_id
        .parse()
        .map_err(|_| anyhow!("ALLOWED_USER_ID must be a whole number, got {raw_id:?}"))?;
    let db_path = optional("DB_PATH").map_or_else(|| PathBuf::from(DEFAULT_DB_PATH), PathBuf::from);

    Ok(Settings {
        token,
        allowed_user_id,
        db_path,
    })
}

fn required(name: &str, description: &str) -> Result<String> {
    optional(name).ok_or_else(|| anyhow!("{name} is not set; it must hold {description}"))
}

fn optional(name: &str) -> Option<String> {
    let value = env::var(name).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn poll(bot: &Bot, db: &mut Db, allowed_user_id: i64) -> Result<()> {
    let mut offset: Option<i64> = None;
    let mut attempts: u32 = 0;

    loop {
        let params = GetUpdatesParams {
            offset,
            limit: None,
            timeout: Some(POLL_TIMEOUT_SECONDS),
            allowed_updates: Some(vec![AllowedUpdate::Message]),
        };

        let updates = match bot.get_updates(&params) {
            Ok(response) => response.result,
            Err(error) => {
                eprintln!("expense-bot: getUpdates failed: {error}");
                sleep(RETRY_DELAY);
                continue;
            }
        };

        for update in updates {
            // Confirming an update to Telegram means never seeing it again, so
            // the offset only moves once the expense is committed.
            let confirmed = Some(i64::from(update.update_id) + 1);
            match handle_update(bot, db, allowed_user_id, update) {
                Ok(()) => {
                    offset = confirmed;
                    attempts = 0;
                }
                Err(error) => {
                    attempts += 1;
                    eprintln!("expense-bot: update failed (attempt {attempts}): {error:#}");
                    if attempts >= MAX_ATTEMPTS {
                        eprintln!("expense-bot: skipping the update after {attempts} attempts");
                        offset = confirmed;
                        attempts = 0;
                    } else {
                        sleep(RETRY_DELAY);
                    }
                    // Stop the batch; the next poll replays from `offset`.
                    break;
                }
            }
        }
    }
}

/// Returns `Err` only when the expense could not be written, which is the one
/// case worth replaying the update for.
fn handle_update(bot: &Bot, db: &mut Db, allowed_user_id: i64, update: Update) -> Result<()> {
    let UpdateContent::Message(message) = update.content else {
        return Ok(());
    };
    let Some(sender) = message.from.as_ref() else {
        return Ok(());
    };
    if i64::try_from(sender.id) != Ok(allowed_user_id) {
        eprintln!("expense-bot: dropped a message from user {}", sender.id);
        return Ok(());
    }

    let chat_id = message.chat.id;
    let Some(text) = message.text.as_deref() else {
        send_text(bot, chat_id, "Solo entiendo mensajes de texto.");
        return Ok(());
    };

    match handle_text(db, text, Utc::now())? {
        Reply::Text(reply) => send_text(bot, chat_id, &reply),
        Reply::Csv {
            filename,
            content,
            caption,
        } => send_csv(bot, chat_id, &filename, &content, &caption),
    }
    Ok(())
}

/// Send failures are logged and dropped rather than propagated: the row is
/// already committed, so replaying the update would insert it twice.
fn send_text(bot: &Bot, chat_id: i64, text: &str) {
    for chunk in split_message(text) {
        let params = SendMessageParams::builder()
            .chat_id(chat_id)
            .text(chunk)
            .build();
        if let Err(error) = bot.send_message(&params) {
            eprintln!("expense-bot: sendMessage failed: {error}");
        }
    }
}

fn send_csv(bot: &Bot, chat_id: i64, filename: &str, content: &str, caption: &str) {
    let path = env::temp_dir().join(filename);
    if let Err(error) = fs::write(&path, content) {
        eprintln!("expense-bot: could not write {}: {error}", path.display());
        send_text(bot, chat_id, "No pude generar el CSV.");
        return;
    }

    let params = SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(FileUpload::InputFile(InputFile { path: path.clone() }))
        .caption(caption)
        .build();
    if let Err(error) = bot.send_document(&params) {
        eprintln!("expense-bot: sendDocument failed: {error}");
        send_text(bot, chat_id, "No pude enviar el CSV.");
    }

    if let Err(error) = fs::remove_file(&path) {
        eprintln!(
            "expense-bot: could not remove {}: {error}",
            path.display()
        );
    }
}
