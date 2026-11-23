fn main() {
    // The locale catalog is embedded by `rust_i18n::i18n!`, which does not
    // register it as a build input. Without this, regenerating the YAML after a
    // locale change would silently keep the previous catalog in the binary.
    println!("cargo:rerun-if-changed=locales/ui.yml");
}
