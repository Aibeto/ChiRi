#!/system/bin/sh
#
# yumi 模块启动脚本 (service.sh)
#

# 1. 等待系统启动完成
until [ "$(getprop sys.boot_completed)" = "1" ]; do
  sleep 1
done

# 2. 定义路径
[ -z "$MODDIR" ] && MODDIR=${0%/*}

DAEMON_PATH="$MODDIR/core/bin/yumi"
SCRIPTS_DIR="$MODDIR/scripts"
LOG_DIR="$MODDIR/logs"
LOG_FILE="$LOG_DIR/service.log"

# 确保日志目录存在
mkdir -p "$LOG_DIR"

# 禁用 OPPO/OnePlus/Realme 的 Oiface
# if [ "$(getprop persist.sys.oiface.enable)" = "1" ]; then
#   setprop persist.sys.oiface.enable 0
#   echo "$(date): Oiface disabled." >> "$LOG_FILE"
# fi

# 禁用小米的 Joyose 服务
# PACKAGE_NAME="com.xiaomi.joyose"
# if pm list packages -e | grep -q "$PACKAGE_NAME"; then
#   pm disable-user "$PACKAGE_NAME" >/dev/null 2>&1
#   pm clear "$PACKAGE_NAME" >/dev/null 2>&1
#   echo "$(date): Joyose service disabled and data cleared." >> "$LOG_FILE"
# fi

# 3. 清理旧进程（含旧看门狗）：重新执行本脚本（模块热更新/管理器重载）时
#    若只 killall yumi，旧看门狗仍存活并在 3s 后把 daemon 再拉起——与新看门狗
#    形成双 daemon 实例，devimp/status/daemon 日志各写两份。先按 pid 文件终止
#    旧看门狗再清 daemon。
if [ -f "$LOG_DIR/watchdog.pid" ]; then
  kill "$(cat "$LOG_DIR/watchdog.pid" 2>/dev/null)" 2>/dev/null
  rm -f "$LOG_DIR/watchdog.pid"
fi
killall -9 yumi > /dev/null 2>&1

# 4. 设置权限
chmod 755 "$DAEMON_PATH"
if [ -d "$SCRIPTS_DIR" ]; then
  chmod -R 755 "$SCRIPTS_DIR"
fi

# 5. 调用禁用 boost 脚本
# if [ -f "$SCRIPTS_DIR/disable_boost.sh" ]; then
#   echo "$(date): Executing disable_boost.sh" >> "$LOG_FILE"
#   "$SCRIPTS_DIR/disable_boost.sh"
# else
#   echo "$(date): disable_boost.sh not found" >> "$LOG_FILE"
# fi

# 6. 启动 yumi 看门狗（崩溃自动重启，卸载时退出）
# 看门狗记录自身 PID 到 logs/watchdog.pid，供 WebUI「关闭调度」定位并终止。
# 退出条件：存在卸载标记 .uninstalling（卸载中）或主进程二进制被删除（卸载完成）。
# 崩溃/异常退出不满足退出条件，3 秒后自动拉起。
# 注意：旧写法 "$1" || exit 0 在 yumi 崩溃（返回非 0）时会直接让看门狗退出、无法自愈，已修正。
# 使用 setsid 而非 nohup，确保进程完全脱离父进程组，防止关闭界面导致服务终止。

# 检测 setsid 可用性，优先使用 BusyBox 的 setsid
SETSID_CMD=""
if command -v setsid >/dev/null 2>&1; then
  SETSID_CMD="setsid"
elif [ -n "$BUSYBOX" ] && "$BUSYBOX" setsid true >/dev/null 2>&1; then
  SETSID_CMD="$BUSYBOX setsid"
fi

# 看门狗启动命令（直接执行 sh -c，而非通过函数，确保 setsid 可正常工作）
WATCHDOG_CMD="sh -c '
  PIDFILE=\"\$1\"; DAEMON=\"\$2\"; FLAG=\"\$3\"
  echo \$\$ > \"\$PIDFILE\"
  while :; do
    [ -f \"\$FLAG\" ] && break      # 卸载标记 → 退出，不残留
    [ -f \"\$DAEMON\" ] || break    # 二进制被删 → 退出，不残留
    \"\$DAEMON\"                    # 崩溃/退出后返回，sleep 后再拉起
    sleep 3
  done
  rm -f \"\$PIDFILE\"
  exit 0
' sh \"$LOG_DIR/watchdog.pid\" \"$DAEMON_PATH\" \"$MODDIR/.uninstalling\" > /dev/null 2>&1"

# 启动看门狗，优先使用 setsid 脱离父进程组
if [ -n "$SETSID_CMD" ]; then
  $SETSID_CMD sh -c "$WATCHDOG_CMD"
else
  # fallback: 使用 nohup（兼容性更好，但可能无法完全脱离进程组）
  nohup sh -c "$WATCHDOG_CMD" > /dev/null 2>&1 &
fi

# 方式 B: 调试模式（启动失败时用这个排查，输出到 logs/boot_error.log）
# nohup sh -c 'PIDFILE="$1"; DAEMON="$2"; FLAG="$3"; echo $$ > "$PIDFILE"; while :; do [ -f "$FLAG" ] && break; [ -f "$DAEMON" ] || break; "$DAEMON"; sleep 3; done; rm -f "$PIDFILE"; exit 0' sh "$LOG_DIR/watchdog.pid" "$DAEMON_PATH" "$MODDIR/.uninstalling" > "$LOG_DIR/boot_error.log" 2>&1 &