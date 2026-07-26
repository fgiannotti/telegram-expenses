use crate::config::{weekly_limit, Category, ALL_CATEGORIES};
use crate::dates::{day_month, iso, local_date, month_key, parse_month_key, week_start};
use crate::db::{CategoryTotal, Db, ExpenseRow, NewExpense};
use crate::money::format_amount;
use crate::parse::{parse_message, ParseResult};
use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use std::mem;

const DEFAULT_RECENT: u32 = 10;
const MAX_RECENT: u32 = 100;

/// Telegram rejects a sendMessage body longer than this.
const TELEGRAM_MAX_CHARS: usize = 4096;

/// What the bot wants to send back. Handlers build this from data alone, which
/// is what lets every reply below be asserted in a unit test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Text(String),
    Csv {
        filename: String,
        content: String,
        caption: String,
    },
}

pub fn handle_text(db: &mut Db, text: &str, now: DateTime<Utc>) -> Result<Reply> {
    let trimmed = text.trim();
    if trimmed.starts_with('/') {
        handle_command(db, trimmed, now)
    } else {
        handle_expense(db, trimmed, now)
    }
}

fn handle_expense(db: &mut Db, text: &str, now: DateTime<Utc>) -> Result<Reply> {
    match parse_message(text) {
        ParseResult::Expense {
            amount,
            category,
            description,
        } => {
            let today = local_date(now);
            let week = week_start(now);
            let outcome = db.add_expense(&NewExpense {
                amount,
                category,
                description: &description,
                raw: text,
                local_date: today,
                week_start: week,
                created_at_ms: now.timestamp_millis(),
            })?;

            let mut reply = format!(
                "Anotado #{}: {} {}",
                outcome.id,
                format_amount(amount),
                category
            );
            if !description.is_empty() {
                reply.push_str(" - ");
                reply.push_str(&description);
            }
            reply.push_str(&format!(
                "\nMes {}: {}",
                month_key(today),
                format_amount(outcome.month_total)
            ));
            if let Some(alert) = limit_alert(category, week, outcome.week_before, outcome.week_after)
            {
                reply.push_str("\n\n");
                reply.push_str(&alert);
            }
            Ok(Reply::Text(reply))
        }
        ParseResult::UnknownCategory(word) => Ok(Reply::Text(format!(
            "No conozco la categoria \"{word}\".\nValidas: {}",
            category_list()
        ))),
        ParseResult::MissingCategory => Ok(Reply::Text(format!(
            "Falta la categoria.\n{}",
            format_hint()
        ))),
        ParseResult::NoAmount => Ok(Reply::Text(format!(
            "No encontre el monto.\n{}",
            format_hint()
        ))),
        ParseResult::Empty => Ok(Reply::Text(format_hint())),
    }
}

/// The weekly-limit warning, or `None` when this expense is not the one that
/// crosses the line. Deriving it from the two totals means it fires exactly
/// once per crossing and needs no stored state.
pub fn limit_alert(
    category: Category,
    week_start: NaiveDate,
    before: i64,
    after: i64,
) -> Option<String> {
    let limit = weekly_limit(category)?;
    if before >= limit || after < limit {
        return None;
    }
    Some(format!(
        "Limite semanal de {} superado\nSemana del {}: {} de {} (+{})",
        category.as_str().to_uppercase(),
        day_month(week_start),
        format_amount(after),
        format_amount(limit),
        format_amount(after - limit),
    ))
}

fn handle_command(db: &mut Db, text: &str, now: DateTime<Utc>) -> Result<Reply> {
    let (head, rest) = match text.split_once(char::is_whitespace) {
        Some((head, rest)) => (head, rest.trim()),
        None => (text, ""),
    };
    // Group chats deliver commands as `/hoy@MiBot`.
    let command = head.split('@').next().unwrap_or(head).to_lowercase();

    match command.as_str() {
        "/hoy" => today(db, now),
        "/semana" => this_week(db, now),
        "/mes" => month(db, rest, now),
        "/ultimos" => latest(db, rest),
        "/borrar" => remove(db, rest),
        "/export" => export(db, rest, now),
        "/ayuda" | "/help" | "/start" => Ok(Reply::Text(help_text())),
        _ => Ok(Reply::Text(format!(
            "No conozco ese comando.\n\n{}",
            help_text()
        ))),
    }
}

