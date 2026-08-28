use std::process::Command;
use std::env;
use std::path::{Path, PathBuf};

/// 检查 bpf-linker 是否已存在于 PATH（CI 预装 / 系统已装则跳过安装）
fn bpf_linker_on_path() -> bool {
    env::var_os("PATH")
        .map(|p| env::split_paths(&p).any(|d| d.join("bpf-linker").is_file()))
        .unwrap_or(false)
}

/// 确保 bpf-linker 可用：已存在则跳过，否则安装并严格检查结果。
/// 之前版本用 `.status()?` 直接透传（只检查进程能否启动，不检查 exit code），
/// install 静默失败后 yumi-ebpf 链接阶段才会暴露 "linker bpf-linker not found"。
fn ensure_bpf_linker(tools_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let tools_bin = tools_dir.join("bin");
    let linker = tools_bin.join("bpf-linker");
    // 1) PATH 中已有 bpf-linker（如 CI 已用 cargo-binstall 安装预编译版）直接复用
    if bpf_linker_on_path() {
        println!("cargo:warning=✅ bpf-linker 已存在于 PATH，跳过安装");
        return Ok(linker);
    }
    // 2) 之前的构建已安装过
    if linker.exists() {
        println!("cargo:warning=✅ bpf-linker 已就绪: {}", linker.display());
        return Ok(linker);
    }

    println!("cargo:warning=⏳ 正在安装 bpf-linker (可能需要数分钟)...");
    let install = Command::new("cargo")
        .args([
            "install", "bpf-linker", "--force",
            "--root", tools_dir.to_str().ok_or("tools_dir 非 UTF-8")?,
            "--target-dir", tools_dir.to_str().ok_or("tools_dir 非 UTF-8")?,
        ])
        .env_remove("RUSTUP_TOOLCHAIN")
        .output();
    match install {
        Ok(out) if !out.status.success() => {
            // 打印完整 stderr，方便 CI 定位（如系统缺少 LLVM 时 llvm-sys 构建失败）
            eprintln!(
                "cargo install bpf-linker 失败 (status: {})\n--- stderr ---\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
            return Err("bpf-linker 安装失败，请检查上方 stderr（常见原因：缺少系统 LLVM，需安装 llvm/clang）".into());
        }
        Err(e) => return Err(format!("无法执行 cargo install bpf-linker: {e}").into()),
        _ => {}
    }
    if !linker.exists() {
        return Err(format!("bpf-linker 安装完成但未找到产物: {}", linker.display()).into());
    }
    println!("cargo:warning=✅ bpf-linker 安装完成: {}", linker.display());
    Ok(linker)
}

/// 构建 yumi-ebpf BPF 程序，参照 frame-analyzer 的 build_ebpf()
fn build_ebpf() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ebpf_dir = manifest_dir.join("yumi-ebpf");
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let target_dir = out_dir.join("ebpf_target");
    let tools_dir = out_dir.join("ebpf_tools");

    // 监控 ebpf crate 变化
    println!("cargo:rerun-if-changed={}", ebpf_dir.join("Cargo.toml").display());
    println!("cargo:rerun-if-changed={}", ebpf_dir.join("src").display());

    // 1. 安装 bpf-linker（参照 frame-analyzer install_ebpf_linker），严格校验
    let linker_bin = ensure_bpf_linker(&tools_dir)?;

    // 2. 编译 BPF 程序（在 yumi-ebpf 目录中，避免 workspace 干扰）
    let mut ebpf_args = vec![
        "--target", "bpfel-unknown-none",
        "-Z", "build-std=core",
        "--target-dir", target_dir.to_str().unwrap(),
    ];

    #[cfg(not(debug_assertions))]
    ebpf_args.push("--release");

    let status = Command::new("cargo")
        .arg("build")
        .args(&ebpf_args)
        .current_dir(&ebpf_dir)
        .env_remove("RUSTUP_TOOLCHAIN")
        .env("PATH", add_path(linker_bin.parent().unwrap())?)
        .status()?;

    if !status.success() {
        panic!("yumi-ebpf 编译失败");
    }

    // 3. 产物路径（binary crate 直接输出到 <target>/<profile>/<name>，无 deps/hash）
    #[cfg(debug_assertions)]
    let profile = "debug";
    #[cfg(not(debug_assertions))]
    let profile = "release";

    let built_obj = target_dir
        .join("bpfel-unknown-none")
        .join(profile)
        .join("yumi-ebpf"); // binary crate 保留原始包名中的连字符

    Ok(built_obj)
}

fn add_path(add: &std::path::Path) -> Result<String, std::env::VarError> {
    let path = env::var("PATH")?;
    Ok(format!("{}:{}", add.display(), path))
}

fn main() {
    match build_ebpf() {
        Ok(bpf_obj) => {
            println!("cargo:warning=✅ yumi-ebpf 编译成功: {}", bpf_obj.display());
        }
        Err(e) => {
            panic!("yumi-ebpf 编译失败: {e}");
        }
    }
}
