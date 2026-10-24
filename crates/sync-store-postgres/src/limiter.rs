//! The limiter of PostgreSQL mode: its counters live in `rate_counters`, so
//! every instance on the database counts against one allowance, and windows
//! follow the database's clock, so instances agree on them.

use std::time::Duration;

use sqlx::PgPool;
use sync_store::{BoxFut, Error, Limit, Limiter, Result, Taken};

/// Counters in the database, shared by every instance.
#[derive(Clone, Debug)]
pub struct PgLimiter {
    pool: PgPool,
}

fn db(error: sqlx::Error) -> Error {
    Error::internal(error)
}

fn int(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

impl PgLimiter {
    /// A limiter on `pool`'s database.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The database's clock, in milliseconds since the Unix epoch.
    async fn now_ms(&self) -> Result<u64> {
        let row = sqlx::query!(
            r#"SELECT (extract(epoch FROM clock_timestamp()) * 1000)::bigint AS "now!""#
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        u64::try_from(row.now).map_err(|_| Error::internal("the database's clock is before 1970"))
    }

    /// Deletes the counters of every window that has ended.
    /// [`Limiter::take`] also drops a key's ended windows when the key
    /// starts a new one.
    async fn prune_now(&self) -> Result<u64> {
        let now = int(self.now_ms().await?);
        let done = sqlx::query!("DELETE FROM rate_counters WHERE window_ends <= $1", now)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(done.rows_affected())
    }

    async fn take_now(&self, key: &[u8], cost: u64, limit: Limit) -> Result<Taken> {
        let now = self.now_ms().await?;
        let ends = limit.window_ends(now);
        let refused = Taken::Refused {
            retry_after: Duration::from_millis(ends - now),
        };
        if cost > limit.allowance {
            return Ok(refused);
        }
        let (window, ends_at) = (int(limit.window_ms()), int(ends));
        let (cost_at, allowance) = (int(cost), int(limit.allowance));
        // One statement adds the cost only while it fits, so instances that
        // take from one key at once never pass the allowance together. The
        // window's length is part of the row's key, as in memory, so two
        // limits on one key never share a count.
        let row = sqlx::query!(
            "INSERT INTO rate_counters (key, window_ms, window_ends, used) VALUES ($1, $2, $3, $4)
             ON CONFLICT (key, window_ms, window_ends)
             DO UPDATE SET used = rate_counters.used + excluded.used
             WHERE rate_counters.used + excluded.used <= $5
             RETURNING used",
            key,
            window,
            ends_at,
            cost_at,
            allowance
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        match row {
            Some(row) => {
                if row.used == cost_at {
                    // The key's first take in this window: its ended
                    // windows go.
                    let now_at = int(now);
                    sqlx::query!(
                        "DELETE FROM rate_counters WHERE key = $1 AND window_ends <= $2",
                        key,
                        now_at
                    )
                    .execute(&self.pool)
                    .await
                    .map_err(db)?;
                }
                Ok(Taken::Allowed)
            }
            None => Ok(refused),
        }
    }
}

impl Limiter for PgLimiter {
    fn take<'a>(&'a self, key: &'a [u8], cost: u64, limit: Limit) -> BoxFut<'a, Result<Taken>> {
        Box::pin(self.take_now(key, cost, limit))
    }

    fn prune(&self) -> BoxFut<'_, Result<u64>> {
        Box::pin(self.prune_now())
    }
}