fn today(db: &Db, now: DateTime<Utc>) -> Result<Reply> {
    let date = local_date(now);
    let totals = db.totals_for_day(date)?;
    Ok(Reply::Text(report(
        &format!("Hoy {}", day_month(date)),
        &totals,
        false,
    )))
}

fn this_week(db: &Db, now: DateTime<Utc>) -> Result<Reply> {
    let start = week_start(now);
    let totals = db.totals_for_week(start)?;
    Ok(Reply::Text(report(
        &format!("Semana del {}", day_month(start)),
        &totals,
        true,
    )))
}

fn month(db: &Db, argument: &str, now: DateTime<Utc>) -> Result<Reply> {
    let Some(key) = month_argument(argument, now) else {
        return Ok(Reply::Text(
            "Mes invalido. Usa /mes o /mes YYYY-MM (por ejemplo /mes 2026-07).".to_string(),
        ));
    };
    let totals = db.totals_for_month(&key)?;
    Ok(Reply::Text(report(&format!("Mes {key}"), &totals, false)))
}

fn latest(db: &Db, argument: &str) -> Result<Reply> {
    let limit = if argument.is_empty() {
        DEFAULT_RECENT
    } else {
        match argument.parse::<u32>() {
            Ok(value) if value > 0 => value.min(MAX_RECENT),
            Ok(_) | Err(_) => {
                return Ok(Reply::Text(
                    "Cantidad invalida. Usa /ultimos o /ultimos <n>.".to_string(),
                ))
            }
        }
    };

    let rows = db.recent(limit)?;
    if rows.is_empty() {
        return Ok(Reply::Text("Todavia no hay gastos registrados.".to_string()));
    }
    let mut out = format!("Ultimos {}:", rows.len());
    for row in &rows {
        out.push('\n');
        out.push_str(&format_row(row));
    }
    Ok(Reply::Text(out))
}

fn remove(db: &mut Db, argument: &str) -> Result<Reply> {
    if argument.is_empty() {
        return Ok(Reply::Text(match db.delete_last()? {
            Some(row) => format!("Borrado {}", format_row(&row)),
            None => "No hay gastos para borrar.".to_string(),
        }));
    }
    let Ok(id) = argument.parse::<i64>() else {
        return Ok(Reply::Text(
            "Id invalido. Usa /borrar o /borrar <id>.".to_string(),
        ));
    };
    Ok(Reply::Text(match db.delete(id)? {
        Some(row) => format!("Borrado {}", format_row(&row)),
        None => format!("No existe el gasto #{id}."),
    }))
}

fn export(db: &Db, argument: &str, now: DateTime<Utc>) -> Result<Reply> {
    let Some(key) = month_argument(argument, now) else {
        return Ok(Reply::Text(
            "Mes invalido. Usa /export o /export YYYY-MM (por ejemplo /export 2026-07).".to_string(),
        ));
    };
    let rows = db.rows_for_month(&key)?;
    if rows.is_empty() {
        return Ok(Reply::Text(format!("No hay gastos en {key}.")));
    }
    let total: i64 = rows.iter().map(|row| row.amount).sum();
    Ok(Reply::Csv {
        filename: format!("gastos-{key}.csv"),
        content: to_csv(&rows),
        caption: format!(
            "Gastos de {key}: {} en {} registros",
            format_amount(total),
            rows.len()
        ),
    })
}

fn month_argument(argument: &str, now: DateTime<Utc>) -> Option<String> {
    if argument.is_empty() {
        Some(month_key(local_date(now)))
    } else {
        parse_month_key(argument)
    }
}

/// One shared shape for `/hoy`, `/semana` and `/mes`. `with_limits` adds the
/// `/ $100.000` progress suffix, which only makes sense for a weekly report.
fn report(title: &str, totals: &[CategoryTotal], with_limits: bool) -> String {
    if totals.is_empty() {
        return format!("{title}: sin gastos");
    }
    let sum: i64 = totals.iter().map(|entry| entry.total).sum();
    let mut out = format!("{title}: {}", format_amount(sum));
    for entry in totals {
        out.push_str(&format!(
            "\n- {}: {}",
            entry.category,
            format_amount(entry.total)
        ));
        if with_limits {
            if let Some(limit) = weekly_limit(entry.category) {
                out.push_str(&format!(" / {}", format_amount(limit)));
            }
        }
    }
    out
}

