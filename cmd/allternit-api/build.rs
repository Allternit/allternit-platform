fn main() {
    // refinery's embed_migrations!("migrations") reads SQL files at compile time,
    // but nothing told cargo to recompile this crate when only a migration changes.
    println!("cargo:rerun-if-changed=migrations");
    // Same for sqlx::migrate!("./migrations_selfhost_pg") (selfhost store).
    println!("cargo:rerun-if-changed=migrations_selfhost_pg");
}
