#![cfg_attr(all(target_os = "windows", not(dev)), windows_subsystem = "windows")]

fn main() {
    usagebar_lib::run();
}
