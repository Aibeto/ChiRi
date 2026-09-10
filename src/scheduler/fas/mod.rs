//! mod.rs: [mods]

mod controller;
mod fps_window;
mod pid;
mod policy_controller;
mod gear_state;
mod frame_pipeline;
mod pid_jank;
mod policy_mgmt;

pub use controller::FasController;
