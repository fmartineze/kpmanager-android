// Solo para probar en el escritorio durante el desarrollo; la app real es la de Android.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    kpmanager_android_lib::run()
}
