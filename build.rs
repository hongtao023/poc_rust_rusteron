use std::path::Path;

fn main() {
    let schema_path = Path::new("schemas/bench.xml");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let output_path = Path::new(&out_dir).join("bench_sbe.rs");

    println!("cargo:rerun-if-changed=schemas/bench.xml");

    let generated_code = ironsbe_codegen::generate_from_file(schema_path)
        .expect("Failed to generate SBE codecs from schemas/bench.xml");

    std::fs::write(&output_path, generated_code)
        .expect("Failed to write generated SBE code");
}
