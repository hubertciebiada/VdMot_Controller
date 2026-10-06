// Hands the ESP-IDF build settings of esp-idf-sys (cfgs, linker arguments) to this crate.
fn main() {
    embuild::espidf::sysenv::output();
}
