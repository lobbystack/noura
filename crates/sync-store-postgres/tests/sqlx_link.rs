//! Link check for trial S0: sqlx's PostgreSQL driver reads a connection URL.
//! Ticket 45 runs the store against a real server.

use sqlx::postgres::PgConnectOptions;

#[test]
fn sqlx_reads_a_postgres_connection_url() {
    let options: PgConnectOptions = "postgres://noura@db.example:5433/sync"
        .parse()
        .expect("valid URL");

    assert_eq!(options.get_host(), "db.example");
    assert_eq!(options.get_port(), 5433);
    assert_eq!(options.get_username(), "noura");
    assert_eq!(options.get_database(), Some("sync"));
}
