use sea_orm_migration::prelude::*;

#[tokio::main]
async fn main() {
    // The sea-orm-migration CLI looks for DATABASE_URL. Mirror COMIC_DATABASE_URL
    // into it so users only configure the COMIC_-prefixed variable.
    if std::env::var_os("DATABASE_URL").is_none()
        && let Ok(url) = std::env::var("COMIC_DATABASE_URL")
    {
        // SAFETY: setting an env var pre-runtime is fine in the migration
        // binary because no other thread is running.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("DATABASE_URL", url)
        };
    }
    cli::run_cli(migration::Migrator).await;
}
