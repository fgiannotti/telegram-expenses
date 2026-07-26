use crate::config::Category;
use crate::dates::{iso, month_key};
use anyhow::{Context, Result};
use chrono::NaiveDate;
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;
use std::str::FromStr;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS expenses (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  amount      INTEGER NOT NULL,
  category    TEXT    NOT NULL,
  description TEXT    NOT NULL DEFAULT '',
  local_date  TEXT    NOT NULL,
  week_start  TEXT    NOT NULL,
  created_at  INTEGER NOT NULL,
  raw         TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_expenses_local_date ON expenses(local_date);
CREATE INDEX IF NOT EXISTS idx_expenses_week ON expenses(week_start, category);
";

const SELECT_COLUMNS: &str = "id, amount, category, description, local_date";

pub struct Db {
    conn: Connection,
}

/// A row about to be written. `local_date` and `week_start` are computed by the
/// caller in Buenos Aires time so no query ever has to do timezone math.
#[derive(Debug, Clone)]
pub struct NewExpense<'a> {
    pub amount: i64,
    pub category: Category,
    pub description: &'a str,
    pub raw: &'a str,
    pub local_date: NaiveDate,
    pub week_start: NaiveDate,
    pub created_at_ms: i64,
}

/// Everything the reply needs, read inside the same transaction as the insert.
/// `week_before`/`week_after` straddle this expense, which is what makes the
/// limit alert fire exactly once without storing a "warned" flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddOutcome {
    pub id: i64,
    pub month_total: i64,
    pub week_before: i64,
    pub week_after: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryTotal {
    pub category: Category,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpenseRow {
    pub id: i64,
    pub amount: i64,
    pub category: Category,
    pub description: String,
    pub local_date: NaiveDate,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("opening database at {}", path.display()))?;
        Self::prepare(conn)
    }

    /// Tests use a throwaway database so nothing touches the filesystem.
    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(conn: Connection) -> Result<Self> {
        // journal_mode returns the resulting mode as a row, so it needs a query
        // rather than pragma_update.
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .context("enabling WAL")?;
        if !mode.eq_ignore_ascii_case("wal") && !mode.eq_ignore_ascii_case("memory") {
            eprintln!("warning: journal_mode is {mode}, expected wal");
        }
        conn.pragma_update(None, "synchronous", "FULL")
            .context("setting synchronous = FULL")?;
        conn.execute_batch(SCHEMA).context("creating schema")?;
        Ok(Self { conn })
    }

    pub fn add_expense(&mut self, expense: &NewExpense<'_>) -> Result<AddOutcome> {
        let day = iso(expense.local_date);
        let week = iso(expense.week_start);
        let month = month_key(expense.local_date);
        let category = expense.category.as_str();

        let tx = self.conn.transaction()?;
        let week_before: i64 = tx.query_row(
            "SELECT COALESCE(SUM(amount), 0) FROM expenses WHERE week_start = ?1 AND category = ?2",
            params![&week, category],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO expenses
               (amount, category, description, local_date, week_start, created_at, raw)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                expense.amount,
                category,
                expense.description,
                &day,
                &week,
                expense.created_at_ms,
                expense.raw,
            ],
        )?;
        let id = tx.last_insert_rowid();
        let month_total: i64 = tx.query_row(
            "SELECT COALESCE(SUM(amount), 0) FROM expenses WHERE substr(local_date, 1, 7) = ?1",
            params![&month],
            |row| row.get(0),
        )?;
        tx.commit()?;

        Ok(AddOutcome {
            id,
            month_total,
            week_before,
            week_after: week_before + expense.amount,
        })
    }

    pub fn totals_for_day(&self, date: NaiveDate) -> Result<Vec<CategoryTotal>> {
        self.totals(
            "SELECT category, SUM(amount) FROM expenses
             WHERE local_date = ?1 GROUP BY category ORDER BY SUM(amount) DESC",
            &iso(date),
        )
    }

    pub fn totals_for_week(&self, week_start: NaiveDate) -> Result<Vec<CategoryTotal>> {
        self.totals(
            "SELECT category, SUM(amount) FROM expenses
             WHERE week_start = ?1 GROUP BY category ORDER BY SUM(amount) DESC",
            &iso(week_start),
        )
    }

    pub fn totals_for_month(&self, month: &str) -> Result<Vec<CategoryTotal>> {
        self.totals(
            "SELECT category, SUM(amount) FROM expenses
             WHERE substr(local_date, 1, 7) = ?1 GROUP BY category ORDER BY SUM(amount) DESC",
            month,
        )
    }

    fn totals(&self, sql: &str, key: &str) -> Result<Vec<CategoryTotal>> {
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement.query_map(params![key], |row| {
            let stored: String = row.get(0)?;
            Ok(CategoryTotal {
                category: stored_category(&stored),
                total: row.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Most recent first, so the ids in the reply match what `/borrar` targets.
    pub fn recent(&self, limit: u32) -> Result<Vec<ExpenseRow>> {
        let sql = format!("SELECT {SELECT_COLUMNS} FROM expenses ORDER BY id DESC LIMIT ?1");
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params![limit], read_expense)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Oldest first: this feeds the CSV export, where chronological order reads
    /// better than the reverse.
    pub fn rows_for_month(&self, month: &str) -> Result<Vec<ExpenseRow>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM expenses
             WHERE substr(local_date, 1, 7) = ?1 ORDER BY id"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params![month], read_expense)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_last(&mut self) -> Result<Option<ExpenseRow>> {
        let last: Option<i64> = self
            .conn
            .query_row("SELECT MAX(id) FROM expenses", [], |row| row.get(0))?;
        match last {
            Some(id) => self.delete(id),
            None => Ok(None),
        }
    }

    pub fn delete(&mut self, id: i64) -> Result<Option<ExpenseRow>> {
        let sql = format!("SELECT {SELECT_COLUMNS} FROM expenses WHERE id = ?1");
        let tx = self.conn.transaction()?;
        let row = tx
            .query_row(&sql, params![id], read_expense)
            .optional()?;
        if row.is_some() {
            tx.execute("DELETE FROM expenses WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(row)
    }
}

fn read_expense(row: &Row<'_>) -> rusqlite::Result<ExpenseRow> {
    let stored_category_text: String = row.get(2)?;
    let stored_date: String = row.get(4)?;
    let local_date = NaiveDate::parse_from_str(&stored_date, "%Y-%m-%d")
        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(4, Type::Text, Box::new(error)))?;
    Ok(ExpenseRow {
        id: row.get(0)?,
        amount: row.get(1)?,
        category: stored_category(&stored_category_text),
        description: row.get(3)?,
        local_date,
    })
}

/// Only `Category::as_str` ever writes this column, so the parse always
/// succeeds; the fallback keeps a hand-edited row from breaking a whole report.
fn stored_category(text: &str) -> Category {
    Category::from_str(text).unwrap_or(Category::Otros)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    fn add(db: &mut Db, amount: i64, category: Category, day: &str, week: &str) -> AddOutcome {
        db.add_expense(&NewExpense {
            amount,
            category,
            description: "algo",
            raw: "raw",
            local_date: date(day),
            week_start: date(week),
            created_at_ms: 1_700_000_000_000,
        })
        .unwrap()
    }

    #[test]
    fn insert_reports_ids_and_running_totals() {
        let mut db = Db::open_in_memory().unwrap();

        let first = add(&mut db, 4_500, Category::Cafe, "2026-07-20", "2026-07-20");
        assert_eq!(first.id, 1);
        assert_eq!(first.week_before, 0);
        assert_eq!(first.week_after, 4_500);
        assert_eq!(first.month_total, 4_500);

        let second = add(&mut db, 500, Category::Cafe, "2026-07-21", "2026-07-20");
        assert_eq!(second.id, 2);
        assert_eq!(second.week_before, 4_500);
        assert_eq!(second.week_after, 5_000);
        assert_eq!(second.month_total, 5_000);
    }

    #[test]
    fn week_totals_are_scoped_to_one_category_and_one_week() {
        let mut db = Db::open_in_memory().unwrap();
        add(&mut db, 60_000, Category::Comida, "2026-07-20", "2026-07-20");
        // Same week, different category.
        add(&mut db, 90_000, Category::Super, "2026-07-21", "2026-07-20");
        // Same category, previous week.
        add(&mut db, 80_000, Category::Comida, "2026-07-15", "2026-07-13");

        let outcome = add(&mut db, 1_000, Category::Comida, "2026-07-22", "2026-07-20");
        assert_eq!(outcome.week_before, 60_000);
        assert_eq!(outcome.week_after, 61_000);
    }

    #[test]
    fn month_total_spans_categories_but_not_months() {
        let mut db = Db::open_in_memory().unwrap();
        add(&mut db, 1_000, Category::Cafe, "2026-06-30", "2026-06-29");
        add(&mut db, 2_000, Category::Super, "2026-07-01", "2026-06-29");
        let outcome = add(&mut db, 3_000, Category::Salud, "2026-07-31", "2026-07-27");
        assert_eq!(outcome.month_total, 5_000);
    }

    #[test]
    fn totals_group_by_category_largest_first() {
        let mut db = Db::open_in_memory().unwrap();
        add(&mut db, 1_000, Category::Cafe, "2026-07-20", "2026-07-20");
        add(&mut db, 9_000, Category::Super, "2026-07-20", "2026-07-20");
        add(&mut db, 500, Category::Cafe, "2026-07-20", "2026-07-20");

        let day = db.totals_for_day(date("2026-07-20")).unwrap();
        assert_eq!(
            day,
            vec![
                CategoryTotal { category: Category::Super, total: 9_000 },
                CategoryTotal { category: Category::Cafe, total: 1_500 },
            ]
        );
        assert_eq!(db.totals_for_week(date("2026-07-20")).unwrap().len(), 2);
        assert_eq!(db.totals_for_month("2026-07").unwrap().len(), 2);
        assert!(db.totals_for_day(date("2026-07-21")).unwrap().is_empty());
    }

    #[test]
    fn recent_returns_newest_first_and_respects_the_limit() {
        let mut db = Db::open_in_memory().unwrap();
        for amount in 1..=5 {
            add(&mut db, amount * 100, Category::Otros, "2026-07-20", "2026-07-20");
        }
        let rows = db.recent(3).unwrap();
        assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), vec![5, 4, 3]);
        assert_eq!(rows[0].amount, 500);
        assert_eq!(rows[0].category, Category::Otros);
        assert_eq!(rows[0].description, "algo");
        assert_eq!(rows[0].local_date, date("2026-07-20"));
    }

    #[test]
    fn delete_last_removes_the_newest_row() {
        let mut db = Db::open_in_memory().unwrap();
        add(&mut db, 100, Category::Cafe, "2026-07-20", "2026-07-20");
        add(&mut db, 200, Category::Cafe, "2026-07-20", "2026-07-20");

        let removed = db.delete_last().unwrap().expect("a row to delete");
        assert_eq!(removed.id, 2);
        assert_eq!(removed.amount, 200);
        assert_eq!(db.recent(10).unwrap().len(), 1);
    }

    #[test]
    fn delete_by_id_and_the_empty_cases() {
        let mut db = Db::open_in_memory().unwrap();
        assert_eq!(db.delete_last().unwrap(), None);
        assert_eq!(db.delete(42).unwrap(), None);

        add(&mut db, 100, Category::Cafe, "2026-07-20", "2026-07-20");
        add(&mut db, 200, Category::Super, "2026-07-20", "2026-07-20");
        let removed = db.delete(1).unwrap().expect("a row to delete");
        assert_eq!(removed.amount, 100);
        assert_eq!(db.recent(10).unwrap().len(), 1);
        assert_eq!(db.delete(1).unwrap(), None);
    }

    #[test]
    fn deleting_back_under_the_limit_lets_the_next_crossing_warn_again() {
        let mut db = Db::open_in_memory().unwrap();
        add(&mut db, 99_000, Category::Comida, "2026-07-20", "2026-07-20");
        let crossing = add(&mut db, 2_000, Category::Comida, "2026-07-21", "2026-07-20");
        assert_eq!((crossing.week_before, crossing.week_after), (99_000, 101_000));

        db.delete(crossing.id).unwrap();
        let again = add(&mut db, 2_000, Category::Comida, "2026-07-22", "2026-07-20");
        assert_eq!((again.week_before, again.week_after), (99_000, 101_000));
    }

    #[test]
    fn export_rows_are_chronological_and_month_scoped() {
        let mut db = Db::open_in_memory().unwrap();
        add(&mut db, 100, Category::Cafe, "2026-07-01", "2026-06-29");
        add(&mut db, 200, Category::Cafe, "2026-08-01", "2026-07-27");
        let rows = db.rows_for_month("2026-07").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].amount, 100);
    }
}
