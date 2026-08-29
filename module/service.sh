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

禁用 OPPO/OnePlus/Realme 的 Oiface（已注释）
if [ "$(getprop persist.sys.oiface.enable)" = "1" ]; then
  setprop persist.sys.oiface.enable 0
  echo "$(date): Oiface disabled." >> "$LOG_FILE"
fi

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
# 循环包装：守护进程异常退出/被杀后 3 秒自动拉起；模块被卸载（二进制文件被删除）时
# 执行失败即退出，避免卸载后残留进程继续锁频篡改 CPU。
# 调试排障：把下面两行换成"方式 B"的单次启动即可看到报错输出。
nohup sh -c '
  while :; do
    "$1" || exit 0
    sleep 3
  done
' sh "$DAEMON_PATH" > /dev/null 2>&1 &

# 方式 B: 调试模式 (如果启动不起来，用这个看报错，输出到 logs/boot_error.log)
# nohup sh -c 'while :; do "$1" || exit 0; sleep 3; done' sh "$DAEMON_PATH" > "$LOG_DIR/boot_error.log" 2>&1 &