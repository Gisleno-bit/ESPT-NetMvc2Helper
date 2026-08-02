[package]
name = "mvc-netmon"
version = "0.1.0"
edition = "2021"

[dependencies]
eframe = { version = "0.27", default-features = false, features = ["glow", "default_fonts"] }
egui = "0.27"

[profile.release]
opt-level = 2
