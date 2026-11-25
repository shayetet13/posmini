use std::{str::FromStr, time::Duration};

use anyhow::{Context, Result};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

use crate::{auth, config::Config, menu};

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Shop {
    pub id: i64,
    pub slug: String,
    pub name: String,
}

const SEED_PRODUCTS: [(&str, &str, i64); 9] = [
    ("กาแฟ", "อเมริกาโน่ร้อน", 4_000),
    ("กาแฟ", "อเมริกาโน่เย็น", 4_500),
    ("กาแฟ", "ลาเต้ร้อน", 5_000),
    ("กาแฟ", "ลาเต้เย็น", 5_500),
    ("กาแฟ", "คาปูชิโน่", 5_500),
    ("ปาท่องโก๋", "ปาท่องโก๋ 1 คู่", 1_000),
    ("ปาท่องโก๋", "ปาท่องโก๋ 3 คู่", 2_500),
    ("ปาท่องโก๋", "ปาท่องโก๋สังขยา", 1_500),
    ("ปาท่องโก๋", "ปาท่องโก๋ 5 คู่", 4_000),
];

pub async fn connect(url: &str) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(url)
        .context("invalid DATABASE_URL")?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .context("open database")?;
    sqlx::migrate!()
        .run(&pool)
        .await
        .context("run migrations")?;
    Ok(pool)
}

pub async fn seed(pool: &SqlitePool, cfg: &Config) -> Result<Shop> {
    sqlx::query("INSERT OR IGNORE INTO shops (slug, name) VALUES (?, ?)")
        .bind(&cfg.shop_slug)
        .bind(&cfg.shop_name)
        .execute(pool)
        .await?;
    let shop: Shop = sqlx::query_as("SELECT id, slug, name FROM shops WHERE slug = ?")
        .bind(&cfg.shop_slug)
        .fetch_one(pool)
        .await?;

    let admin_exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE shop_id = ? AND username = ?")
            .bind(shop.id)
            .bind(&cfg.admin_user)
            .fetch_one(pool)
            .await?;
    if admin_exists == 0 {
        let hash = auth::hash_password(&cfg.admin_password)?;
        sqlx::query("INSERT INTO users (shop_id, username, password_hash) VALUES (?, ?, ?)")
            .bind(shop.id)
            .bind(&cfg.admin_user)
            .bind(hash)
            .execute(pool)
            .await?;
    }

    let product_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM products WHERE shop_id = ?")
        .bind(shop.id)
        .fetch_one(pool)
        .await?;
    if product_count == 0 {
        for (sort, (category, name, price)) in (0_i64..).zip(SEED_PRODUCTS) {
            sqlx::query(
                "INSERT INTO products (shop_id, category, name, price_satang, sort) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(shop.id)
            .bind(category)
            .bind(name)
            .bind(price)
            .bind(sort)
            .execute(pool)
            .await?;
        }
    }
    // Menus made before cards had colours get one each.
    menu::ensure_colors(pool, shop.id).await?;
    Ok(shop)
}
