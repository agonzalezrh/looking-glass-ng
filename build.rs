fn main() {
    // Tell cargo to link against liblgmp
    println!("cargo:rustc-link-lib=lgmp");
    println!("cargo:rustc-link-search=/usr/local/lib");
    // Rebuild if the library changes
    println!("cargo:rerun-if-changed=/usr/local/lib/liblgmp.a");
}
