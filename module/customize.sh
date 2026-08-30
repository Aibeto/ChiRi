#!/system/bin/sh
#
# ########################################################################################
#   yumi 模块安装脚本
#   作者: yuki
# ########################################################################################

# --- 模块路径和工具 ---
# $MODPATH 是 Magisk 传入的模块安装路径

# --- 自动检测 BusyBox (暂未使用，预留) ---
if [ -x "/data/adb/magisk/busybox" ]; then
  BUSYBOX="/data/adb/magisk/busybox"
elif [ -x "/data/adb/ksu/bin/busybox" ]; then
  BUSYBOX="/data/adb/ksu/bin/busybox"
elif [ -x "/data/adb/ap/bin/busybox" ]; then
  BUSYBOX="/data/adb/ap/bin/busybox"
fi

# --- 语言定义 ---
CURRENT_LOCALE=$(/system/bin/getprop persist.sys.locale)
if [ -z "$CURRENT_LOCALE" ]; then
    CURRENT_LOCALE=$(/system/bin/getprop ro.product.locale)
fi

LANG_CODE="en"
MSG_WELCOME="Welcome to ChiRi Scheduler! (Based on Yumi Scheduler)"

if echo "$CURRENT_LOCALE" | $BUSYBOX grep -qi "zh"; then
  LANG_CODE="zh"
  MSG_WELCOME="欢迎使用 ChiRi 调度！（Based on Yumi Scheduler）"
fi

# --- 仅输出欢迎信息 ---
ui_print " "
ui_print "$MSG_WELCOME"
ui_print " "

# --- 结束 ---
# 保留默认配置，不执行文件操作