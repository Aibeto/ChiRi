#!/system/bin/sh
#
# yumi 模块 action 脚本
#

# 1. 等待系统启动完成
# until [ "$(getprop sys.boot_completed)" = "1" ]; do
#   sleep 1
# done

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

# 禁用小米的 Joyose 服务（已注释）
# PACKAGE_NAME="com.xiaomi.joyose"
# if pm list packages -e | grep -q "$PACKAGE_NAME"; then
#   pm disable-user "$PACKAGE_NAME" >/dev/null 2>&1
#   pm clear "$PACKAGE_NAME" >/dev/null 2>&1
#   echo "$(date): Joyose service disabled and data cleared." >> "$LOG_FILE"
# fi

# 3. 清理旧进程
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

# 6. 启动 yumi 守护进程（崩溃自动重启）
# 守护进程退出后 3 秒自动拉起；二进制被删（卸载）时自动退出，不残留。
# 调试：换成下方"方式 B"可输出报错日志。
nohup sh -c '
  while :; do
    "$1" || exit 0
    sleep 3
  done
' sh "$DAEMON_PATH" > /dev/null 2>&1 &

# 方式 B: 调试模式（启动失败时用这个排查，输出到 logs/boot_error.log）
# nohup sh -c 'while :; do "$1" || exit 0; sleep 3; done' sh "$DAEMON_PATH" > "$LOG_DIR/boot_error.log" 2>&1 &

# 记录启动日志（action 阶段无 ui_print，改写日志）
echo "$(date): daemon started." >> "$LOG_FILE"


ui_print "ChiRi Scheduler started."
