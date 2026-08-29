# 如何不重启设备更新调度

1. 准备新版本的zip压缩包
2. 打开webui，点击关闭调度
3. 退出webui，打开 `MT管理器` 等支持 root 去权限的文件管理器
4. 打开 `/data/adb/modules/chiri` 目录，使用新版本zip中的文件替换旧文件
5. 以 root 权限执行 `action.sh` 或在root管理器里点击Action按钮，通常会在1秒左右完成，部分root管理器可能不会自动退出已执行完的界面

6.（可选）检查 `logs/daemon.log` 确认调度已启动
