#!/system/bin/sh
# customize.sh: [busybox] [i18n] [welcome] [hot-update-check] [volume-key] [battery-detect] [config-keep] [mode-select] [hot-update-flow] [full-install]
#
# ChiRi Scheduler Installation Script


# [busybox] 
# 模块路径和工具：$MODPATH 是 Magisk 传入的模块安装路径

# --- 自动检测 BusyBox ---
if [ -x "/data/adb/magisk/busybox" ]; then
  BUSYBOX="/data/adb/magisk/busybox"
elif [ -x "/data/adb/ksu/bin/busybox" ]; then
  BUSYBOX="/data/adb/ksu/bin/busybox"
elif [ -x "/data/adb/ap/bin/busybox" ]; then
  BUSYBOX="/data/adb/ap/bin/busybox"
fi

# [i18n] 
# 语言定义
CURRENT_LOCALE=$(/system/bin/getprop persist.sys.locale)
if [ -z "$CURRENT_LOCALE" ]; then
    CURRENT_LOCALE=$(/system/bin/getprop ro.product.locale)
fi

LANG_CODE="en"
MSG_WELCOME="ChiRi Scheduler"
MSG_SELECT_MODE="Please select installation mode:"
MSG_VOLUME_UP="[Volume UP] Normal installation (requires reboot) [Recommended]"
MSG_VOLUME_DOWN="[Volume DOWN] Hot update (Experimental)"
MSG_SELECTED_UP="Selected: Normal installation (Requires device reboot)"
MSG_SELECTED_DOWN="Selected: Hot update (Experimental)"
MSG_HOT_UPDATE_START="Mission start"
MSG_STOPPING_DAEMON="Stopping daemon process..."
MSG_STOPPING_MAIN="Stopping main process..."
MSG_COPYING_FILES="Copying module files..."
MSG_RESTARTING_SERVICE="Restarting service..."
MSG_HOT_UPDATE_DONE="Mission accomplished"
MSG_FULL_INSTALL="Proceeding with normal installation"
MSG_HOT_UPDATE_UNAVAILABLE="Hot update unavailable, falling back to normal installation..."
MSG_RESTARTING_SCHEDULER="Restarting scheduler..."
MSG_VERIFY_SERVICE="Verifying daemon service..."
MSG_SERVICE_FAIL="Restart process failed, please try again or choose normal installation"
MSG_HOT_UPDATE_HINT="If you encounter any errors at the end, please ignore them. Run Action manually, or the scheduler will stop once the manager closes."
MSG_INSTALL_CANCELLED="Installation cancelled"
MSG_HOT_UPDATE_ABORT="Hot update done. Please run Action manually, or the scheduler will stop once the manager closes."
MSG_KEEP_CONFIG_ASK="Keep the existing module configuration?"
MSG_KEEP_CONFIG_UP="[Volume UP] Keep existing configuration (not recommended)"
MSG_KEEP_CONFIG_DOWN="[Volume DOWN] Overwrite the existing configuration with the defaults"
MSG_KEEP_CONFIG_YES="Existing configuration kept"
MSG_KEEP_CONFIG_NO="Replaced with the default configuration"
MSG_OPLUS_ON="OPlus private node detected; enabled private-node readings in the new config"
MSG_OPLUS_DUAL_ON="Second cell detected (index 11); enabled dual-cell voltage"
MSG_OPLUS_NO_META="Could not locate this device's meta.yaml, so the battery switches and divisors were not auto-applied (set them in the WebUI battery page)"
MSG_OPLUS_NO_SED="sed is unavailable, so the battery switches and divisors were not auto-applied (set them in the WebUI battery page)"
MSG_OPLUS_CHECK="Checking for the OPlus private node (bcc_parms)..."
MSG_OPLUS_SKIP="No OPlus private node on this device; falling back to the standard node"
MSG_VOLT_CHECK="Reading the standard voltage node to calibrate the unit divisors..."
MSG_VOLT_APPLY="Calibration divisors written (same value for voltage and current)"
MSG_VOLT_FAIL="Standard voltage node unreadable or out of the 3-4.5 V range; divisors left as they are"

