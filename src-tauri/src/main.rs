// Prevents an additional console window on Windows in release, where the
// console would otherwise hang around and kill the app if the user closes it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    quota_lib::run();
}