fn format_row(row: &ExpenseRow) -> String {
    let mut out = format!(
        "#{} {} {} {}",
        row.id,
        day_month(row.local_date),
        format_amount(row.amount),
        row.category
    );
    if !row.description.is_empty() {
        out.push_str(" - ");
        out.push_str(&row.description);
    }
    out
}

fn to_csv(rows: &[ExpenseRow]) -> String {
    let mut out = String::from("id,fecha,categoria,descripcion,monto\n");
    for row in rows {
        out.push_str(&format!(
            "{},{},{},{},{}\n",
            row.id,
            iso(row.local_date),
            row.category,
            csv_field(&row.description),
            row.amount
        ));
    }
    out
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Breaks a reply into sendable messages. `/ultimos 100` can exceed Telegram's
/// limit, and the API rejects the whole message rather than truncating it.
pub fn split_message(text: &str) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0;

    for line in lines_within_limit(text) {
        let line_len = line.chars().count();
        if current_len > 0 && current_len + 1 + line_len > TELEGRAM_MAX_CHARS {
            chunks.push(mem::take(&mut current));
            current_len = 0;
        }
        if current_len > 0 {
            current.push('\n');
            current_len += 1;
        }
        current.push_str(&line);
        current_len += line_len;
    }

    if !current.is_empty() || chunks.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// The lines of `text`, with any single line that already exceeds one message
/// hard-split so the caller can assume every line fits.
fn lines_within_limit(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut piece = String::new();
        let mut piece_len = 0;
        for character in line.chars() {
            if piece_len == TELEGRAM_MAX_CHARS {
                out.push(mem::take(&mut piece));
                piece_len = 0;
            }
            piece.push(character);
            piece_len += 1;
        }
        out.push(piece);
    }
    out
}

