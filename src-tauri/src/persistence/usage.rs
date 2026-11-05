//! The `api_usage` table: what each paid API call used and (roughly) cost.

use rusqlite::{Connection, params};

use crate::domain::now_ms;
use crate::domain::usage::{UsagePeriod, UsageRow};
use crate::error::Result;
use crate::services::pricing;

pub struct NewUsage<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub kind: &'a str,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cached_tokens: u32,
    pub estimated: bool,
    pub latency_ms: u32,
    pub ok: bool,
}

pub fn record(conn: &Connection, u: &NewUsage) -> Result<()> {
    let cost = pricing::cost(u.provider, u.model, u.input_tokens, u.output_tokens, u.cached_tokens);
    conn.execute(
        "INSERT INTO api_usage (at, provider, model, kind, input_tokens, output_tokens, cached_tokens, estimated, latency_ms, ok, cost_usd)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            now_ms(),
            u.provider,
            u.model,
            u.kind,
            u.input_tokens,
            u.output_tokens,
            u.cached_tokens,
            u.estimated,
            u.latency_ms,
            u.ok,
            cost
        ],
    )?;
    Ok(())
}

pub fn summary(conn: &Connection, period: UsagePeriod) -> Result<Vec<UsageRow>> {
    let since = match period {
        UsagePeriod::Day => now_ms() - 24 * 3_600_000,
        UsagePeriod::Week => now_ms() - 7 * 24 * 3_600_000,
        UsagePeriod::All => 0,
    };
    let mut stmt = conn.prepare(
        "SELECT provider, model, kind, COUNT(*), SUM(ok = 0),
                SUM(input_tokens), SUM(output_tokens), SUM(cached_tokens),
                CAST(AVG(latency_ms) AS INTEGER), COALESCE(SUM(cost_usd), 0.0),
                SUM(cost_usd IS NULL), MAX(estimated)
         FROM api_usage WHERE at >= ?1
         GROUP BY provider, model, kind
         ORDER BY COALESCE(SUM(cost_usd), 0.0) DESC, COUNT(*) DESC",
    )?;
    let rows = stmt
        .query_map([since], |r| {
            Ok(UsageRow {
                provider: r.get(0)?,
                model: r.get(1)?,
                kind: r.get(2)?,
                calls: r.get(3)?,
                errors: r.get(4)?,
                input_tokens: r.get(5)?,
                output_tokens: r.get(6)?,
                cached_tokens: r.get(7)?,
                avg_latency_ms: r.get(8)?,
                cost_usd: r.get(9)?,
                unpriced_calls: r.get(10)?,
                estimated: r.get::<_, i64>(11)? != 0,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_conn;

    fn u<'a>(model: &'a str, kind: &'a str, input: u32, ok: bool) -> NewUsage<'a> {
        NewUsage {
            provider: "gemini",
            model,
            kind,
            input_tokens: input,
            output_tokens: 0,
            cached_tokens: 0,
            estimated: false,
            latency_ms: 100,
            ok,
        }
    }

    #[test]
    fn groups_by_model_and_kind_and_sums_cost() {
        let conn = test_conn();
        record(&conn, &u("gemini-embedding-2", "embed_index", 1_000_000, true)).unwrap();
        record(&conn, &u("gemini-embedding-2", "embed_index", 500_000, true)).unwrap();
        record(&conn, &u("gemini-embedding-2", "embed_query", 10, false)).unwrap();
        record(&conn, &u("some-unknown-model", "chat", 5, true)).unwrap();

        let rows = summary(&conn, UsagePeriod::All).unwrap();
        assert_eq!(rows.len(), 3);
        let index = rows.iter().find(|r| r.kind == "embed_index").unwrap();
        assert_eq!((index.calls, index.input_tokens), (2, 1_500_000));
        let expect = pricing::cost("gemini", "gemini-embedding-2", 1_500_000, 0, 0).unwrap();
        assert!(expect > 0.0 && (index.cost_usd - expect).abs() < 1e-9, "{}", index.cost_usd);
        let query = rows.iter().find(|r| r.kind == "embed_query").unwrap();
        assert_eq!(query.errors, 1);
        let unknown = rows.iter().find(|r| r.model == "some-unknown-model").unwrap();
        assert_eq!((unknown.unpriced_calls, unknown.cost_usd), (1, 0.0));
    }

    #[test]
    fn periods_leave_out_older_calls() {
        let conn = test_conn();
        record(&conn, &u("gemini-embedding-2", "embed_index", 10, true)).unwrap();
        conn.execute("UPDATE api_usage SET at = at - 3 * 24 * 3600 * 1000", []).unwrap();
        assert!(summary(&conn, UsagePeriod::Day).unwrap().is_empty());
        assert_eq!(summary(&conn, UsagePeriod::Week).unwrap().len(), 1);
    }
}
