use sqlx::migrate::Migrator;
use sqlx::{Pool, Sqlite, SqlitePool};
use std::path::Path;

use crate::orderbook::{order_side_to_db_value, LimitOrder};

static MIGRATOR: Migrator = sqlx::migrate!();

pub async fn setup_database(db_path: &str) -> Result<Pool<Sqlite>, sqlx::Error> {
    let database_url = format!("sqlite:{}", db_path);
    let pool = SqlitePool::connect(&database_url).await?;

    // Run migrations
    if Path::new(&format!("./{}", db_path)).exists() {
        MIGRATOR.run(&pool).await?;
    }

    Ok(pool)
}

// Function to insert an offer into the database
pub async fn insert_offer(offer: &LimitOrder, db_pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let query = "INSERT INTO orders (side, asset, amount, price, status, user_id, timestamp, nonce) VALUES (?, ?, ?, ?, ?, ?, ?, ?)";
    sqlx::query(query)
        .bind(order_side_to_db_value(&offer.side)) // Convert enum to string
        .bind(&offer.asset)
        .bind(offer.amount.to_string()) // Convert U256 to string
        .bind(offer.price.to_string())
        .bind(&offer.status.to_string()) // Assuming you also convert OrderStatus to string
        .bind(offer.user_id.to_string()) // Convert PeerId to string
        .bind(offer.timestamp)
        .bind(offer.nonce.to_string()) // Convert U256 to string
        .execute(db_pool)
        .await?;
    Ok(())
}