if echo "$CURRENT_LOCALE" | $BUSYBOX grep -qi "zh"; then
  LANG_CODE="zh"
  MSG_WELCOME="ChiRi 千漓调度"
  MSG_SELECT_MODE="模式选择："
  MSG_VOLUME_UP="[ 音量 + ] 普通安装（需要重启）[推荐]"
  MSG_VOLUME_DOWN="[ 音量 - ] 热更新（实验性）"
  MSG_SELECTED_UP="已选择：普通安装（需要重启）"
  MSG_SELECTED_DOWN="已选择：热更新（实验性）"
  MSG_HOT_UPDATE_START="任务开始"
  MSG_STOPPING_DAEMON="正在停止守护进程..."
  MSG_STOPPING_MAIN="正在停止主进程..."
  MSG_COPYING_FILES="正在复制模块文件..."
  MSG_RESTARTING_SERVICE="正在重启服务..."
  MSG_HOT_UPDATE_DONE="完成"
  MSG_FULL_INSTALL="开始安装"
  MSG_HOT_UPDATE_UNAVAILABLE="热更新不可用，回退到完整安装..."
  MSG_RESTARTING_SCHEDULER="正在重启调度器..."
  MSG_VERIFY_SERVICE="正在确认守护进程状态..."
  MSG_SERVICE_FAIL="重启进程失败，请重试或使用普通安装"
  MSG_HOT_UPDATE_HINT="如有报错请忽略。调度未启动，需要手动执行一次Action"
  MSG_INSTALL_CANCELLED="安装已取消"
  MSG_HOT_UPDATE_ABORT="热更新已完成，如有报错请忽略。调度未启动，需要手动执行一次Action"
  MSG_KEEP_CONFIG_ASK="是否保留模块内已有的配置？"
  MSG_KEEP_CONFIG_UP="[ 音量 + ] 保留现有配置（不建议）"
  MSG_KEEP_CONFIG_DOWN="[ 音量 - ] 使用默认配置覆盖现有配置"
  MSG_KEEP_CONFIG_YES="已保留"
  MSG_KEEP_CONFIG_NO="已更换为默认配置"
  MSG_OPLUS_ON="检测到 OPlus 私有节点，已为新配置启用私有节点读取"
  MSG_OPLUS_DUAL_ON="检测到第二个电芯，已启用双电芯电压"
  MSG_OPLUS_NO_META="未定位到当前机型的 meta.yaml，电池读数开关与校准倍数未自动写入（可在 WebUI 电池读数页设置）"
  MSG_OPLUS_NO_SED="sed 不可用，电池读数开关与校准倍数未自动写入（可在 WebUI 电池读数页设置）"
  MSG_OPLUS_CHECK="正在检查 OPlus 私有节点（bcc_parms）..."
  MSG_OPLUS_SKIP="本机没有 OPlus 私有节点，回退标准节点读数"
  MSG_VOLT_CHECK="正在读取标准节点电压，推算校准倍数..."
  MSG_VOLT_APPLY="已写入校准倍数（电压与电流同值）"
  MSG_VOLT_FAIL="标准节点电压读数不可用或不在 3~4.5V 量级，校准倍数保持原样，安装完成后请打开webui手动校准"

fi

# [welcome] 
# 欢迎信息
ui_print " "
ui_print "$MSG_WELCOME"
ui_print " "

# [hot-update-check] 
# 检查热更新标记文件
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

# [volume-key]
# 音量键检测（兼容 Magisk/KernelSU 环境）：安装模式选择与「是否保留配置」两处共用。
# 返回 0 = 音量上键，返回 1 = 音量下键，返回 2 = 错误（多次按下事件）。
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
    ui_print "未检测到音量键，使用默认选项..."
    ui_print "No volume key detected, using the default choice..."
    return 0
}