fn category_list() -> String {
    ALL_CATEGORIES
        .iter()
        .map(|category| category.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_hint() -> String {
    format!(
        "Formato: <monto> <categoria> [descripcion]\n\
         Ejemplos: 4500 cafe con Juan | 1.500 transporte subte | 1,5k super\n\
         Categorias: {}",
        category_list()
    )
}

fn help_text() -> String {
    format!(
        "{}\n\n\
         Comandos:\n\
         /hoy - total de hoy por categoria\n\
         /semana - total de la semana (lunes a domingo)\n\
         /mes [YYYY-MM] - total del mes\n\
         /ultimos [n] - ultimos gastos con su id\n\
         /borrar [id] - borra el ultimo gasto, o el que indiques\n\
         /export [YYYY-MM] - descarga el mes en CSV\n\
         /ayuda - este mensaje",
        format_hint()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dates::monday_of;
    use crate::db::AddOutcome;

    const LIMIT: i64 = 100_000;

    fn monday() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 20).unwrap()
    }

    #[test]
    fn alert_fires_on_the_expense_that_crosses_the_limit() {
        let alert = limit_alert(Category::Comida, monday(), 96_000, 104_500)
            .expect("the crossing expense must warn");
        assert_eq!(
            alert,
            "Limite semanal de COMIDA superado\n\
             Semana del 20/07: $104.500 de $100.000 (+$4.500)"
        );
    }

    #[test]
    fn alert_fires_when_the_total_lands_exactly_on_the_limit() {
        assert!(limit_alert(Category::Comida, monday(), 99_000, LIMIT).is_some());
    }

    #[test]
    fn alert_stays_quiet_below_the_limit() {
        assert_eq!(limit_alert(Category::Comida, monday(), 0, 99_999), None);
    }

    #[test]
    fn alert_does_not_repeat_once_already_over() {
        // The crossing already happened on an earlier expense.
        assert_eq!(limit_alert(Category::Comida, monday(), LIMIT, 104_500), None);
        assert_eq!(
            limit_alert(Category::Comida, monday(), 150_000, 160_000),
            None
        );
    }

    #[test]
    fn categories_without_a_limit_never_alert() {
        for category in ALL_CATEGORIES {
            if category != Category::Comida {
                assert_eq!(limit_alert(category, monday(), 0, 500_000), None);
            }
        }
    }

    // --- end-to-end through the handler, still with no network ---

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    fn text_of(reply: Reply) -> String {
        match reply {
            Reply::Text(text) => text,
            Reply::Csv { .. } => panic!("expected a text reply"),
        }
    }

    fn say(db: &mut Db, text: &str, at: &str) -> String {
        text_of(handle_text(db, text, utc(at)).unwrap())
    }

    #[test]
    fn logging_an_expense_confirms_with_id_and_month_total() {
        let mut db = db();
        let reply = say(&mut db, "4500 cafe con Juan", "2026-07-20T15:00:00Z");
        assert_eq!(
            reply,
            "Anotado #1: $4.500 cafe - con Juan\nMes 2026-07: $4.500"
        );
    }

    #[test]
    fn an_expense_with_no_description_omits_the_dash() {
        let mut db = db();
        assert_eq!(
            say(&mut db, "12000 super", "2026-07-20T15:00:00Z"),
            "Anotado #1: $12.000 super\nMes 2026-07: $12.000"
        );
    }

    #[test]
    fn the_limit_warning_is_appended_to_the_confirmation_once() {
        let mut db = db();
        say(&mut db, "96000 comida", "2026-07-20T15:00:00Z");

        let crossing = say(&mut db, "8500 comida asado", "2026-07-21T15:00:00Z");
        assert!(
            crossing.ends_with(
                "Limite semanal de COMIDA superado\n\
                 Semana del 20/07: $104.500 de $100.000 (+$4.500)"
            ),
            "unexpected reply: {crossing}"
        );

        let after = say(&mut db, "1000 comida", "2026-07-22T15:00:00Z");
        assert!(!after.contains("Limite semanal"), "warned twice: {after}");
    }

    #[test]
    fn the_monday_boundary_resets_the_weekly_total() {
        let mut db = db();
        // Sunday 23:00 in Buenos Aires, still the week of the 13th.
        say(&mut db, "99000 comida", "2026-07-20T02:00:00Z");
        // Monday 00:00 local: new week, so this does not cross anything.
        let reply = say(&mut db, "5000 comida", "2026-07-20T03:00:00Z");
        assert!(!reply.contains("Limite semanal"), "unexpected: {reply}");

        assert_eq!(
            say(&mut db, "/semana", "2026-07-20T03:00:00Z"),
            "Semana del 20/07: $5.000\n- comida: $5.000 / $100.000"
        );
    }

    #[test]
    fn parse_failures_produce_specific_help() {
        let mut db = db();
        let at = "2026-07-20T15:00:00Z";
        assert!(say(&mut db, "4500 nafta", at).starts_with("No conozco la categoria \"nafta\"."));
        assert!(say(&mut db, "4500", at).starts_with("Falta la categoria."));
        assert!(say(&mut db, "cafe con Juan", at).starts_with("No encontre el monto."));
        assert!(say(&mut db, "   ", at).starts_with("Formato:"));
    }

    #[test]
    fn day_and_month_reports() {
        let mut db = db();
        let at = "2026-07-20T15:00:00Z";
        assert_eq!(say(&mut db, "/hoy", at), "Hoy 20/07: sin gastos");

        say(&mut db, "4500 cafe", at);
        say(&mut db, "12000 super", at);
        assert_eq!(
            say(&mut db, "/hoy", at),
            "Hoy 20/07: $16.500\n- super: $12.000\n- cafe: $4.500"
        );
        assert_eq!(
            say(&mut db, "/mes", at),
            "Mes 2026-07: $16.500\n- super: $12.000\n- cafe: $4.500"
        );
        assert_eq!(say(&mut db, "/mes 2026-06", at), "Mes 2026-06: sin gastos");
        assert!(say(&mut db, "/mes julio", at).starts_with("Mes invalido."));
    }

    #[test]
    fn only_the_weekly_report_shows_limit_progress() {
        let mut db = db();
        let at = "2026-07-20T15:00:00Z";
        say(&mut db, "64000 comida", at);
        assert_eq!(
            say(&mut db, "/semana", at),
            "Semana del 20/07: $64.000\n- comida: $64.000 / $100.000"
        );
        assert_eq!(say(&mut db, "/hoy", at), "Hoy 20/07: $64.000\n- comida: $64.000");
    }

    #[test]
    fn listing_and_deleting() {
        let mut db = db();
        let at = "2026-07-20T15:00:00Z";
        assert_eq!(
            say(&mut db, "/ultimos", at),
            "Todavia no hay gastos registrados."
        );
        assert_eq!(say(&mut db, "/borrar", at), "No hay gastos para borrar.");

        say(&mut db, "4500 cafe con Juan", at);
        say(&mut db, "12000 super", at);
        assert_eq!(
            say(&mut db, "/ultimos", at),
            "Ultimos 2:\n#2 20/07 $12.000 super\n#1 20/07 $4.500 cafe - con Juan"
        );
        assert_eq!(say(&mut db, "/ultimos 1", at), "Ultimos 1:\n#2 20/07 $12.000 super");
        assert!(say(&mut db, "/ultimos cero", at).starts_with("Cantidad invalida."));

        assert_eq!(
            say(&mut db, "/borrar 1", at),
            "Borrado #1 20/07 $4.500 cafe - con Juan"
        );
        assert_eq!(say(&mut db, "/borrar 1", at), "No existe el gasto #1.");
        assert_eq!(say(&mut db, "/borrar", at), "Borrado #2 20/07 $12.000 super");
    }

    #[test]
    fn export_produces_a_csv_attachment() {
        let mut db = db();
        let at = "2026-07-20T15:00:00Z";
        assert_eq!(
            say(&mut db, "/export", at),
            "No hay gastos en 2026-07."
        );

        say(&mut db, "4500 cafe con Juan, y Ana", at);
        say(&mut db, "12000 super", at);
        match handle_text(&mut db, "/export 2026-07", utc(at)).unwrap() {
            Reply::Csv {
                filename,
                content,
                caption,
            } => {
                assert_eq!(filename, "gastos-2026-07.csv");
                assert_eq!(caption, "Gastos de 2026-07: $16.500 en 2 registros");
                assert_eq!(
                    content,
                    "id,fecha,categoria,descripcion,monto\n\
                     1,2026-07-20,cafe,\"con Juan, y Ana\",4500\n\
                     2,2026-07-20,super,,12000\n"
                );
            }
            Reply::Text(text) => panic!("expected a csv reply, got {text}"),
        }
    }

    #[test]
    fn commands_tolerate_the_bot_suffix_and_unknown_verbs() {
        let mut db = db();
        let at = "2026-07-20T15:00:00Z";
        assert_eq!(say(&mut db, "/hoy@MiGastoBot", at), "Hoy 20/07: sin gastos");
        assert!(say(&mut db, "/ayuda", at).starts_with("Formato:"));
        assert!(say(&mut db, "/pizza", at).starts_with("No conozco ese comando."));
    }

    #[test]
    fn week_start_helper_agrees_with_the_report_header() {
        // Guards the report title against a stray off-by-one day.
        let start = week_start(utc("2026-07-26T20:00:00Z"));
        assert_eq!(start, monday_of(NaiveDate::from_ymd_opt(2026, 7, 26).unwrap()));
        assert_eq!(day_month(start), "20/07");
    }

    #[test]
    fn short_replies_are_sent_as_one_message() {
        assert_eq!(split_message("hola"), vec!["hola".to_string()]);
        assert_eq!(split_message(""), vec![String::new()]);
    }

    #[test]
    fn long_replies_split_on_line_boundaries() {
        let line = "#1 20/07 $4.500 cafe con Juan";
        let text = vec![line; 300].join("\n");
        assert!(text.chars().count() > TELEGRAM_MAX_CHARS);

        let chunks = split_message(&text);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.chars().count() <= TELEGRAM_MAX_CHARS);
            // No line was cut in half.
            for chunk_line in chunk.lines() {
                assert_eq!(chunk_line, line);
            }
        }
        assert_eq!(chunks.join("\n"), text);
    }

    #[test]
    fn a_single_oversized_line_is_hard_split() {
        let text = "a".repeat(TELEGRAM_MAX_CHARS + 10);
        let chunks = split_message(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chars().count(), TELEGRAM_MAX_CHARS);
        assert_eq!(chunks[1].chars().count(), 10);
    }

    #[test]
    fn splitting_counts_characters_not_bytes() {
        // Multi-byte characters must not be counted twice or the chunks come
        // out well under the limit for no reason.
        let text = "ñ".repeat(TELEGRAM_MAX_CHARS);
        assert_eq!(split_message(&text).len(), 1);
    }

    #[test]
    fn add_outcome_totals_drive_the_alert_decision() {
        let outcome = AddOutcome {
            id: 1,
            month_total: 104_500,
            week_before: 96_000,
            week_after: 104_500,
        };
        assert!(limit_alert(
            Category::Comida,
            monday(),
            outcome.week_before,
            outcome.week_after
        )
        .is_some());
    }
}
