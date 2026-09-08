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
  MSG_WELCOME="ChiRi INSTALLATION SCRIPT"
  MSG_SELECT_MODE="TELL ME YOUR CHOICE:"
  MSG_VOLUME_UP="[ Volume + ] FULL INSTALL (NEED REBOOT)"
  MSG_VOLUME_DOWN="[ Volume - ] HOT UPDATE (BETA)"
  MSG_SELECTED_UP="SELECT: FULL INSTALL (NEED REBOOT)"
  MSG_SELECTED_DOWN="SELECT: HOT UPDATE (BETA)"
  MSG_HOT_UPDATE_START="MISSION START"
  MSG_STOPPING_DAEMON="STOPPING DAEMON PROCESS"
  MSG_STOPPING_MAIN="STOPPING MAIN PROCESS"
  MSG_COPYING_FILES="COPYING MODULE FILES"
  MSG_RESTARTING_SERVICE="RESTARTING SERVICE"
  MSG_HOT_UPDATE_DONE="MISSION ACCOMPLISHED"
  MSG_FULL_INSTALL="FULL INSTALLATION PROCESS"
  MSG_HOT_UPDATE_UNAVAILABLE="HOT UPDATE UNAVAILABLE, FALLING TO FULL INSTALLATION..."
  MSG_RESTARTING_SCHEDULER="RESTARTING SCHEDULER"
  MSG_VERIFY_SERVICE="VERIFYING  MSG_SERVICE_FAIL  MSG_SERVICE_FAIL="DAEMON DID NOT START! PLEASE REBOOT THE DEVICE TO COMPLETE THE UPDATE. (DAEMON IS REQUIRED TO START THE SCHEDULER)"
  MSG_HOT_UPDATE_HINT=""
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
        # **必须 export**：安装器（KSU/Magisk）环境可能已把 MODDIR 导出为
        # staging 目录（modules_update），service.sh 的 [ -z "$MODDIR" ] 会
        # 直接继承错位路径——看门狗从 staging 拉起 daemon、日志/pidfile/锁
        # 全部写入 staging，KSU 清理 staging 后二进制消失、调度静默死亡。
        # export 后子进程强制拿到 live 目录（仅局部赋值子进程不可见）。
        export MODDIR
        
        # 1. 停止守护进程和主进程
        # **必须先杀旧看门狗**：看门狗每 3s 把 daemon 拉回（此时还是旧二进制），
        # 复活若落在下方文件复制窗口，cp 覆盖运行中的 yumi 会 ETXTBSY 失败且
        # 被 2>/dev/null 吞掉——模块目录残留旧版本，热更新后手动重启调度仍是
        # 旧版，须重启一次（KSU 应用 modules_update）才被覆盖固化
        ui_print "$MSG_STOPPING_DAEMON"
        PID_FILE="$MODDIR/logs/watchdog.pid"
        if [ -f "$PID_FILE" ]; then
            pid=$(cat "$PID_FILE" 2>/dev/null)
            case "$pid" in
                ''|0|*[!0-9]*) ;;
                *) kill "$pid" 2>/dev/null ;;
            esac
            rm -f "$PID_FILE"
        fi
        if [ -x "/system/bin/killall" ]; then
            /system/bin/killall -9 yumi 2>/dev/null
        elif [ -n "$BUSYBOX" ]; then
            $BUSYBOX killall -9 yumi 2>/dev/null
        fi
        sleep 1

        ui_print "$MSG_STOPPING_MAIN"
        if [ -x "/system/bin/killall" ]; then
            /system/bin/killall -9 yumi 2>/dev/null
        elif [ -n "$BUSYBOX" ]; then
            $BUSYBOX killall -9 yumi 2>/dev/null
        fi
        sleep 1

        # 轮询确认 daemon 已真正退出（最多 ~5s）：卡在不可中断 IO（D 状态）的
        # 进程对 SIGKILL 也要等 IO 返回，未死透时 cp 覆盖二进制会 ETXTBSY
        YUMI_ALIVE=true
        n=0
        while [ $n -lt 5 ]; do
            if [ -x "/system/bin/pidof" ]; then
                DAEMON_PID=$(/system/bin/pidof yumi 2>/dev/null)
            elif [ -n "$BUSYBOX" ]; then
                DAEMON_PID=$($BUSYBOX pgrep -x yumi 2>/dev/null)
            else
                DAEMON_PID=""
            fi
            if [ -z "$DAEMON_PID" ]; then
                YUMI_ALIVE=false
                break
            fi
            sleep 1
            n=$((n + 1))
        done

        # 2. 复制模块文件到目标目录
        ui_print "$MSG_COPYING_FILES"
        # 备份用户配置文件（注意：rules.yaml 在模块根，不在 config/ 子目录）
        if [ -f "$MODDIR/config/config.yaml" ]; then
            cp "$MODDIR/config/config.yaml" "$MODDIR/config/config.yaml.bak"
        fi
        if [ -f "$MODDIR/rules.yaml" ]; then
            cp "$MODDIR/rules.yaml" "$MODDIR/rules.yaml.bak"
        fi

        # 复制新文件
        cp -r "$MODPATH"/* "$MODDIR/" 2>/dev/null

        # 二进制单独复制并校验：daemon 未完全退出（YUMI_ALIVE）或内核仍持有
        # 文本页时 cp 会 ETXTBSY 失败——静默失败会残留旧版本，热更新后调度
        # 跑旧版。失败时恢复配置备份并重启旧版服务保持调度连续，明确告知
        # 用户当前为旧版。
        # 注意：热更新路径**必须以 exit 1 结束**（成功与失败皆然）——exit 0
        # 会让 KSU 把模块归为「待更新」，Action/WebUI 被禁用直到重启。
        UPDATE_OK=true
        if [ "$YUMI_ALIVE" = "true" ] || ! cp "$MODPATH/core/bin/yumi" "$MODDIR/core/bin/yumi" 2>/dev/null; then
            UPDATE_OK=false
        fi
        # 关键文件存在性抽查：上面的 cp -r 把错误吞进 /dev/null，ENOSPC/IO 错误
        # 会留下半新半旧模块且用户无感知（成功提示只看 daemon 是否存活）。
        # service.sh/module.prop/allowHotUpdate/rules.yaml 任一缺失即判失败，
        # 走与二进制相同的回滚分支。
        for key_file in service.sh module.prop allowHotUpdate rules.yaml; do
            [ -f "$MODDIR/$key_file" ] || UPDATE_OK=false
        done
        if [ "$UPDATE_OK" = "false" ]; then
            if [ -f "$MODDIR/config/config.yaml.bak" ]; then
                mv "$MODDIR/config/config.yaml.bak" "$MODDIR/config/config.yaml"
            fi
            if [ -f "$MODDIR/rules.yaml.bak" ]; then
                mv "$MODDIR/rules.yaml.bak" "$MODDIR/rules.yaml"
            fi
            chmod 755 "$MODDIR/core/bin/yumi" 2>/dev/null
            ui_print "ERROR: hot update copy failed! Old version kept & restarting."
            ui_print "错误：热更新文件复制失败！正在重启旧版本以保持调度。"
            ui_print "当前仍为旧版本——请重新刷入模块或重试热更新完成升级。"
            ui_print "Old version is running — re-flash or retry the hot update to upgrade."
        fi

        # 恢复用户配置文件
        if [ -f "$MODDIR/config/config.yaml.bak" ]; then
            mv "$MODDIR/config/config.yaml.bak" "$MODDIR/config/config.yaml"
        fi
        if [ -f "$MODDIR/rules.yaml.bak" ]; then
            mv "$MODDIR/rules.yaml.bak" "$MODDIR/rules.yaml"
        fi

        # 设置权限（注意二进制在 core/bin/yumi，模块根下并无 yumi 文件）
        chmod 755 "$MODDIR/service.sh" 2>/dev/null
        chmod 755 "$MODDIR/action.sh" 2>/dev/null
        chmod 755 "$MODDIR/core/bin/yumi" 2>/dev/null
        
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