# [battery-detect]
# 按机器把电池读数那几个开关与校准倍数写进**当前机型**那一份 meta.yaml：
#   1) 先定位文件——优先按模块里的 active_config.chr（daemon 写的生效配置相对路径，
#      如 8550/meta.yaml）；退一步用 ro.soc.model 的数字部分（SM8550 → 8550）拼机型
#      目录；都拿不到就打印说明跳过，不去猜别的机型文件；
#   2) OPlus 私有节点 bcc_parms 读得到内容 → `oplus_chg: false` 改 true，并按第 12 个
#      字段（0 基下标 11）判双电芯 → `oplus_dual_cell: false` 改 true；
#   3) 两条路都走同一个入口 `apply_divisor_from_raw` 写校准倍数：按节点**电压原始值的
#      位数**推算 `voltage_divisor` / `current_divisor`（两者同值）——标准节点取
#      voltage_now、私有节点取 bcc_parms 下标 6（电芯0电压）。模板缺省 1000000 是标准
#      ABI 的 µV/µA 口径；私有节点通常报 mV/mA → 推算出 1000，不硬编码。
# 改的是 $MODPATH（= modules_update 暂存）里那一份：完整安装由安装器落地、热更新由
# 下面的 cp -r 覆盖到 live 目录——两条路拿到的都是这份结果（所以只改一次）。
# 只在用户选择「不保留配置」时调用（见 [config-keep]）——保留时那份 meta.yaml 是用户
# 自己的文件，安装器不该碰；这些项随时可以在 WebUI 电池读数页改。
# 定义必须早于调用点（普通安装与热更新两条路都会用到）。
OPLUS_BCC="/sys/class/oplus_chg/battery/bcc_parms"
STD_VOLT="/sys/class/power_supply/battery/voltage_now"

# 定位当前机型的 meta.yaml，结果放进全局 META_FILE（定位不到就留空）
META_FILE=""
SED_CMD=""
locate_meta_file() {
    META_FILE=""
    local rel=""
    local active="/data/adb/modules/chiri/active_config.chr"
    if [ -f "$active" ]; then
        rel=$(cat "$active" 2>/dev/null | tr -d ' \t\r\n')
    fi
    if [ -z "$rel" ]; then
        local soc=$(getprop ro.soc.model 2>/dev/null | tr -cd '0-9')
        if [ -n "$soc" ] && [ -f "$MODPATH/config/$soc/meta.yaml" ]; then
            rel="$soc/meta.yaml"
        fi
    fi
    if [ -n "$rel" ] && [ -f "$MODPATH/config/$rel" ]; then
        META_FILE="$MODPATH/config/$rel"
    fi
}

# 校准倍数统一入口：按**电压原始值的位数**推算除数并写入两个 divisor。
# 原始值有 n 位 → 除数 = 1 后跟 n-1 个 0（4382 → 1000 = 4.382V；4382000 → 1000000
# = 4.382V），电流套用同一个数——同一节点的电压/电流单位一致。首位必须是 3 或 4
# （电池 3~4.5V），否则不猜：量级填错会让功率整条曲线失真，宁可留模板缺省值让用户
# 在 WebUI 里改。两条路共用（标准节点取 voltage_now、私有节点取 bcc_parms 下标 6），
# 口径自然一致，也不必对私有节点的 mV/mA 硬编码。返回 1 = 没写。
apply_divisor_from_raw() {
    local meta="$1"
    local raw
    raw=$(printf '%s' "$2" | tr -d ' \t\r\n-')
    case "$raw" in
        ''|*[!0-9]*) return 1 ;;
    esac
    case "$raw" in
        3*|4*) ;;
        *) return 1 ;;
    esac

    local len=${#raw}
    local div="1"
    local i=1
    while [ "$i" -lt "$len" ]; do
        div="${div}0"
        i=$((i + 1))
    done
    $SED_CMD -i "s/^voltage_divisor: .*/voltage_divisor: $div/" "$meta"
    $SED_CMD -i "s/^current_divisor: .*/current_divisor: $div/" "$meta"
    ui_print "$MSG_VOLT_APPLY"
    ui_print "raw=$raw -> divisor=$div"
    return 0
}

