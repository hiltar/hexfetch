use std::env;
use std::path::Path;

fn main() {
    embuild::espidf::sysenv::output();
    let out_dir = env::var("OUT_DIR").unwrap();
    let src_partitions = Path::new("partitions.csv");
    let dst_partitions = Path::new(&out_dir).join("partitions.csv");
    
    if src_partitions.exists() {
        std::fs::copy(src_partitions, dst_partitions)
            .expect("Failed to copy partitions.csv to build directory");
    }
}
