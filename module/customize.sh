#!/system/bin/sh
#
# ChiRi Scheduler Installation Script


# --- 模块路径和工具 ---
# $MODPATH 是 Magisk 传入的模块安装路径

# --- 自动检测 BusyBox ---
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
MSG_SELECT_MODE="Please select installation mode:"
MSG_VOLUME_UP="[Volume UP] Full installation (Recommended)"
MSG_VOLUME_DOWN="[Volume DOWN] Hot update (Keep current config)"
MSG_SELECTED_UP="Selected: Full installation"
MSG_SELECTED_DOWN="Selected: Hot update"
MSG_HOT_UPDATE_START="Starting hot update process..."
MSG_STOPPING_DAEMON="Stopping daemon process..."
MSG_STOPPING_MAIN="Stopping main process..."
MSG_COPYING_FILES="Copying module files..."
MSG_RESTARTING_SERVICE="Restarting service..."
MSG_HOT_UPDATE_DONE="Hot update completed successfully!"
MSG_FULL_INSTALL="Proceeding with full installation..."
MSG_HOT_UPDATE_UNAVAILABLE="Hot update unavailable, falling back to full installation..."
MSG_RESTARTING_SCHEDULER="Restarting scheduler..."

if echo "$CURRENT_LOCALE" | $BUSYBOX grep -qi "zh"; then
  LANG_CODE="zh"
  MSG_WELCOME="欢迎使用 ChiRi 调度！（Based on Yumi Scheduler）"
  MSG_SELECT_MODE="请选择安装模式："
  MSG_VOLUME_UP="[音量上键] 完整安装（推荐）"
  MSG_VOLUME_DOWN="[音量下键] 热更新（保留当前配置）"
  MSG_SELECTED_UP="已选择：完整安装"
  MSG_SELECTED_DOWN="已选择：热更新"
  MSG_HOT_UPDATE_START="开始热更新流程..."
  MSG_STOPPING_DAEMON="正在停止守护进程..."
  MSG_STOPPING_MAIN="正在停止主进程..."
  MSG_COPYING_FILES="正在复制模块文件..."
  MSG_RESTARTING_SERVICE="正在重启服务..."
  MSG_HOT_UPDATE_DONE="热更新完成！"
  MSG_FULL_INSTALL="继续完整安装流程..."
  MSG_HOT_UPDATE_UNAVAILABLE="热更新不可用，回退到完整安装..."
  MSG_RESTARTING_SCHEDULER="正在重启调度..."
fi

# --- 欢迎信息 ---
ui_print " "
ui_print "$MSG_WELCOME"
ui_print " "

# --- 检查热更新标记文件 ---
# 检查zip内的allowHotUpdate文件
ZIP_HOT_UPDATE_FLAG="$MODPATH/allowHotUpdate"
# 检查已安装模块的allowHotUpdate文件
INSTALLED_HOT_UPDATE_FLAG="/data/adb/modules/chiri/allowHotUpdate"

# 热更新可用条件：两个文件都存在且内容为1
HOT_UPDATE_AVAILABLE=false
if [ -f "$ZIP_HOT_UPDATE_FLAG" ] && [ "$(cat "$ZIP_HOT_UPDATE_FLAG")" = "1" ] && \
   [ -f "$INSTALLED_HOT_UPDATE_FLAG" ] && [ "$(cat "$INSTALLED_HOT_UPDATE_FLAG")" = "1" ]; then
    HOT_UPDATE_AVAILABLE=true
fi

