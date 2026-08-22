//! Registers `schema.graphql` with cynic so every derive in `src/` is
//! checked against the real SDL at compile time (platform P.9 Q2).

fn main() {
    cynic_codegen::register_schema("platform")
        .from_sdl_file("schema.graphql")
        .expect("schema.graphql parses")
        .as_default()
        .expect("register the platform schema as default");
}