# 私有节点可用 → 开 oplus_chg / oplus_dual_cell；返回 1 表示「没有私有节点」，
# 调用方接着走标准节点校准。
apply_oplus_switches() {
    local meta="$1"
    local bcc=""
    if [ -f "$OPLUS_BCC" ]; then
        bcc=$(cat "$OPLUS_BCC" 2>/dev/null)
    fi
    if [ -z "$bcc" ]; then
        ui_print "$MSG_OPLUS_SKIP"
        return 1
    fi

    if grep -q "^oplus_chg: false" "$meta"; then
        $SED_CMD -i "s/^oplus_chg: false/oplus_chg: true/" "$meta"
        ui_print "$MSG_OPLUS_ON"
    fi
    # 字段数（逗号个数 + 1）先算一次：**cut 在字段不足时会把整行原样透传**（无分隔符
    # 的行默认输出整行），不先数逗号就会拿错值——下面两处都要用。
    local commas=$(printf '%s' "$bcc" | tr -cd ',')
    # 校准倍数：与标准节点路径同一个入口，原始值改取私有节点的电芯0电压（下标 6 =
    # 第 7 项，故需 ≥6 个逗号）。该节点通常报 mV/mA → 推算出 1000；万一某机型报
    # µV/µA 也能自动算对，不必硬编码。推算不出来就不写——此时 daemon 也用不了该
    # 节点、会回退标准节点，而模板缺省的 1000000 正是标准节点口径，正好对得上。
    if [ ${#commas} -ge 6 ]; then
        apply_divisor_from_raw "$meta" "$(printf '%s' "$bcc" | cut -d ',' -f 7)"
    fi
    # 双电芯 = 第 12 个字段（0 基下标 11）存在且为正数，与 daemon read_oplus_bcc 同口径。
    local f12=""
    if [ ${#commas} -ge 11 ]; then
        f12=$(printf '%s' "$bcc" | cut -d ',' -f 12 | tr -d ' \t\r\n')
    fi
    case "$f12" in
        ''|*[!0-9]*|0) ;;
        *)
            if grep -q "^oplus_dual_cell: false" "$meta"; then
                $SED_CMD -i "s/^oplus_dual_cell: false/oplus_dual_cell: true/" "$meta"
                ui_print "$MSG_OPLUS_DUAL_ON"
            fi
            ;;
    esac
    return 0
}

# 没有私有节点 → 走标准 power_supply 节点：取 voltage_now 交给统一入口推算。
apply_standard_divisor() {
    local meta="$1"
    ui_print "$MSG_VOLT_CHECK"
    local raw=""
    if [ -f "$STD_VOLT" ]; then
        raw=$(cat "$STD_VOLT" 2>/dev/null)
    fi
    apply_divisor_from_raw "$meta" "$raw" || ui_print "$MSG_VOLT_FAIL"
}

apply_battery_defaults() {
    # 无论走哪条分支都打印：只在成功时才打会让人分不清「没检测」和「检测了但没命中」
    ui_print "$MSG_OPLUS_CHECK"

    locate_meta_file
    if [ -z "$META_FILE" ]; then
        ui_print "$MSG_OPLUS_NO_META"
        return 0
    fi

    if [ -n "$BUSYBOX" ] && [ -x "$BUSYBOX" ]; then
        SED_CMD="$BUSYBOX sed"
    elif command -v sed >/dev/null 2>&1; then
        SED_CMD="sed"
    fi
    if [ -z "$SED_CMD" ]; then
        ui_print "$MSG_OPLUS_NO_SED"
        return 0
    fi

    if ! apply_oplus_switches "$META_FILE"; then
        apply_standard_divisor "$META_FILE"
    fi
}

# [config-keep]
# 「是否保留现有配置」：音量上键 = 保留（默认，超时也走这支），音量下键 = 不保留。
#   保留   → 删掉暂存目录的 config/：安装器随后不管走哪条路（完整安装的暂存落地、
#            热更新的 cp -r）都不会覆盖模块里已有的配置；
#   不保留 → 什么都不做，按当前流程由包内模板覆盖（旧行为）。
# 保留不等于放任旧文件：daemon 在启动与每次热重载前都会调 common::sync_meta_snapshot()
# ——缺键按内嵌默认补齐（meta 字段全部可选，老文件本来就合法），格式非法才用内嵌
# 默认整份覆盖并追加警告注释（meta 的 `nofix` 开关会跳过这层自愈）。
# 注意 config/ 里还有 i18n/*.ftl 与 normal/*.yaml 这类随包副本：运行期不读（都用
# 二进制内嵌的那份），保留时它们会停在旧版本。
ask_keep_config() {
    ui_print "$MSG_KEEP_CONFIG_ASK"
    ui_print "$MSG_KEEP_CONFIG_UP"
    ui_print "$MSG_KEEP_CONFIG_DOWN"
    ui_print " "

    detect_volume_key
    keep_result=$?
    if [ $keep_result -eq 2 ]; then
        ui_print "$MSG_INSTALL_CANCELLED"
        abort "$MSG_INSTALL_CANCELLED"
    fi
    if [ $keep_result -eq 0 ]; then
        ui_print "$MSG_KEEP_CONFIG_YES"
        CONFIG_KEPT=true
        rm -rf "$MODPATH/config"
    else
        ui_print "$MSG_KEEP_CONFIG_NO"
        CONFIG_KEPT=false
        # 覆盖安装：按机型把电池读数开关与校准倍数写好（两条安装路径都经过这里）
        apply_battery_defaults
    fi
    ui_print " "
}

# [mode-select] 
# 安装模式选择（音量键交互）
if [ "$HOT_UPDATE_AVAILABLE" = "true" ]; then
    # 热更新模式可用，显示选择菜单
    ui_print "$MSG_SELECT_MODE"
    ui_print "$MSG_VOLUME_UP"
    ui_print "$MSG_VOLUME_DOWN"
    ui_print " "
    
    # 等待用户按键选择（音量键检测函数见 [volume-key] 段）
    detect_volume_key
    choice_result=$?
    
    # 检查是否检测到多个按键事件（错误）
    if [ $choice_result -eq 2 ]; then
        ui_print "$MSG_INSTALL_CANCELLED"
        # 用 abort 而非 exit：exit 会跳过安装器收尾清理，modules_update 暂存
        # 残留会让管理器把模块标记为「待重启更新」、屏蔽 Action/WebUI
        abort "$MSG_INSTALL_CANCELLED"
    fi

    # 模式已选定：再问一次是否保留模块内已有的配置（完整安装与热更新共用这一问）
    ask_keep_config
    
# [hot-update-flow] 
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
        # 复活若落在下方文件复制窗口，cp 覆盖运行中的 chiri 会 ETXTBSY 失败且
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
            /system/bin/killall -9 chiri 2>/dev/null
        elif [ -n "$BUSYBOX" ]; then
            $BUSYBOX killall -9 chiri 2>/dev/null
        fi
        sleep 1

        ui_print "$MSG_STOPPING_MAIN"
        if [ -x "/system/bin/killall" ]; then
            /system/bin/killall -9 chiri 2>/dev/null
        elif [ -n "$BUSYBOX" ]; then
            $BUSYBOX killall -9 chiri 2>/dev/null
        fi
        sleep 1

        # 轮询确认 daemon 已真正退出（最多 ~5s）：卡在不可中断 IO（D 状态）的
        # 进程对 SIGKILL 也要等 IO 返回，未死透时 cp 覆盖二进制会 ETXTBSY
        CHIRI_ALIVE=true
        n=0
        while [ $n -lt 5 ]; do
            if [ -x "/system/bin/pidof" ]; then
                DAEMON_PID=$(/system/bin/pidof chiri 2>/dev/null)
            elif [ -n "$BUSYBOX" ]; then
                DAEMON_PID=$($BUSYBOX pgrep -x chiri 2>/dev/null)
            else
                DAEMON_PID=""
            fi
            if [ -z "$DAEMON_PID" ]; then
                CHIRI_ALIVE=false
                break
            fi
            sleep 1
            n=$((n + 1))
        done

        # 2. 复制模块文件到目标目录
        ui_print "$MSG_COPYING_FILES"
        # 备份用户可修改配置（meta.yaml 抬头字段；rules.yaml 在模块根，不在 config/ 子目录。
        # feature.yaml 等仅存于二进制，无需备份）
        if [ -f "$MODDIR/config/meta.yaml" ]; then
            cp "$MODDIR/config/meta.yaml" "$MODDIR/config/meta.yaml.bak"
        fi
        if [ -f "$MODDIR/rules.yaml" ]; then
            cp "$MODDIR/rules.yaml" "$MODDIR/rules.yaml.bak"
        fi

        # 复制新文件
        cp -r "$MODPATH"/* "$MODDIR/" 2>/dev/null

        # 二进制单独复制并校验：daemon 未完全退出（CHIRI_ALIVE）或内核仍持有
        # 文本页时 cp 会 ETXTBSY 失败——静默失败会残留旧版本，热更新后调度
        # 跑旧版。失败时恢复配置备份并重启旧版服务保持调度连续，明确告知
        # 用户当前为旧版。
        # 注意：热更新路径**必须以 abort 报错结束**（成功与失败皆然）——正常
        # 结束（exit 0）会让安装器保留 modules_update 暂存，KSU 把模块归为
        # 「待重启更新」，Action/WebUI 被禁用直到重启。
        UPDATE_OK=true
        if [ "$CHIRI_ALIVE" = "true" ] || ! cp "$MODPATH/core/bin/chiri" "$MODDIR/core/bin/chiri" 2>/dev/null; then
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
            if [ -f "$MODDIR/config/meta.yaml.bak" ]; then
                mv "$MODDIR/config/meta.yaml.bak" "$MODDIR/config/meta.yaml"
            fi
            if [ -f "$MODDIR/rules.yaml.bak" ]; then
                mv "$MODDIR/rules.yaml.bak" "$MODDIR/rules.yaml"
            fi
            chmod 755 "$MODDIR/core/bin/chiri" 2>/dev/null
            ui_print "ERROR: hot update copy failed! Old version kept."
            ui_print "错误：热更新文件复制失败！旧版本文件已保留。"
            ui_print "请手动执行 Action 启动旧版本调度。"
            ui_print "Run Action manually to start the old scheduler."
        fi

        # 恢复用户配置文件：只在「保留配置」那条路盖回去。
        # 选了覆盖（CONFIG_KEPT=false）时不再还原——这两份备份盖回去会把新包里的
        # meta.yaml 顶掉，本轮写入的电池校准倍数也就白写了；而 ChiRi 机型的生效配置
        # 本来是 config/<soc>/meta.yaml、压根不在这条备份链里（新文件直接生效），
        # 只有回退机型（生效配置就是根上那份 config/meta.yaml）才会被旧文件顶回，
        # 行为不一致。备份本身仍留给上面的复制失败回滚。
        if [ "$CONFIG_KEPT" != "false" ]; then
            if [ -f "$MODDIR/config/meta.yaml.bak" ]; then
                mv "$MODDIR/config/meta.yaml.bak" "$MODDIR/config/meta.yaml"
            fi
            if [ -f "$MODDIR/rules.yaml.bak" ]; then
                mv "$MODDIR/rules.yaml.bak" "$MODDIR/rules.yaml"
            fi
        else
            rm -f "$MODDIR/config/meta.yaml.bak" "$MODDIR/rules.yaml.bak"
        fi

        # 设置权限（注意二进制在 core/bin/chiri，模块根下并无 chiri 文件）
        chmod 755 "$MODDIR/service.sh" 2>/dev/null
        chmod 755 "$MODDIR/action.sh" 2>/dev/null
        chmod 755 "$MODDIR/core/bin/chiri" 2>/dev/null
        # 外部打包脚本（对外暴露的稳定接口，daemon 与 WebUI 共用）
        chmod 755 "$MODDIR/scripts/pack.sh" 2>/dev/null
        
        # 3.【已取消】重启调度服务（2026-09-17）：
        #    热更新后不再自动拉起调度——安装器环境里 setsid/nohup 拉起的 service.sh
        #    生命周期不可控（管理器退出后进程可能被收割），失败场景也无法在安装器
        #    里可靠提示。统一改为要求用户手动执行 Action 启动（见下方文案）。
        ui_print " "
        ui_print "$MSG_HOT_UPDATE_ABORT"

        # 5. 清理安装暂存 + 走官方失败路径结束安装。
        #    背景：安装器在执行 customize.sh **之前**已把 zip 解压到
        #    /data/adb/modules_update/<id>（暂存），脚本正常结束后由安装器
        #    收尾并标记「待重启应用」。热更新已把文件直接热替换到 live 目录，
        #    若让安装"成功"，管理器（KSU/Magisk）会按 modules_update/<id>
        #    的存在显示「需要重启更新」并屏蔽 Action/WebUI——必须以失败收场。
        #    此前用 exit 1 实现，但 KSU 官方文档明确：**exit 会跳过安装器的
        #    收尾清理步骤**——暂存目录残留在磁盘上，管理器照样按「有暂存 =
        #    待重启」标记模块（且旧暂存永久残留，之后每次热更新都无法解除，
        #    Action/WebUI 一直被屏蔽）。
        #    正确做法（官方 abort 语义）：
        #    1) 显式清理两处「待重启」标记——
        #       a. modules_update/chiri：本次安装的暂存（其内容已热替换进
        #          live 目录，无保留价值）及此前残留的旧暂存；
        #       b. 模块目录内的 update 标记文件：管理器判定「待重启更新」的
        #          另一依据（上次完整安装/被标记后遗留），一并删除；
        #       两处清理后 Action/WebUI 立即恢复可用、无需重启；
        #    2) abort 走官方失败路径：打印消息 + 执行收尾清理 + 安装器报失败，
        #       后续「完整安装」代码不会执行。
        #    顺序约束：必须位于所有 $MODPATH 读取（cp -r 源）之后；脚本自身
        #    由安装器从暂存 source 执行，unlink 不影响已打开的 fd，删除后
        #    仅剩 abort 一条语句。
        rm -rf /data/adb/modules_update/chiri
        rm -f /data/adb/modules/chiri/update
        abort "$MSG_HOT_UPDATE_ABORT"
    fi
else
    # 热更新不可用，显示提示信息
    ui_print "$MSG_HOT_UPDATE_UNAVAILABLE"
    ui_print " "
    # 没有模式选择，但「是否保留现有配置」照问（首次安装时没有旧配置，保留也无害）
    ask_keep_config
fi

# [full-install] 
# 完整安装流程（原逻辑）：模块文件由 Magisk 从暂存目录复制过来
# （唯一例外是 [battery-detect]：覆盖安装时会按机型修 staged 的 meta.yaml）

# 完整安装收尾：电池读数的开关与校准倍数已在 [config-keep] 里写好
# （放在那儿是为了热更新路径也走得到——热更新以 abort 结束时不会回到文件末尾）