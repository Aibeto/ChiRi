mod zip_ext;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::{Parser, Subcommand};
use fs_extra::{dir, file};
use xshell::{cmd, Shell};
use zip::{write::FileOptions, CompressionMethod};

use crate::zip_ext::zip_create_from_directory_with_options;

#[derive(Parser)]
#[command(name = "xtask", about = "Yumi Build System")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 编译项目并打包
    #[command(alias = "b")]
    Build {
        /// 不打包 zip，仅组装模块目录（CI 使用，交由 GitHub 下载时自动打包）
        #[arg(long)]
        no_pack: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // 初始化 xshell
    let sh = Shell::new()?;

    match cli.command {
        Commands::Build { no_pack } => build(&sh, no_pack)?,
    }

    Ok(())
}

fn cal_git_code(sh: &Shell) -> Result<usize> {
    // xshell 极大地简化了获取命令 stdout 的过程
    let output = cmd!(sh, "git rev-list --count HEAD").read()?;
    Ok(output.trim().parse::<usize>()?)
}

fn get_date() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M").to_string()
}

/// 从 module/module.prop 读取 name 与 version，作为产物命名依据。
/// module.prop 为 KEY=VALUE 格式（Magisk/KernelSU 模块规范）。
fn read_module_prop() -> Result<(String, String)> {
    let content = fs::read_to_string("module/module.prop")?;
    let mut name = String::new();
    let mut version = String::new();
    for line in content.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "name" => name = value.trim().to_string(),
            "version" => version = value.trim().to_string(),
            _ => {}
        }
    }
    if name.is_empty() || version.is_empty() {
        anyhow::bail!("module/module.prop 缺少 name 或 version 字段");
    }
    Ok((name, version))
}

fn build(sh: &Shell, no_pack: bool) -> Result<()> {
    let temp_dir = temp_dir();

    // 产物命名以 module.prop 为准（name-version-提交数-日期）
    let (module_name, module_version) = read_module_prop()?;
    let base_name = format!(
        "{}-{}-{}-{}",
        module_name,
        module_version,
        cal_git_code(sh)?,
        get_date()
    );

    // 1. 清理并重建临时目录
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir)?;

    // 2. 编译 WebUI
    build_webui(sh)?;

    // 3. 编译 Rust 核心
    build_core(sh)?;

    // 4. 拷贝 module 目录内容
    let module_dir = Path::new("module").to_path_buf();
    dir::copy(
        &module_dir,
        &temp_dir,
        &dir::CopyOptions::new().overwrite(true).content_only(true),
    )?;

    if temp_dir.join(".gitignore").exists() {
        fs::remove_file(temp_dir.join(".gitignore"))?;
    }

    // 5. 组装 bin 目录
    let bin_path = temp_dir.join("core").join("bin");
    fs::create_dir_all(&bin_path)?;

    file::copy(
        aarch64_bin_path(),
        bin_path.join("yumi"),
        &file::CopyOptions::new().overwrite(true),
    )?;

    let webroot_dir = temp_dir.join("webroot");
    dir::copy(
        Path::new("webui").join("dist"),
        &webroot_dir,
        &dir::CopyOptions::new().overwrite(true).content_only(true),
    )?;

    // 6. 产物输出
    let output_dir = Path::new("output");
    fs::create_dir_all(output_dir)?; // 确保 output 目录存在

    if no_pack {
        // 不打包：把组装好的模块目录移出临时目录，交 CI/GitHub 代为打包，
        // 目录名即为 GitHub artifact 名（下载时自动生成同名 .zip）。
        let final_dir = output_dir.join(&base_name);
        let _ = fs::remove_dir_all(&final_dir);
        fs::rename(&temp_dir, &final_dir)?;
        println!("模块目录已生成: {}", final_dir.display());
    } else {
        let zip_path = output_dir.join(format!("{base_name}.zip"));
        println!("开始打包: {}", zip_path.display());

        let options: FileOptions<'_, ()> = FileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(9));

        zip_create_from_directory_with_options(&zip_path, &temp_dir, |_| options)?;
    }

    println!("构建成功！");
    Ok(())
}

fn temp_dir() -> PathBuf {
    Path::new("output").join(".temp")
}

fn aarch64_bin_path() -> PathBuf {
    Path::new("target")
        .join("aarch64-linux-android")
        .join("release")
        .join("yumi")
}

fn build_core(sh: &Shell) -> Result<()> {
    println!("正在编译 Rust Core...");
    // push_env 会在当前作用域内设置环境变量，离开作用域自动恢复
    let _env = sh.push_env("RUSTFLAGS", "-C default-linker-libraries");
    cmd!(sh, "cargo +nightly ndk --platform 26 -t arm64-v8a build -Z build-std -r").run()?;
    Ok(())
}

fn build_webui(sh: &Shell) -> Result<()> {
    println!("正在编译 WebUI...");
    // push_dir 类似于 cd，离开作用域后会自动切回原目录
    let _dir = sh.push_dir("webui");
    cmd!(sh, "npm run build").run()?;
    Ok(())
}
