#!/system/bin/sh
#
# ChiRi 模块 action 脚本：手动启动/重启调度
# KernelSU/Magisk 在用户点击模块「Action」按钮时执行本脚本。
# 作用：显式终止旧看门狗与主进程 → 重新拉起看门狗与主进程 → 分步打印消息。
# 说明：action 阶段无 ui_print（那是安装期函数），统一用 log() 输出到 stdout 与 service.log。
#

# 0. 定义路径
[ -z "$MODDIR" ] && MODDIR=${0%/*}

DAEMON_PATH="$MODDIR/core/bin/yumi"
LOG_DIR="$MODDIR/logs"
LOG_FILE="$LOG_DIR/service.log"
PID_FILE="$LOG_DIR/watchdog.pid"
STOP_FLAG="$MODDIR/.uninstalling"

mkdir -p "$LOG_DIR"

# 分步消息：同时打印到 stdout（KernelSU action 弹窗可见）与日志文件
log() { echo "$(date): $*"; echo "$(date): $*" >> "$LOG_FILE"; }

# 1. 终止旧看门狗与主进程（确保不残留重复实例，消除竞态）
log "stopping old watchdog and daemon..."
[ -f "$PID_FILE" ] && kill "$(cat "$PID_FILE" 2>/dev/null)" 2>/dev/null
killall -9 yumi > /dev/null 2>&1
rm -f "$PID_FILE"
sleep 1
log "stopped."

# 2. 设置权限
chmod 755 "$DAEMON_PATH"

# 3. 启动 yumi 看门狗（崩溃自动重启，卸载时退出）
# 看门狗记录自身 PID，供后续 action/WebUI「关闭调度」定位并终止。
# 使用 setsid 而非 nohup，确保进程完全脱离父进程组，防止关闭界面导致服务终止。

# 检测 setsid 可用性，优先使用 BusyBox 的 setsid
SETSID_CMD=""
if command -v setsid >/dev/null 2>&1; then
  SETSID_CMD="setsid"
elif [ -x "/data/adb/magisk/busybox" ] && "/data/adb/magisk/busybox" setsid true >/dev/null 2>&1; then
  SETSID_CMD="/data/adb/magisk/busybox setsid"
elif [ -x "/data/adb/ksu/bin/busybox" ] && "/data/adb/ksu/bin/busybox" setsid true >/dev/null 2>&1; then
  SETSID_CMD="/data/adb/ksu/bin/busybox setsid"
elif [ -x "/data/adb/ap/bin/busybox" ] && "/data/adb/ap/bin/busybox" setsid true >/dev/null 2>&1; then
  SETSID_CMD="/data/adb/ap/bin/busybox setsid"
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
' sh \"$PID_FILE\" \"$DAEMON_PATH\" \"$STOP_FLAG\" > /dev/null 2>&1"

# 启动看门狗，优先使用 setsid 脱离父进程组
if [ -n "$SETSID_CMD" ]; then
  $SETSID_CMD sh -c "$WATCHDOG_CMD"
else
  # fallback: 使用 nohup（兼容性更好，但可能无法完全脱离进程组）
  nohup sh -c "$WATCHDOG_CMD" > /dev/null 2>&1 &
fi

# 4. 打印执行结果
log "daemon restarted."
exit 0