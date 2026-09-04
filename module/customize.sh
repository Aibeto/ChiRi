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
MSG_VOLUME_UP="[Volume UP] Full installation"
MSG_VOLUME_DOWN="[Volume DOWN] Hot update"
MSG_SELECTED_UP="Selected: Full installation (Requires device reboot)"
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
MSG_VERIFY_SERVICE="Verifying daemon service..."
MSG_SERVICE_FAIL="Daemon did not start! Please reboot the device to complete the update."
MSG_HOT_UPDATE_HINT="(The installer will now report a failure on purpose: this prevents the manager from flagging the module as updated/reboot-required. WebUI and Action stay available.)"

if echo "$CURRENT_LOCALE" | $BUSYBOX grep -qi "zh"; then
  LANG_CODE="zh"
  MSG_WELCOME="欢迎使用 ChiRi 调度！（Based on Yumi Scheduler）"
  MSG_SELECT_MODE="请选择安装模式："
  MSG_VOLUME_UP="[音量上键] 完整安装（需要重启设备）"
  MSG_VOLUME_DOWN="[音量下键] 热更新"
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
  MSG_VERIFY_SERVICE="正在确认调度服务启动..."
  MSG_SERVICE_FAIL="守护进程未能启动！请重启设备以完成更新。"
  MSG_HOT_UPDATE_HINT="（安装器随后显示“安装失败”为预期行为：防止管理器把热更新识别为模块更新而提示重启、隐藏 WebUI 与 Action。）"
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
    # 返回 2 表示错误（多次按下事件）
    # 说明：单次物理按键可能被多个输入设备重复上报，检测时需去重，
    #       同一轮轮询内同键的多次 DOWN 视为同一次按下，避免误判。
    detect_volume_key() {
        ui_print "等待音量键按下..."
        ui_print "Waiting for volume key press..."
        
        # 临时文件写入模块暂存目录（安装环境 /tmp 可能不可写）
        local tmp_file="$MODPATH/.getevent_output"
        rm -f "$tmp_file"
        
        # 后台监听所有输入设备的音量键事件
        getevent -l > "$tmp_file" 2>/dev/null &
        local getevent_pid=$!
        
        local round=0
        local first_key=""
        local first_round=-1
        local last_up=0
        local last_down=0
        
        # 每 0.1 秒轮询一次：前 10 秒等待第一次按下，之后 1 秒确认窗口
        while [ $round -lt 120 ]; do
            local up=$(grep -c "KEY_VOLUMEUP.*DOWN" "$tmp_file" 2>/dev/null || echo 0)
            local down=$(grep -c "KEY_VOLUMEDOWN.*DOWN" "$tmp_file" 2>/dev/null || echo 0)
            
            if [ -z "$first_key" ]; then
                # 记录第一个按下事件（同轮内多设备重复上报视为同一次按下）
                if [ "$up" -gt 0 ]; then
                    first_key="KEY_VOLUMEUP"
                    first_round=$round
                    last_up=$up
                elif [ "$down" -gt 0 ]; then
                    first_key="KEY_VOLUMEDOWN"
                    first_round=$round
                    last_down=$down
                fi
            else
                # 确认窗口：第一个按键后再监听 1 秒，确认无第二次按下
                if [ $((round - first_round)) -ge 10 ]; then
                    kill $getevent_pid 2>/dev/null
                    rm -f "$tmp_file"
                    if [ "$first_key" = "KEY_VOLUMEUP" ]; then
                        return 0
                    else
                        return 1
                    fi
                fi
                # 第二次按下：出现另一按键，或同键计数在新轮次增加
                if [ "$first_key" = "KEY_VOLUMEUP" ]; then
                    if [ "$down" -gt 0 ] || [ "$up" -gt "$last_up" ]; then
                        kill $getevent_pid 2>/dev/null
                        rm -f "$tmp_file"
                        ui_print "错误：检测到多个音量键按下事件！"
                        ui_print "Error: Multiple volume key press events detected!"
                        return 2
                    fi
                else
                    if [ "$up" -gt 0 ] || [ "$down" -gt "$last_down" ]; then
                        kill $getevent_pid 2>/dev/null
                        rm -f "$tmp_file"
                        ui_print "错误：检测到多个音量键按下事件！"
                        ui_print "Error: Multiple volume key press events detected!"
                        return 2
                    fi
                fi
            fi
            sleep 0.1
            round=$((round + 1))
        done
        
        # 超时，清理并使用默认选择
        kill $getevent_pid 2>/dev/null
        rm -f "$tmp_file"
        if [ -n "$first_key" ]; then
            if [ "$first_key" = "KEY_VOLUMEUP" ]; then
                return 0
            else
                return 1
            fi
        fi
        ui_print "未检测到音量键，使用完整安装流程..."
        ui_print "No volume key detected, proceeding with full installation..."
        return 0
    }
    
    # 等待用户按键选择（脚本顶层非函数环境，不能用 local）
    detect_volume_key
    choice_result=$?
    
    # 检查是否检测到多个按键事件（错误）
    if [ $choice_result -eq 2 ]; then
        ui_print "安装已取消，请重新运行安装脚本。"
        ui_print "Installation cancelled. Please run the installation script again."
        exit 1
    fi
    
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

        # 4. 确认服务启动状态：watchdog nohup 拉起 daemon 有延迟，轮询最多 ~6s
        ui_print "$MSG_VERIFY_SERVICE"
        SERVICE_OK=false
        CHECK_ROUND=0
        while [ $CHECK_ROUND -lt 3 ]; do
            sleep 2
            if [ -x "/system/bin/pidof" ]; then
                DAEMON_PID=$(/system/bin/pidof yumi 2>/dev/null)
            elif [ -n "$BUSYBOX" ]; then
                DAEMON_PID=$($BUSYBOX pgrep -x yumi 2>/dev/null)
            else
                DAEMON_PID=""
            fi
            if [ -n "$DAEMON_PID" ]; then
                SERVICE_OK=true
                break
            fi
            CHECK_ROUND=$((CHECK_ROUND + 1))
        done

        ui_print " "
        if [ "$SERVICE_OK" = "true" ]; then
            ui_print "$MSG_HOT_UPDATE_DONE"
        else
            ui_print "$MSG_SERVICE_FAIL"
        fi
        ui_print "$MSG_HOT_UPDATE_HINT"

        # 5. 按报错退出：安装器视本次安装为失败，中止后续“完整安装”——
        #    不覆盖上面已热替换的模块目录、不写 update 标记，管理器不会把
        #    热更新识别为“模块更新”（不提示重启，WebUI 与 Action 保持可用）。
        exit 1
    fi
else
    # 热更新不可用，显示提示信息
    ui_print "$MSG_HOT_UPDATE_UNAVAILABLE"
    ui_print " "
fi

# --- 完整安装流程（原逻辑） ---
# 保留默认配置，不执行文件操作
# 完整安装将由 Magisk 自动处理模块文件复制