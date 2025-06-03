use std::env;

fn main() {
    let build = env::var("M3_BUILD").unwrap();
    let target = env::var("M3_TARGET").unwrap();
    println!("cargo::rerun-if-env-changed=M3_BUILD");
    println!("cargo::rerun-if-env-changed=M3_TARGET");
    println!("cargo::rustc-env=M3_BUILD={}", build);
    println!("cargo::rustc-cfg=M3_BUILD=\"{}\"", build);
    println!("cargo::rustc-env=M3_TARGET={}", target);
    println!("cargo::rustc-cfg=M3_TARGET=\"{}\"", target);
}
