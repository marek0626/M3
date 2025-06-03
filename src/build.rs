use std::env;

fn main() {
    for e in ["M3_BUILD", "M3_TARGET", "M3_LX"] {
        println!("cargo::rerun-if-env-changed={}", e);
        if let Ok(val) = env::var(e) {
            println!("cargo::rustc-env={}={}", e, val);
            println!("cargo::rustc-cfg={}=\"{}\"", e, val);
        }
    }
}
