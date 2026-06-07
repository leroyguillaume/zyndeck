fn main() {
    // `sqlx::migrate!` embeds the migrations at compile time, so adding a new
    // `.sql` file does not, on its own, make Cargo rebuild this crate — the
    // macro keeps expanding to the stale set. Re-running the build script (and
    // thus recompiling) whenever the directory changes forces a fresh expansion.
    println!("cargo:rerun-if-changed=migrations");
}