if [ "$HOT_UPDATE_AVAILABLE" = "true" ]; then
    # 热更新模式可用，显示选择菜单
    ui_print "$MSG_SELECT_MODE"
    ui_print "$MSG_VOLUME_UP"
    ui_print "$MSG_VOLUME_DOWN"
    ui_print " "
    
    # 音量键检测函数（兼容 Magisk/KernelSU 环境）
    # 返回 0 表示音量上键，返回 1 表示音量下键
    detect_volume_key() {
        # 尝试使用 Magisk 的 chooseport（如果可用）
        if type chooseport >/dev/null 2>&1; then
            chooseport && return 0 || return 1
        fi
        
        # 备用方案：读取音量键设备节点
        for input_dev in /dev/input/event*; do
            local name=$(cat /sys/class/input/$(basename $input_dev)/device/name 2>/dev/null)
            case "$name" in
                *volume*|*Volume*|*gpio-keys*|*qpnp_pon*)
                    # 检测音量键事件（1秒超时）
                    local event=$(timeout 1 getevent -l $input_dev 2>/dev/null | head -1)
                    if echo "$event" | grep -q "KEY_VOLUMEUP"; then
                        return 0
                    elif echo "$event" | grep -q "KEY_VOLUMEDOWN"; then
                        return 1
                    fi
                    ;;
            esac
        done
        
        # 如果没有找到音量键，使用默认选择（完整安装）
        ui_print "未检测到音量键，使用完整安装流程..."
        ui_print "No volume key detected, proceeding with full installation..."
        return 0
    }
    
    # 等待用户按键选择
    detect_volume_key
    local choice_result=$?
    
    if [ $choice_result -eq 0 ]; then
        # 音量上键 - 完整安装
        ui_print "$MSG_SELECTED_UP"
        ui_print "$MSG_FULL_INSTALL"
        # 继续执行完整安装流程（原逻辑）
    else
        # 音量下键 - 热更新
        ui_print "$MSG_SELECTED_DOWN"
        ui_print "$MSG_HOT_UPDATE_START"
        
        # 热更新流程
        MODDIR="/data/adb/modules/chiri"
        
        # 1. 停止守护进程和主进程
        ui_print "$MSG_STOPPING_DAEMON"
        if [ -x "/system/bin/killall" ]; then
            /system/bin/killall yumi 2>/dev/null
        elif [ -n "$BUSYBOX" ]; then
            $BUSYBOX killall yumi 2>/dev/null
        fi
        sleep 1
        
        ui_print "$MSG_STOPPING_MAIN"
        if [ -x "/system/bin/killall" ]; then
            /system/bin/killall yumi 2>/dev/null
        elif [ -n "$BUSYBOX" ]; then
            $BUSYBOX killall yumi 2>/dev/null
        fi
        sleep 1
        
        # 2. 复制模块文件到目标目录
        ui_print "$MSG_COPYING_FILES"
        # 备份用户配置文件
        if [ -f "$MODDIR/config/config.yaml" ]; then
            cp "$MODDIR/config/config.yaml" "$MODDIR/config/config.yaml.bak"
        fi
        if [ -f "$MODDIR/config/rules.yaml" ]; then
            cp "$MODDIR/config/rules.yaml" "$MODDIR/config/rules.yaml.bak"
        fi
        
        # 复制新文件
        cp -r "$MODPATH"/* "$MODDIR/" 2>/dev/null
        
        # 恢复用户配置文件
        if [ -f "$MODDIR/config/config.yaml.bak" ]; then
            mv "$MODDIR/config/config.yaml.bak" "$MODDIR/config/config.yaml"
        fi
        if [ -f "$MODDIR/config/rules.yaml.bak" ]; then
            mv "$MODDIR/config/rules.yaml.bak" "$MODDIR/config/rules.yaml"
        fi
        
        # 设置权限
        chmod 755 "$MODDIR/service.sh" 2>/dev/null
        chmod 755 "$MODDIR/action.sh" 2>/dev/null
        chmod 755 "$MODDIR/yumi" 2>/dev/null
        
        # 3. 重启调度服务
        # 使用setsid启动service.sh，确保进程脱离安装环境存活
        ui_print "$MSG_RESTARTING_SCHEDULER"
        if [ -f "$MODDIR/service.sh" ]; then
            # 检测 setsid 可用性，优先使用 BusyBox 的 setsid
            SETSID_CMD=""
            if command -v setsid >/dev/null 2>&1; then
                SETSID_CMD="setsid"
            elif [ -n "$BUSYBOX" ] && "$BUSYBOX" setsid true >/dev/null 2>&1; then
                SETSID_CMD="$BUSYBOX setsid"
            fi
            
            # 启动 service.sh，优先使用 setsid 脱离父进程组
            if [ -n "$SETSID_CMD" ]; then
                $SETSID_CMD sh "$MODDIR/service.sh" </dev/null >/dev/null 2>&1 &
            else
                # fallback: 使用 nohup（兼容性更好，但可能无法完全脱离进程组）
                nohup sh "$MODDIR/service.sh" </dev/null >/dev/null 2>&1 &
            fi
        fi
        sleep 2
        
        ui_print " "
        ui_print "$MSG_HOT_UPDATE_DONE"
        
        # 热更新完成，跳过后续安装步骤
        return 0
    fi
else
    # 热更新不可用，显示提示信息
    ui_print "$MSG_HOT_UPDATE_UNAVAILABLE"
    ui_print " "
fi

# --- 完整安装流程（原逻辑） ---
# 保留默认配置，不执行文件操作
# 完整安装将由 Magisk 自动处理模块文件复制