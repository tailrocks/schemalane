use schemalane_macros::embed_migrations;

embed_migrations!("does-not-exist");

fn main() {}
