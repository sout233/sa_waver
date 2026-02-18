// src/main.rs

use sa_waver::WaverPlugin;
use nih_plug::nih_export_standalone;

fn main() {
    nih_export_standalone::<WaverPlugin>();
}
