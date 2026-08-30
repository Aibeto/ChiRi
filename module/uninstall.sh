#!/system/bin/sh

# 停止残留守护进程：模块卸载后运行中的进程不会自动退出，会继续锁频篡改 CPU，
# 必须先 killall（循环包装壳会因二进制文件被删除而自行退出）。
killall -9 yumi > /dev/null 2>&1
sleep 1

# 恢复被 CLG 锁定的 CPU 频率（守护进程被强杀时来不及 release，governor 可能停留在
# performance、min==max 锁频）：放宽到硬件全档并退回系统默认 schedutil governor。
for d in /sys/devices/system/cpu/cpufreq/policy*; do
  [ -f "$d/scaling_available_frequencies" ] || continue
  max_f=$(tr ' ' '\n' < "$d/scaling_available_frequencies" | sort -n | tail -1)
  min_f=$(tr ' ' '\n' < "$d/scaling_available_frequencies" | sort -n | head -1)
  [ -n "$max_f" ] && echo "$max_f" > "$d/scaling_max_freq" 2>/dev/null
  [ -n "$min_f" ] && echo "$min_f" > "$d/scaling_min_freq" 2>/dev/null
  if grep -q schedutil "$d/scaling_available_governors" 2>/dev/null; then
    echo schedutil > "$d/scaling_governor" 2>/dev/null
  fi
done

# 恢复 OPPO/OnePlus/Realme 的 Oiface
# if [ -n "$(getprop persist.sys.oiface.enable)" ]; then
#   setprop persist.sys.oiface.enable 1
# fi

# 恢复小米的 Joyose 服务
# PACKAGE_NAME="com.xiaomi.joyose"
# if pm list packages | grep -q "$PACKAGE_NAME"; then
#   pm enable "$PACKAGE_NAME" >/dev/null 2>&1
# fi

echo "卸载ChiRi调度成功完成 请重启手机"
