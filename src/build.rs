use std::env;

fn main() {
    let vars = [
        ("M3_BUILD", "\"debug\",\"release\",\"bench\""),
        ("M3_TARGET", "\"hw22\",\"hw23\",\"hw\",\"gem5\""),
        ("M3_LX", "\"1\""),
        ("M3_ROTS", "\"1\""),
    ];

    for (name, vals) in vars {
        println!("cargo::rerun-if-env-changed={}", name);
        println!("cargo::rustc-check-cfg=cfg({}, values({}))", name, vals);
        if let Ok(val) = env::var(name) {
            println!("cargo::rustc-env={}={}", name, val);
            println!("cargo::rustc-cfg={}=\"{}\"", name, val);
        }
    }

    println!("cargo::rustc-check-cfg=cfg(dylint_lib, values(\"m3_lints\"))");
}
