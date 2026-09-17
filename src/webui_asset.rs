//! webui_asset.rs - 内嵌 WebUI 资产的启动还原: [restore]
//!
//! 构建期由 build.rs 把 webui/dist 全量嵌进二进制（清单见 OUT_DIR/webui_assets.rs）；
//! daemon 启动时把内嵌副本与模块 webroot/ 逐文件比对，只覆盖「缺失或不一致」的文件：
//! 界面文件不在可写面暴露，被篡改的部分每次开机都回到出厂内容。
//! meta.yaml 的 `nofix: true` 会跳过整个操作（判定与日志在 main.rs）。
use std::fs;
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/webui_assets.rs"));

/// 把内嵌资产还原到 `root/webroot/`，返回被覆盖（含新建）的文件数。
/// 内容一致的文件跳过（避免每次开机无谓擦写）；单文件失败跳过、不中断整体。
pub fn restore_webroot(root: &Path) -> usize {
    if !EMBEDDED {
        return 0;
    }
    let dir = root.join("webroot");
    let mut restored = 0usize;
    for (rel, bytes) in FILES {
        let path = dir.join(rel);
        if fs::read(&path).is_ok_and(|cur| cur == *bytes) {
            continue;
        }
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if crate::common::write_file_no_panic(&path, bytes) {
            restored += 1;
        }
    }
    restored
}
